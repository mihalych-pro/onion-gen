//! The paired search engine.
//!
//! The chain engine in [`crate::batch`] pays a full point addition for every
//! candidate. This one keeps a table of affine offsets `Q(m) = 8m·G` and reads
//! **two** candidates out of one base point `P`, because on an Edwards curve
//! the sum and the difference share their expensive terms:
//!
//! ```text
//! y(P+Q) = (x1y1 - x2y2) / (x1y2 - y1x2)
//! y(P-Q) = (x1y1 + x2y2) / (x1y2 + y1x2)
//! ```
//!
//! `x1y2` and `y1x2` are the only multiplications, and `x1y1`, `x2y2` are
//! stored with the points. Two multiplications therefore produce two
//! candidates, against roughly eight per candidate for a point addition, and
//! the base point moves once per round rather than once per candidate.
//!
//! A round covers the deltas `c - 8·half … c + 8·half`, centre included, so
//! rounds tile the counter space when the centre advances by `8·(2·half+1)`.

use crate::curve::{self, Cached, Point};
use crate::field::Fe;
use crate::filter::LimbProbe;

/// The step between candidates. Clamping requires the low three bits of the
/// scalar to stay zero, and adding a multiple of eight leaves them alone.
pub const STEP: u64 = 8;

/// An offset from the table, affine, carrying the product of its coordinates
/// because every pair needs it and it never changes.
#[derive(Clone, Copy)]
struct Affine {
    x: Fe,
    y: Fe,
    xy: Fe,
}

pub struct PairEngine {
    /// `table[j]` is `8(j+1)·G`.
    table: Vec<Affine>,
    /// `8·(2·half+1)·G`, the distance between round centres.
    stride: Cached,
    /// Moves the block's start point to the first centre: `8·(half+1)·G`.
    to_first_centre: Cached,

    /// Numerators, with the running product of the denominators before them
    /// folded in. Holding the two together is what removes the third array a
    /// separate Montgomery scratch would need.
    num: Vec<Fe>,
    den: Vec<Fe>,
    /// Candidates the probe could not rule out, with their canonical bytes.
    /// Almost always empty, so the bytes are built a handful of times per
    /// round instead of once per candidate.
    survivors: Vec<(u32, [u8; 32])>,
    slots: usize,

    /// The current round's centre, affine.
    centre: Affine,
    /// The scalar delta of that centre from the block's base scalar.
    centre_delta: u64,
    started: bool,
    /// Whether this processor has the two-carry-chain multiply instructions.
    #[cfg(target_arch = "x86_64")]
    wide_carries: bool,
}

/// Which carry chains the paired engine will use on this machine.
///
/// The engine is the default path, so a measurement that does not say this is
/// one that cannot be compared against another machine's.
pub fn carry_path() -> &'static str {
    #[cfg(target_arch = "x86_64")]
    {
        if std::arch::is_x86_feature_detected!("bmi2") && std::arch::is_x86_feature_detected!("adx")
        {
            return "wide carries (mulx, adcx/adox)";
        }
        return "baseline carries";
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        "baseline carries"
    }
}

impl PairEngine {
    /// `batch_size` is a request: the engine produces `2·(batch_size/2)+1`
    /// candidates per round, the pairs plus their centre.
    pub fn new(batch_size: usize) -> Self {
        assert!(batch_size >= 2, "batch size must be at least two");
        let half = batch_size / 2;
        let two_d = curve::two_d();
        let step = curve::eight_basepoint_cached();

        // Q(1)…Q(half) by repeated addition, then one inversion for all of them.
        let mut points = Vec::with_capacity(half);
        let mut p = curve::to_p3(&curve::add(&Point::identity(), &step));
        for _ in 0..half {
            points.push(p);
            p = curve::to_p3(&curve::add(&p, &step));
        }
        let table = to_affine_batch(&points);

        // The centre advances by 8·(2·half+1); the first centre sits at
        // 8·(half+1) so that the smallest delta a round produces is 8.
        let stride = curve::to_cached(&multiple_of_eight_g(2 * half as u64 + 1), &two_d);
        let to_first_centre = curve::to_cached(&multiple_of_eight_g(half as u64 + 1), &two_d);

        let n = 2 * half + 1;
        PairEngine {
            table,
            stride,
            to_first_centre,
            num: vec![Fe::zero(); n],
            den: vec![Fe::zero(); n],
            survivors: Vec::with_capacity(64),
            slots: n,
            centre: Affine {
                x: Fe::zero(),
                y: Fe::zero(),
                xy: Fe::zero(),
            },
            centre_delta: 0,
            started: false,
            #[cfg(target_arch = "x86_64")]
            wide_carries: std::arch::is_x86_feature_detected!("bmi2")
                && std::arch::is_x86_feature_detected!("adx"),
        }
    }

