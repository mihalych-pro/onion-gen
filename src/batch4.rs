//! The batch engine over four interleaved chains.
//!
//! Same scheme as the scalar engine, with the parallelism taken where it is
//! available: the `+8G` chain is strictly sequential, so four chains run side
//! by side instead. Lane `j` starts at `A + j·8G` and every lane advances by
//! `4·8G`, which means the candidate at group `i`, lane `j` sits at offset
//! `8(4i + j)` — the same numbering the scalar engine produces, so callers do
//! not need to know which engine ran.
//!
//! Storage is structure-of-arrays throughout: a vector load wants limb `i` of
//! four candidates side by side, and converting per element would spend the
//! gain on shuffling.
//!
//! Two interleaved chain sets, to give the processor independent work while a
//! multiplication is in flight, measured no faster on either architecture.

use crate::curve::{self, Point};
use crate::curve4::{self, Cached4, Point4};
use crate::field::Fe;
use crate::field4::{self, Fe4, LANES};

/// The chain step, in candidates.
pub const CHAIN_STEP: u64 = 8;

/// One stored group: the coordinates of four candidates, without `T`.
///
/// `T` is needed to add points, so the running chains keep it, but a stored
/// candidate never needs it: packing reads `Y` and `Z`, and the sign reads `X`
/// and `Z`.
#[derive(Clone, Copy)]
struct StoredGroup {
    x: Fe4,
    y: Fe4,
    z: Fe4,
}

/// Reusable buffers for one worker.
pub struct BatchEngine4 {
    groups: Vec<StoredGroup>,
    scratch: Vec<Fe4>,
    packed: Vec<[u8; 32]>,
    step: Cached4,
}

impl BatchEngine4 {
    /// `batch_size` is rounded up to a whole number of interleaved groups.
    pub fn new(batch_size: usize) -> Self {
        assert!(batch_size > 0, "batch size must be positive");
        let groups = batch_size.div_ceil(LANES);
        let zero = StoredGroup {
            x: Fe4::zero(),
            y: Fe4::zero(),
            z: Fe4::zero(),
        };
        BatchEngine4 {
            groups: vec![zero; groups],
            scratch: vec![Fe4::zero(); groups],
            packed: vec![[0u8; 32]; groups * LANES],
            step: Cached4::broadcast(&curve4::chain_step(LANES as u64), &curve::two_d()),
        }
    }

    pub fn batch_size(&self) -> usize {
        self.groups.len() * LANES
    }

    /// Spreads one chain start across the lanes: lane `j` begins `j` steps in.
    pub fn split_chains(start: &Point) -> Point4 {
        let cached = curve::eight_basepoint_cached();
        let mut points = [*start; LANES];
        for j in 1..LANES {
            points[j] = curve::to_p3(&curve::add(&points[j - 1], &cached));
        }
        Point4::from_points(&points)
    }

    /// Advances the four chains through one batch, leaving the results in
    /// [`BatchEngine4::packed`].
    pub fn run(&mut self, chains: &mut Point4) {
        for slot in self.groups.iter_mut() {
            *slot = StoredGroup {
                x: chains.x,
                y: chains.y,
                z: chains.z,
            };
            *chains = curve4::step(chains, &self.step);
        }
        self.pack();
    }

    /// The packed candidates of the most recent [`BatchEngine4::run`], without
    /// sign bits. Index `4i + j` is the candidate at offset `8(4i + j)`.
    pub fn packed(&self) -> &[[u8; 32]] {
        &self.packed
    }

    /// Montgomery's trick, four lanes at a time.
    ///
    /// The product chain runs along the groups while each lane keeps its own
    /// accumulator, so one vector inversion serves four candidates — the
    /// amortisation that makes the scalar engine viable, applied twice over.
    ///
    /// One chain and not two: a second one to hide multiplication latency
    /// measures 8% slower, because the extra inversion costs about 265
    /// multiplications and the chain is limited by throughput rather than by
    /// latency.
    fn pack(&mut self) {
        let n = self.groups.len();

        let mut acc = Fe4::broadcast(&Fe::one());
        for i in 0..n {
            self.scratch[i] = acc;
            acc = field4::mul(&acc, &self.groups[i].z);
        }

        acc = acc.invert();

        for i in (0..n).rev() {
            let next = field4::mul(&acc, &self.groups[i].z);
            let zinv = field4::mul(&acc, &self.scratch[i]);
            acc = next;
            self.groups[i].z = zinv;

            // Packed without the canonical reduction. A field element can
            // encode differently from its canonical form only when its value
            // lands in `[p, 2^255)` — 19 values out of `2^255`, which no run
            // will ever produce. Matching therefore reads these bytes directly,
            // and the reduction is applied only to a candidate that matched,
            // where correctness of the final address does matter.
            let y = field4::mul(&self.groups[i].y, &zinv);
            for lane in 0..LANES {
                self.packed[i * LANES + lane] = y.canonical_lane_to_bytes(lane);
            }
        }
    }

