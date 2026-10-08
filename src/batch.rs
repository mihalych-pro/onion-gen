//! The batch search engine.
//!
//! One scalar multiplication per seed, after which the point advances by adding
//! the constant `8G`, and the whole batch is packed with a single field
//! inversion.
//!
//! Two things are left out of the batch:
//!
//! * **The `T` coordinate.** Adding points needs it, so the running accumulator
//!   keeps it, but a stored candidate never does: packing reads `Y` and `Z`,
//!   finishing the sign reads `X` and `Z`. Dropping it saves a quarter of the
//!   array and of its memory traffic.
//! * **The sign bit.** It costs one field multiplication per candidate and is
//!   only needed once a candidate has matched. Matching is unaffected: the sign
//!   lands in the top bit of the last byte, the final symbol of the address.

use crate::curve::{self, Cached, Point};
use crate::field::Fe;

/// The step of the search chain. The chain advances by `8G` rather than `G`
/// because clamping requires the low three bits of the scalar to stay zero,
/// and adding a multiple of eight leaves them alone.
pub const CHAIN_STEP: u64 = 8;

/// The default batch size.
///
/// One inversion is paid per batch, so a larger one spreads it further; the
/// two arrays the paired engine keeps grow with it and leave cache, and the
/// first effect wins until about eight thousand. Measured on an M1 Pro, eight
/// threads: 74.3 M/s at 512, 88.3 at 2048, 91.9 at 8192, 92.8 at 16384, 92.0
/// at 32768.
pub const DEFAULT_BATCH_SIZE: usize = 8192;

/// A stored candidate: the accumulator's coordinates without `T`.
#[derive(Clone, Copy)]
struct StoredPoint {
    x: Fe,
    y: Fe,
    z: Fe,
}

/// Reusable buffers for one worker. Allocation happens once, never inside the
/// hot loop.
pub struct BatchEngine {
    points: Vec<StoredPoint>,
    scratch: Vec<Fe>,
    packed: Vec<[u8; 32]>,
    step: Cached,
}

impl BatchEngine {
    pub fn new(batch_size: usize) -> Self {
        assert!(batch_size > 0, "batch size must be positive");
        let zero = StoredPoint {
            x: Fe::zero(),
            y: Fe::zero(),
            z: Fe::zero(),
        };
        BatchEngine {
            points: vec![zero; batch_size],
            scratch: vec![Fe::zero(); batch_size],
            packed: vec![[0u8; 32]; batch_size],
            step: curve::eight_basepoint_cached(),
        }
    }

    pub fn batch_size(&self) -> usize {
        self.points.len()
    }

    /// Advances `acc` through one batch, leaving the results in
    /// [`BatchEngine::packed`].
    ///
    /// Element `i` corresponds to the scalar the accumulator held on entry plus
    /// `8i`. The results are returned through a separate accessor rather than
    /// from this call so that the mutable borrow ends here: callers need to
    /// read the batch and call [`BatchEngine::finish_sign`] at the same time.
    pub fn run(&mut self, acc: &mut Point) {
        for slot in self.points.iter_mut() {
            *slot = StoredPoint {
                x: acc.x,
                y: acc.y,
                z: acc.z,
            };
            *acc = curve::to_p3(&curve::add(acc, &self.step));
        }
        self.pack();
    }

    /// The packed candidates of the most recent [`BatchEngine::run`], **without
    /// sign bits**. Matching reads these directly.
    pub fn packed(&self) -> &[[u8; 32]] {
        &self.packed
    }

    /// Montgomery's trick: one inversion for the whole batch instead of one per
    /// candidate. A single inversion costs ~265 multiplications, so amortising
    /// it over the batch is what makes the engine fast at all.
    ///
    /// Afterwards each point's `z` holds `z⁻¹`, which is what
    /// [`BatchEngine::finish_sign`] needs.
    fn pack(&mut self) {
        let n = self.points.len();

        // Forward pass: scratch[i] = z₀·…·zᵢ₋₁, acc ends as the product of all z.
        let mut acc = Fe::one();
        for i in 0..n {
            self.scratch[i] = acc;
            acc = acc.mul(&self.points[i].z);
        }

        acc = acc.invert();

        // Backward pass: peel one z off the accumulated inverse at a time.
        for i in (0..n).rev() {
            let next = acc.mul(&self.points[i].z);
            let zinv = acc.mul(&self.scratch[i]);
            acc = next;
            self.points[i].z = zinv;
            self.packed[i] = self.points[i].y.mul(&zinv).to_bytes();
        }
    }