    /// Candidates produced per round.
    pub fn batch_size(&self) -> usize {
        self.slots
    }

    /// Starts a block. `acc` must be the point of the block's base scalar; it is
    /// moved to the first round's centre.
    pub fn begin(&mut self, acc: &mut Point) {
        *acc = curve::to_p3(&curve::add(acc, &self.to_first_centre));
        self.centre_delta = STEP * (self.table.len() as u64 + 1);
        self.started = true;
    }

    /// Fills the batch from `acc`, tests every candidate, and moves `acc` to the
    /// next round's centre.
    ///
    /// `probe` is asked before a candidate is reduced to bytes. When it is
    /// `None` every candidate survives and is reduced, which is what a set
    /// holding a substring or a suffix needs.
    pub fn run(&mut self, acc: &mut Point, probe: Option<&LimbProbe>) {
        // The arithmetic is four 64-bit limbs fed through add-with-carry
        // chains, which is what `mulx` and `adcx`/`adox` exist for: they give
        // two independent carry chains where the baseline has one. The
        // baseline x86-64 target has neither, so the loop is compiled twice and
        // chosen once — the same thing the vector field does for AVX2, and the
        // reason one binary can stay portable without paying for it.
        #[cfg(target_arch = "x86_64")]
        if self.wide_carries {
            // SAFETY: the flag is set only where both features are detected.
            unsafe { self.run_wide(acc, probe) };
            return;
        }
        self.run_inner(acc, probe);
    }

    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "bmi2,adx")]
    unsafe fn run_wide(&mut self, acc: &mut Point, probe: Option<&LimbProbe>) {
        self.run_inner(acc, probe);
    }

    #[inline(always)]
    fn run_inner(&mut self, acc: &mut Point, probe: Option<&LimbProbe>) {
        debug_assert!(self.started, "begin() must run before the first batch");
        let half = self.table.len();
        let n = self.slots;

        self.centre = to_affine(acc);
        let pa = self.centre;

        // The running product of the denominators is folded into the numerators
        // as they are produced, so `num[i]` ends up holding
        // `num_i * d_0 * … * d_(i-1)`. Montgomery's trick then needs neither a
        // third array nor a second pass: the whole round touches each of the
        // two arrays once here and once below, where a separate forward pass
        // would read both and write one again.
        let mut running = Fe::one();
        for j in 0..half {
            let q = &self.table[j];
            let x1y2 = pa.x.mul(&q.y);
            let y1x2 = pa.y.mul(&q.x);

            let d0 = x1y2.sub(&y1x2);
            self.num[2 * j] = pa.xy.sub(&q.xy).mul(&running);
            self.den[2 * j] = d0;
            running = running.mul(&d0);

            let d1 = x1y2.add(&y1x2);
            self.num[2 * j + 1] = pa.xy.add(&q.xy).mul(&running);
            self.den[2 * j + 1] = d1;
            running = running.mul(&d1);
        }
        // The centre itself is already affine, so its fraction is y over one.
        self.num[n - 1] = pa.y.mul(&running);
        self.den[n - 1] = Fe::one();

        let mut inv = running.invert();

        self.survivors.clear();
        for i in (0..n).rev() {
            let v = inv.mul(&self.num[i]);
            inv = inv.mul(&self.den[i]);
            // The whole point of the probe: this is four limbs in registers,
            // and almost every candidate ends here.
            match probe {
                Some(p) if !p.may_match(v.0[0]) => {}
                _ => self.survivors.push((i as u32, v.to_bytes())),
            }
        }

        *acc = curve::to_p3(&curve::add(acc, &self.stride));
        self.centre_delta += STEP * (2 * half as u64 + 1);
    }

    /// The candidates of the most recent round the probe could not rule out,
    /// each with its index and its canonical bytes, **without the sign bit**.
    pub fn survivors(&self) -> &[(u32, [u8; 32])] {
        &self.survivors
    }

    /// The scalar delta of candidate `index`, counted from the block's base
    /// scalar. This is what a find reports and what the key is derived from.
    pub fn delta(&self, index: usize) -> u64 {
        let half = self.table.len();
        // The centre already moved on, so count back one stride.
        let centre = self.centre_delta - STEP * (2 * half as u64 + 1);
        if index == 2 * half {
            return centre;
        }
        let m = STEP * (index as u64 / 2 + 1);
        if index.is_multiple_of(2) {
            centre + m
        } else {
            centre - m
        }
    }

    /// The fully packed key of candidate `index`, sign bit included.
    ///
    /// The paired form carries `y` only, so the sign costs an inversion here.
    /// Matching reads [`PairEngine::packed`] and reaches this only for a
    /// candidate that already matched or that needs the checksum.
    pub fn packed_with_sign(&self, index: usize, packed: &[u8; 32]) -> [u8; 32] {
        let mut out = *packed;
        let half = self.table.len();
        let pa = &self.centre;

        let x = if index == 2 * half {
            pa.x
        } else {
            let q = &self.table[index / 2];
            let x1y2 = pa.x.mul(&q.y);
            let y1x2 = pa.y.mul(&q.x);
            let two = Fe::one().add(&Fe::one());
            let dp = curve::two_d().mul(&pa.x.mul(&q.x).mul(&pa.y.mul(&q.y)));
            // x(P±Q) = 2(x1y2 ± y1x2) / (2 ± 2d·x1x2·y1y2), scaled by two so the
            // curve constant can be used as it is stored.
            // An even index is P+Q, an odd one P-Q, matching the fractions
            // built in `run`.
            let (num, den) = if index.is_multiple_of(2) {
                (x1y2.add(&y1x2), two.add(&dp))
            } else {
                (x1y2.sub(&y1x2), two.sub(&dp))
            };
            num.add(&num).mul(&den.invert())
        };

        out[31] ^= x.is_negative() << 7;
        out
    }
}