    /// Applies the deferred sign bit to candidate `index`.
    ///
    /// Valid only for the batch produced by the most recent
    /// [`BatchEngine4::run`], because it consumes the `z⁻¹` packing left behind.
    pub fn finish_sign(&self, index: usize, packed: &mut [u8; 32]) {
        let group = index / LANES;
        let lane = index % LANES;
        let x = field4::mul(&self.groups[group].x, &self.groups[group].z).to_canonical();
        packed[31] ^= (x.canonical_lane_to_bytes(lane)[0] & 1) << 7;
    }

    /// The fully packed key of candidate `index`, canonically reduced and with
    /// the sign bit applied.
    ///
    /// This is the path a hit takes, so it pays for the reduction the hot loop
    /// skips.
    pub fn packed_with_sign(&self, index: usize) -> [u8; 32] {
        let group = index / LANES;
        let lane = index % LANES;
        // `z` holds `z⁻¹` after packing, so `y` is one multiplication away.
        // Recomputing beats keeping another array of the batch alive: the
        // first version stored it and lost a fifth of the throughput to the
        // extra cache pressure.
        let y = field4::mul(&self.groups[group].y, &self.groups[group].z);
        let mut out = y.to_canonical().canonical_lane_to_bytes(lane);
        self.finish_sign(index, &mut out);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::batch::BatchEngine;
    use crate::key;

    fn seed_from(n: u64) -> [u8; 32] {
        let mut b = [0u8; 32];
        let mut x = n.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(0xBEEF);
        for chunk in b.chunks_mut(8) {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            chunk.copy_from_slice(&x.to_le_bytes());
        }
        b
    }

    fn start(seed: u64) -> Point {
        let sk = key::secret_scalar(&seed_from(seed));
        curve::scalar_base_mult(&sk, &Point::basepoint(), &curve::two_d())
    }

    /// The two engines must produce the same candidates in the same order. This
    /// is the test that catches a lane mix-up: every candidate would still be a
    /// valid key, just attached to the wrong counter offset, and the secret
    /// recovered for a hit would not open the address.
    #[test]
    fn vector_engine_matches_the_scalar_engine() {
        for seed in 0..4u64 {
            let mut scalar_acc = start(seed);
            let mut scalar = BatchEngine::new(64);
            scalar.run(&mut scalar_acc);

            let mut chains = BatchEngine4::split_chains(&start(seed));
            let mut vector = BatchEngine4::new(64);
            vector.run(&mut chains);

            assert_eq!(scalar.batch_size(), vector.batch_size());
            for i in 0..vector.batch_size() {
                assert_eq!(
                    vector.packed_with_sign(i),
                    scalar.packed_with_sign(i),
                    "seed {seed} candidate {i}"
                );
            }
        }
    }

    /// Consecutive batches must tile the counter space without a gap or an
    /// overlap, exactly as the scalar engine does.
    #[test]
    fn consecutive_batches_continue_the_chains() {
        let mut scalar_acc = start(9);
        let mut scalar = BatchEngine::new(32);
        let mut chains = BatchEngine4::split_chains(&start(9));
        let mut vector = BatchEngine4::new(32);

        for round in 0..4 {
            scalar.run(&mut scalar_acc);
            vector.run(&mut chains);
            for i in 0..vector.batch_size() {
                assert_eq!(
                    vector.packed_with_sign(i),
                    scalar.packed_with_sign(i),
                    "round {round} candidate {i}"
                );
            }
        }
    }

    /// A candidate's offset has to stay `8·index`, or the secret recovered for
    /// a hit would be the wrong one.
    #[test]
    fn candidate_offsets_match_the_scalar_numbering() {
        let seed = seed_from(3);
        let sk = key::secret_scalar(&seed);
        let mut chains = BatchEngine4::split_chains(&curve::scalar_base_mult(
            &sk,
            &Point::basepoint(),
            &curve::two_d(),
        ));
        let mut vector = BatchEngine4::new(16);
        vector.run(&mut chains);

        for i in 0..vector.batch_size() {
            let mut expected_scalar = sk;
            key::add_to_scalar(&mut expected_scalar, CHAIN_STEP * i as u64);
            let expected = key::pack(&curve::scalar_base_mult(
                &expected_scalar,
                &Point::basepoint(),
                &curve::two_d(),
            ));
            assert_eq!(vector.packed_with_sign(i), expected, "candidate {i}");
        }
    }

    #[test]
    fn batch_size_rounds_up_to_whole_groups() {
        assert_eq!(BatchEngine4::new(1).batch_size(), LANES);
        assert_eq!(BatchEngine4::new(LANES).batch_size(), LANES);
        assert_eq!(BatchEngine4::new(LANES + 1).batch_size(), LANES * 2);
        assert_eq!(BatchEngine4::new(2048).batch_size(), 2048);
    }
}