    /// Applies the deferred sign bit to candidate `index`.
    ///
    /// Valid only for the batch produced by the most recent
    /// [`BatchEngine::run`], because it consumes the `z⁻¹` left behind by
    /// packing.
    pub fn finish_sign(&self, index: usize, packed: &mut [u8; 32]) {
        let p = &self.points[index];
        packed[31] ^= p.x.mul(&p.z).is_negative() << 7;
    }

    /// The fully packed key of candidate `index`, sign bit included.
    pub fn packed_with_sign(&self, index: usize) -> [u8; 32] {
        let mut out = self.packed[index];
        self.finish_sign(index, &mut out);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key;

    fn seed_from(n: u64) -> [u8; 32] {
        let mut b = [0u8; 32];
        let mut x = n.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(0xABCD);
        for chunk in b.chunks_mut(8) {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            chunk.copy_from_slice(&x.to_le_bytes());
        }
        b
    }

    fn start_point(seed: &[u8; 32]) -> Point {
        let sk = key::secret_scalar(seed);
        curve::scalar_base_mult(&sk, &Point::basepoint(), &curve::two_d())
    }

    /// The batch must produce exactly the keys that one-at-a-time scalar
    /// multiplication produces for the same counter range. This is the check
    /// that the chain steps by the right constant and that dropping `T` from
    /// storage changed nothing observable.
    #[test]
    fn batch_matches_scalar_multiplication() {
        let seed = seed_from(7);
        let sk = key::secret_scalar(&seed);
        let mut acc = start_point(&seed);

        let mut engine = BatchEngine::new(64);
        engine.run(&mut acc);

        for i in 0..engine.batch_size() {
            let mut expected_scalar = sk;
            key::add_to_scalar(&mut expected_scalar, CHAIN_STEP * i as u64);
            let expected = key::pack(&curve::scalar_base_mult(
                &expected_scalar,
                &Point::basepoint(),
                &curve::two_d(),
            ));
            assert_eq!(engine.packed_with_sign(i), expected, "candidate {i}");
        }
    }

    /// The accumulator must be left exactly one batch further along, so that
    /// consecutive batches tile the counter space without gaps or overlap.
    #[test]
    fn accumulator_advances_by_exactly_one_batch() {
        let seed = seed_from(11);
        let mut acc = start_point(&seed);
        let mut engine = BatchEngine::new(32);

        engine.run(&mut acc);
        let first_of_second = key::pack(&acc);

        engine.run(&mut acc);
        assert_eq!(
            engine.packed_with_sign(0),
            first_of_second,
            "the second batch must start where the accumulator was left"
        );
    }

    /// Batch inversion must agree with inverting each element on its own.
    #[test]
    fn batch_inversion_matches_elementwise() {
        let seed = seed_from(13);
        let mut acc = start_point(&seed);

        // Capture the z coordinates before packing overwrites them.
        let mut engine = BatchEngine::new(2048);
        let mut expected = Vec::with_capacity(engine.batch_size());
        {
            let mut probe = acc;
            for _ in 0..engine.batch_size() {
                expected.push(probe.z.invert());
                probe = curve::to_p3(&curve::add(&probe, &curve::eight_basepoint_cached()));
            }
        }

        engine.run(&mut acc);

        for (i, want) in expected.iter().enumerate() {
            assert!(
                engine.points[i].z.equals(want),
                "z inverse differs at index {i}"
            );
        }
    }

    /// Batch size is a performance knob, not a semantic one: the same root seed
    /// must yield the same candidates whatever the size.
    #[test]
    fn batch_size_does_not_change_results() {
        let seed = seed_from(17);
        const TOTAL: usize = 1536;
        let mut reference: Vec<[u8; 32]> = Vec::new();

        for (run, size) in [512usize, 8192, 3].into_iter().enumerate() {
            let mut acc = start_point(&seed);
            let mut engine = BatchEngine::new(size);
            let mut collected: Vec<[u8; 32]> = Vec::with_capacity(TOTAL);

            while collected.len() < TOTAL {
                engine.run(&mut acc);
                for i in 0..engine.batch_size() {
                    if collected.len() == TOTAL {
                        break;
                    }
                    collected.push(engine.packed_with_sign(i));
                }
            }

            if run == 0 {
                reference = collected;
            } else {
                assert_eq!(collected, reference, "batch size {size} diverged");
            }
        }
    }
}