/// `k·8·G`, used for the table's stride points.
fn multiple_of_eight_g(k: u64) -> Point {
    let mut scalar = [0u8; 32];
    let product = (k as u128) * 8;
    scalar[..16].copy_from_slice(&product.to_le_bytes());
    curve::scalar_base_mult(&scalar, &Point::basepoint(), &curve::two_d())
}

fn to_affine(p: &Point) -> Affine {
    let zinv = p.z.invert();
    let x = p.x.mul(&zinv);
    let y = p.y.mul(&zinv);
    Affine {
        x,
        y,
        xy: x.mul(&y),
    }
}

/// One inversion for the whole table rather than one per entry.
fn to_affine_batch(points: &[Point]) -> Vec<Affine> {
    let n = points.len();
    let mut prefix = vec![Fe::one(); n];
    let mut running = Fe::one();
    for i in 0..n {
        prefix[i] = running;
        running = running.mul(&points[i].z);
    }
    let mut inv = running.invert();
    let mut out = vec![
        Affine {
            x: Fe::zero(),
            y: Fe::zero(),
            xy: Fe::zero()
        };
        n
    ];
    for i in (0..n).rev() {
        let next = inv.mul(&points[i].z);
        let zinv = inv.mul(&prefix[i]);
        inv = next;
        let x = points[i].x.mul(&zinv);
        let y = points[i].y.mul(&zinv);
        out[i] = Affine {
            x,
            y,
            xy: x.mul(&y),
        };
    }
    out
}

#[cfg(test)]
impl PairEngine {
    /// Every candidate of the last round, indexed, for the tests. The product
    /// path asks the probe instead and never builds this.
    fn all(&self) -> Vec<(usize, [u8; 32])> {
        let mut v: Vec<(usize, [u8; 32])> = self
            .survivors
            .iter()
            .map(|(i, b)| (*i as usize, *b))
            .collect();
        v.sort_by_key(|(i, _)| *i);
        v
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

    /// Every candidate must be exactly the key that one-at-a-time scalar
    /// multiplication produces for the delta the engine reports. This is the
    /// only check that matters: it ties the fast path to the definition.
    #[test]
    fn every_candidate_matches_scalar_multiplication() {
        let seed = seed_from(7);
        let sk = key::secret_scalar(&seed);
        let mut acc = start_point(&seed);

        let mut engine = PairEngine::new(16);
        engine.begin(&mut acc);

        for _round in 0..3 {
            engine.run(&mut acc, None);
            for (i, bytes) in engine.all() {
                let mut expected_scalar = sk;
                key::add_to_scalar(&mut expected_scalar, engine.delta(i));
                let expected = key::pack(&curve::scalar_base_mult(
                    &expected_scalar,
                    &Point::basepoint(),
                    &curve::two_d(),
                ));
                assert_eq!(
                    engine.packed_with_sign(i, &bytes),
                    expected,
                    "candidate {i} at delta {}",
                    engine.delta(i)
                );
            }
        }
    }

    /// Rounds must tile the counter space: every multiple of eight from 8 up to
    /// the last candidate, each exactly once.
    #[test]
    fn rounds_tile_the_space_without_gaps_or_repeats() {
        let seed = seed_from(11);
        let mut acc = start_point(&seed);
        let mut engine = PairEngine::new(8);
        engine.begin(&mut acc);

        let mut seen = Vec::new();
        for _ in 0..5 {
            engine.run(&mut acc, None);
            for i in 0..engine.batch_size() {
                seen.push(engine.delta(i));
            }
        }
        seen.sort_unstable();
        let expected: Vec<u64> = (1..=seen.len() as u64).map(|k| k * STEP).collect();
        assert_eq!(seen, expected, "the rounds do not tile the deltas");
    }

    /// The paired engine and the chain engine must agree candidate for
    /// candidate, since both claim to enumerate the same space.
    #[test]
    fn agrees_with_the_chain_engine() {
        use crate::batch::BatchEngine;
        let seed = seed_from(13);

        let mut chain_acc = start_point(&seed);
        let mut chain = BatchEngine::new(256);
        chain.run(&mut chain_acc);
        // The chain starts at delta 0; the paired engine starts at 8.
        let chain_keys: Vec<[u8; 32]> = (1..64).map(|i| chain.packed_with_sign(i)).collect();

        let mut pair_acc = start_point(&seed);
        let mut pairs = PairEngine::new(8);
        pairs.begin(&mut pair_acc);
        let mut by_delta = std::collections::BTreeMap::new();
        for _ in 0..10 {
            pairs.run(&mut pair_acc, None);
            for (i, bytes) in pairs.all() {
                by_delta.insert(pairs.delta(i), pairs.packed_with_sign(i, &bytes));
            }
        }

        for (k, want) in chain_keys.iter().enumerate() {
            let delta = STEP * (k as u64 + 1);
            assert_eq!(by_delta.get(&delta), Some(want), "delta {delta} differs");
        }
    }

    /// The projective form of the same identity, which is what the device
    /// kernels use. With extended coordinates `Z` cancels out of both the
    /// numerator and the denominator, so no inversion is needed to reach the
    /// affine base point:
    ///
    /// ```text
    /// y(P±Q) = (T ± x2y2·Z) / (X·y2 ± Y·x2)
    /// ```
    #[test]
    fn the_projective_form_agrees_with_the_affine_one() {
        let seed = seed_from(23);
        let base = start_point(&seed);
        let two_d = curve::two_d();

        for m in 1..6u64 {
            let q_point = multiple_of_eight_g(m);
            let q = to_affine(&q_point);

            let xy2 = q.x.mul(&q.y);
            let a = base.x.mul(&q.y);
            let b = base.y.mul(&q.x);
            let czq = xy2.mul(&base.z);
            let sum = base.t.sub(&czq).mul(&a.sub(&b).invert());
            let dif = base.t.add(&czq).mul(&a.add(&b).invert());

            // What the group layer says, through the engine's own addition.
            let want_sum = curve::to_p3(&curve::add(&base, &curve::to_cached(&q_point, &two_d)));
            let neg = Point {
                x: Fe::zero().sub(&q_point.x),
                y: q_point.y,
                z: q_point.z,
                t: Fe::zero().sub(&q_point.t),
            };
            let want_dif = curve::to_p3(&curve::add(&base, &curve::to_cached(&neg, &two_d)));

            assert_eq!(
                sum.to_bytes(),
                want_sum.y.mul(&want_sum.z.invert()).to_bytes(),
                "projective P+Q at m={m}"
            );
            assert_eq!(
                dif.to_bytes(),
                want_dif.y.mul(&want_dif.z.invert()).to_bytes(),
                "projective P-Q at m={m}"
            );
        }
    }

    /// The probe may only ever say "no" to a candidate that would not have
    /// matched. A prefilter that drops a real hit shows up as keys never found,
    /// which is indistinguishable from bad luck, so it is checked against the
    /// full scan rather than argued about.
    #[test]
    fn the_probe_never_hides_a_match() {
        use crate::filter::FilterSet;

        // Short filters, so that hits are frequent enough to mean something.
        let filters = FilterSet::parse_all(["ab", "zq", "m2"]).expect("valid filters");
        let probe = filters.leading_probe().expect("a set of prefixes has one");

        let seed = seed_from(29);
        let mut guarded_acc = start_point(&seed);
        let mut full_acc = start_point(&seed);
        let mut guarded = PairEngine::new(256);
        let mut full = PairEngine::new(256);
        guarded.begin(&mut guarded_acc);
        full.begin(&mut full_acc);

        let mut checked = 0usize;
        let mut hits = 0usize;
        for _ in 0..40 {
            guarded.run(&mut guarded_acc, Some(&probe));
            full.run(&mut full_acc, None);

            let kept: std::collections::BTreeSet<u32> =
                guarded.survivors().iter().map(|(i, _)| *i).collect();
            for (i, bytes) in full.survivors() {
                checked += 1;
                if filters.match_key(bytes).is_some() {
                    hits += 1;
                    assert!(
                        kept.contains(i),
                        "the probe hid candidate {i}, which matches"
                    );
                }
            }
        }
        assert!(checked > 5_000, "too few candidates to mean anything");
        assert!(hits > 0, "no hits at all, so the test proves nothing");
    }

    /// Batch size is a performance knob: the set of keys must not depend on it.
    #[test]
    fn batch_size_does_not_change_the_keys() {
        let seed = seed_from(17);
        let mut reference: std::collections::BTreeMap<u64, [u8; 32]> = Default::default();

        for (run, size) in [8usize, 64, 250].into_iter().enumerate() {
            let mut acc = start_point(&seed);
            let mut engine = PairEngine::new(size);
            engine.begin(&mut acc);
            let mut got: std::collections::BTreeMap<u64, [u8; 32]> = Default::default();
            while got.len() < 400 {
                engine.run(&mut acc, None);
                for (i, bytes) in engine.all() {
                    got.insert(engine.delta(i), engine.packed_with_sign(i, &bytes));
                }
            }
            if run == 0 {
                reference = got;
            } else {
                for (d, key) in &reference {
                    if let Some(other) = got.get(d) {
                        assert_eq!(other, key, "batch size {size} differs at delta {d}");
                    }
                }
            }
        }
    }
}
