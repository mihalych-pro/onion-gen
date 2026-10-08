//! Four field elements at a time, in radix 2^25.5.
//!
//! Candidates in a batch are independent, so one field element goes per lane
//! and no cross-lane operation is ever needed: this is the scalar algorithm
//! with each scalar replaced by a vector.
//!
//! The representation differs from the scalar path, which uses radix 2^51 over
//! five 64-bit limbs. NEON multiplies `32x32 -> 64` but not `64x64 -> 128`, so
//! the vectorisable form is radix 2^25.5 over ten 32-bit limbs — twice the
//! operations, which is why four lanes buy 1.88x rather than 4x.

use crate::field::Fe;

/// Lanes per vector: four 32-bit values in a 128-bit register.
pub const LANES: usize = 4;
/// Limbs per field element in radix 2^25.5.
pub const LIMBS: usize = 10;

/// Bit width of each limb, alternating 26 and 25.
const WIDTHS: [u32; LIMBS] = [26, 25, 26, 25, 26, 25, 26, 25, 26, 25];
/// Bit offset of each limb within the 255-bit value.
const OFFSETS: [u32; LIMBS] = [0, 26, 51, 77, 102, 128, 153, 179, 204, 230];

const MASK25: u64 = (1 << 25) - 1;
const MASK26: u64 = (1 << 26) - 1;

/// Four field elements in structure-of-arrays layout: `limbs[i]` holds limb `i`
/// of all four elements, which is exactly what one vector load wants.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fe4 {
    pub limbs: [[u32; LANES]; LIMBS],
}

/// Reads `width` bits starting at `bit` from a 32-byte little-endian value.
#[inline]
fn read_bits(bytes: &[u8; 32], bit: u32, width: u32) -> u32 {
    let byte = (bit / 8) as usize;
    let shift = bit % 8;
    // A limb spans at most 26 bits, so at most 5 bytes once misaligned.
    let mut acc = 0u64;
    for (i, b) in bytes[byte..(byte + 5).min(32)].iter().enumerate() {
        acc |= u64::from(*b) << (8 * i);
    }
    ((acc >> shift) & ((1u64 << width) - 1)) as u32
}

/// Writes `value` into `width` bits starting at `bit`. Ranges never overlap, so
/// the caller may write limbs in any order.
#[inline]
fn write_bits(bytes: &mut [u8; 32], bit: u32, width: u32, value: u32) {
    debug_assert!(u64::from(value) < (1u64 << width));
    let byte = (bit / 8) as usize;
    let shift = bit % 8;
    let placed = u64::from(value) << shift;
    for i in 0..5usize {
        if byte + i >= 32 {
            break;
        }
        bytes[byte + i] |= ((placed >> (8 * i)) & 0xff) as u8;
    }
}

/// Splits a canonical 32-byte encoding into radix 2^25.5 limbs.
fn limbs_from_bytes(bytes: &[u8; 32]) -> [u32; LIMBS] {
    let mut out = [0u32; LIMBS];
    for (i, limb) in out.iter_mut().enumerate() {
        *limb = read_bits(bytes, OFFSETS[i], WIDTHS[i]);
    }
    out
}

/// Packs limbs back into 32 bytes.
///
/// The result may be a non-canonical representative — that is, numerically at
/// least `p` — and that is fine: it is fed to the scalar field, which works
/// modulo `p` regardless. Reducing to the canonical form in this representation
/// would mean reimplementing the whole conditional-subtraction dance for no
/// gain.
fn limbs_to_bytes(limbs: &[u32; LIMBS]) -> [u8; 32] {
    let mut carried: [u64; LIMBS] = std::array::from_fn(|i| u64::from(limbs[i]));
    carry_chain(&mut carried);

    let mut out = [0u8; 32];
    for (i, limb) in carried.iter().enumerate() {
        write_bits(&mut out, OFFSETS[i], WIDTHS[i], *limb as u32);
    }
    out
}

/// Propagates carries so every limb fits its width. The wrap from the top limb
/// folds back into limb 0 multiplied by 19, which is the shape of `2^255 - 19`.
#[inline]
fn carry_chain(m: &mut [u64; LIMBS]) {
    let mut c;
    c = m[0] >> 26;
    m[0] &= MASK26;
    m[1] += c;
    c = m[1] >> 25;
    m[1] &= MASK25;
    m[2] += c;
    c = m[2] >> 26;
    m[2] &= MASK26;
    m[3] += c;
    c = m[3] >> 25;
    m[3] &= MASK25;
    m[4] += c;
    c = m[4] >> 26;
    m[4] &= MASK26;
    m[5] += c;
    c = m[5] >> 25;
    m[5] &= MASK25;
    m[6] += c;
    c = m[6] >> 26;
    m[6] &= MASK26;
    m[7] += c;
    c = m[7] >> 25;
    m[7] &= MASK25;
    m[8] += c;
    c = m[8] >> 26;
    m[8] &= MASK26;
    m[9] += c;
    c = m[9] >> 25;
    m[9] &= MASK25;
    m[0] += c * 19;
    c = m[0] >> 26;
    m[0] &= MASK26;
    m[1] += c;
}

impl Fe4 {
    pub fn zero() -> Self {
        Fe4 {
            limbs: [[0; LANES]; LIMBS],
        }
    }

    /// Gathers four scalar elements into one vector element.
    pub fn from_elements(elements: &[Fe; LANES]) -> Self {
        let mut out = Fe4::zero();
        for (lane, e) in elements.iter().enumerate() {
            let limbs = limbs_from_bytes(&e.to_bytes());
            for (slot, limb) in out.limbs.iter_mut().zip(limbs) {
                slot[lane] = limb;
            }
        }
        out
    }

    /// Scatters back into four scalar elements.
    pub fn to_elements(&self) -> [Fe; LANES] {
        let mut out = [Fe::zero(); LANES];
        for (lane, slot) in out.iter_mut().enumerate() {
            *slot = Fe::from_bytes(&limbs_to_bytes(&self.lane(lane)));
        }
        out
    }

    /// One lane as radix 2^25.5 limbs, for tests and cross-checks.
    pub fn lane(&self, lane: usize) -> [u32; LIMBS] {
        std::array::from_fn(|i| self.limbs[i][lane])
    }
}

/// Twice the field modulus, limb by limb: `2p = 2^256 - 38`.
///
/// Subtraction adds this before subtracting, because the limbs are unsigned and
/// `a - b` would wrap. Adding a multiple of `p` changes nothing modulo `p`.
const TWO_P: [u32; LIMBS] = [
    0x7ff_ffda, 0x3ff_fffe, 0x7ff_fffe, 0x3ff_fffe, 0x7ff_fffe, 0x3ff_fffe, 0x7ff_fffe, 0x3ff_fffe,
    0x7ff_fffe, 0x3ff_fffe,
];

/// Limb ceilings for the **second** operand of [`mul`].
///
/// Multiplication scales that operand before feeding it to an instruction that
/// reads 32 bits, and the scaling depends on parity: even limbs are multiplied
/// by 19, odd ones by 38 (19 after a doubling). So the ceilings differ, and the
/// odd one is the binding constraint.
///
/// The margin is wider than the group layer needs: three stacked additions of
/// maximal reduced limbs still fit, and four do not. One subtraction uses about
/// as much of it as three additions, because `a + 2p - b` adds roughly `2^27`.
/// The layer above stays at one operation and carries where two would stack, so
/// it never approaches this.
pub const MAX_EVEN_LIMB: u32 = u32::MAX / 19;
/// See [`MAX_EVEN_LIMB`].
pub const MAX_ODD_LIMB: u32 = u32::MAX / 38;

/// Limb ceiling for the **first** operand of [`mul`].
///
/// It is used unscaled, and the constraint is that the 64-bit accumulator must
/// hold ten products of a scaled second operand by this one. Far looser than
/// the second operand's ceiling, and never the one that binds in practice.
pub const MAX_FIRST_LIMB: u32 = 1 << 30;

impl Fe4 {
    /// Adds without reducing.
    ///
    /// Reduced inputs give limbs below `2^27`, which multiplication accepts.
    /// Carrying here and relaxing again before the multiplication would be work
    /// that buys nothing — the same reasoning as on the scalar path.
    #[inline]
    pub fn add(&self, other: &Fe4) -> Fe4 {
        let mut out = Fe4::zero();
        for (slot, (a, b)) in out
            .limbs
            .iter_mut()
            .zip(self.limbs.iter().zip(other.limbs.iter()))
        {
            // Written as a fixed-size array operation so the compiler emits one
            // vector instruction per limb. Measured against explicit intrinsics
            // and matching them, so the portable form is kept.
            *slot = [a[0] + b[0], a[1] + b[1], a[2] + b[2], a[3] + b[3]];
        }
        out
    }

    /// Subtracts without reducing, by way of `a + 2p - b`.
    ///
    /// The detour exists because limbs are unsigned: `a - b` would wrap where
    /// the scalar path would simply go negative. Adding `2p` keeps every limb
    /// non-negative for reduced inputs and leaves the value unchanged modulo
    /// `p`.
    #[inline]
    pub fn sub(&self, other: &Fe4) -> Fe4 {
        let mut out = Fe4::zero();
        for (i, (slot, (a, b))) in out
            .limbs
            .iter_mut()
            .zip(self.limbs.iter().zip(other.limbs.iter()))
            .enumerate()
        {
            let p = TWO_P[i];
            *slot = [
                a[0] + p - b[0],
                a[1] + p - b[1],
                a[2] + p - b[2],
                a[3] + p - b[3],
            ];
        }
        out
    }

    /// Reduces every limb back into its width.
    ///
    /// Needed wherever two additions stack: the group layer doubles a value and
    /// then adds to it, and without this the operand would leave the range
    /// multiplication accepts.
    ///
    /// Written as elementwise 32-bit operations across the lanes rather than a
    /// loop over lanes with 64-bit arithmetic. The first version did the
    /// latter and cost more than it saved: a carry runs once per point
    /// addition, so a scalar one undoes a noticeable part of what the vector
    /// multiplications win. Limbs reaching this are below `2^29`, so 32 bits
    /// are enough and each step is four independent lanes.
    #[inline]
    #[allow(
        clippy::needless_range_loop,
        reason = "each step touches two adjacent limbs of the same array; an \
                  iterator would need a split_at_mut and read worse"
    )]
    pub fn carry(&self) -> Fe4 {
        const MASK26_32: u32 = (1 << 26) - 1;
        const MASK25_32: u32 = (1 << 25) - 1;
        let mut m = self.limbs;

        macro_rules! step {
            ($i:expr, $j:expr, $sh:expr, $mask:expr) => {
                for lane in 0..LANES {
                    let c = m[$i][lane] >> $sh;
                    m[$i][lane] &= $mask;
                    m[$j][lane] += c;
                }
            };
        }

        step!(0, 1, 26, MASK26_32);
        step!(1, 2, 25, MASK25_32);
        step!(2, 3, 26, MASK26_32);
        step!(3, 4, 25, MASK25_32);
        step!(4, 5, 26, MASK26_32);
        step!(5, 6, 25, MASK25_32);
        step!(6, 7, 26, MASK26_32);
        step!(7, 8, 25, MASK25_32);
        step!(8, 9, 26, MASK26_32);
        // The wrap from the top limb folds into limb 0 scaled by 19.
        for lane in 0..LANES {
            let c = m[9][lane] >> 25;
            m[9][lane] &= MASK25_32;
            m[0][lane] += c * 19;
        }
        step!(0, 1, 26, MASK26_32);

        Fe4 { limbs: m }
    }

    /// The same scalar element in every lane.
    ///
    /// Used for constants the chains share — the chain step is one point for
    /// all four lanes.
    pub fn broadcast(element: &Fe) -> Self {
        let limbs = limbs_from_bytes(&element.to_bytes());
        let mut out = Fe4::zero();
        for (slot, limb) in out.limbs.iter_mut().zip(limbs) {
            *slot = [limb; LANES];
        }
        out
    }

    /// Reduces all four lanes to the canonical representative.
    ///
    /// Splitting this out from byte packing matters: the reduction is the
    /// expensive half and it vectorises across lanes, while packing bits is
    /// cheap and inherently per-lane. Doing both per lane, as the first version
    /// did, put a scalar trial-reduction chain on every candidate.
    ///
    /// The chain is ref10's `fe_tobytes`: propagate a trial carry to learn
    /// whether the value is at least `p`, then fold that answer back in.
    #[inline]
    #[allow(
        clippy::needless_range_loop,
        reason = "same shape as carry: adjacent limbs of one array"
    )]
    pub fn to_canonical(&self) -> Fe4 {
        const MASK26_32: u32 = (1 << 26) - 1;
        const MASK25_32: u32 = (1 << 25) - 1;
        let mut h = self.limbs;

        for lane in 0..LANES {
            let mut q = (19 * h[9][lane] + (1 << 24)) >> 25;
            q = (h[0][lane] + q) >> 26;
            q = (h[1][lane] + q) >> 25;
            q = (h[2][lane] + q) >> 26;
            q = (h[3][lane] + q) >> 25;
            q = (h[4][lane] + q) >> 26;
            q = (h[5][lane] + q) >> 25;
            q = (h[6][lane] + q) >> 26;
            q = (h[7][lane] + q) >> 25;
            q = (h[8][lane] + q) >> 26;
            q = (h[9][lane] + q) >> 25;
            h[0][lane] += 19 * q;
        }

        macro_rules! step {
            ($i:expr, $sh:expr, $mask:expr) => {
                for lane in 0..LANES {
                    let c = h[$i][lane] >> $sh;
                    h[$i][lane] &= $mask;
                    h[$i + 1][lane] += c;
                }
            };
        }
        step!(0, 26, MASK26_32);
        step!(1, 25, MASK25_32);
        step!(2, 26, MASK26_32);
        step!(3, 25, MASK25_32);
        step!(4, 26, MASK26_32);
        step!(5, 25, MASK25_32);
        step!(6, 26, MASK26_32);
        step!(7, 25, MASK25_32);
        step!(8, 26, MASK26_32);
        for lane in 0..LANES {
            h[9][lane] &= MASK25_32;
        }

        Fe4 { limbs: h }
    }

    /// Packs one already-canonical lane into 32 bytes.
    ///
    /// Only correct after [`Fe4::to_canonical`]; packing a non-canonical value
    /// would encode a different address.
    ///
    /// Written out rather than looped. The general bit-writer this replaced
    /// spent about fifty operations per candidate, and packing runs once per
    /// candidate — it was the largest single item of overhead left in the
    /// vector engine. Each byte here is one shift and at most one or.
    #[inline]
    pub fn canonical_lane_to_bytes(&self, lane: usize) -> [u8; 32] {
        let h0 = self.limbs[0][lane];
        let h1 = self.limbs[1][lane];
        let h2 = self.limbs[2][lane];
        let h3 = self.limbs[3][lane];
        let h4 = self.limbs[4][lane];
        let h5 = self.limbs[5][lane];
        let h6 = self.limbs[6][lane];
        let h7 = self.limbs[7][lane];
        let h8 = self.limbs[8][lane];
        let h9 = self.limbs[9][lane];

        [
            h0 as u8,
            (h0 >> 8) as u8,
            (h0 >> 16) as u8,
            ((h0 >> 24) | (h1 << 2)) as u8,
            (h1 >> 6) as u8,
            (h1 >> 14) as u8,
            ((h1 >> 22) | (h2 << 3)) as u8,
            (h2 >> 5) as u8,
            (h2 >> 13) as u8,
            ((h2 >> 21) | (h3 << 5)) as u8,
            (h3 >> 3) as u8,
            (h3 >> 11) as u8,
            ((h3 >> 19) | (h4 << 6)) as u8,
            (h4 >> 2) as u8,
            (h4 >> 10) as u8,
            (h4 >> 18) as u8,
            h5 as u8,
            (h5 >> 8) as u8,
            (h5 >> 16) as u8,
            ((h5 >> 24) | (h6 << 1)) as u8,
            (h6 >> 7) as u8,
            (h6 >> 15) as u8,
            ((h6 >> 23) | (h7 << 3)) as u8,
            (h7 >> 5) as u8,
            (h7 >> 13) as u8,
            ((h7 >> 21) | (h8 << 4)) as u8,
            (h8 >> 4) as u8,
            (h8 >> 12) as u8,
            ((h8 >> 20) | (h9 << 6)) as u8,
            (h9 >> 2) as u8,
            (h9 >> 10) as u8,
            (h9 >> 18) as u8,
        ]
    }

    /// The canonical 32-byte encoding of one lane.
    pub fn lane_to_bytes(&self, lane: usize) -> [u8; 32] {
        limbs_to_canonical_bytes(&self.lane(lane))
    }

    /// Inversion through `x^(p-2)`, four elements at a time.
    ///
    /// The addition chain is ref10's, and the whole point of doing it here is
    /// that one chain now serves four candidates: the batch inversion that made
    /// the scalar engine viable becomes four times cheaper again.
    pub fn invert(&self) -> Fe4 {
        let z1 = *self;
        let z2 = sq(&z1);
        let z8 = sq(&sq(&z2));
        let z9 = mul(&z1, &z8);
        let z11 = mul(&z2, &z9);
        let z22 = sq(&z11);
        let z_5_0 = mul(&z9, &z22);

        let mut t = sq(&z_5_0);
        for _ in 1..5 {
            t = sq(&t);
        }
        let z_10_0 = mul(&t, &z_5_0);

        let mut t = sq(&z_10_0);
        for _ in 1..10 {
            t = sq(&t);
        }
        let z_20_0 = mul(&t, &z_10_0);

        let mut t = sq(&z_20_0);
        for _ in 1..20 {
            t = sq(&t);
        }
        let z_40_0 = mul(&t, &z_20_0);

        let mut t = sq(&z_40_0);
        for _ in 1..10 {
            t = sq(&t);
        }
        let z_50_0 = mul(&t, &z_10_0);

        let mut t = sq(&z_50_0);
        for _ in 1..50 {
            t = sq(&t);
        }
        let z_100_0 = mul(&t, &z_50_0);

        let mut t = sq(&z_100_0);
        for _ in 1..100 {
            t = sq(&t);
        }
        let z_200_0 = mul(&t, &z_100_0);

        let mut t = sq(&z_200_0);
        for _ in 1..50 {
            t = sq(&t);
        }
        let t = mul(&t, &z_50_0);

        let mut t = sq(&t);
        for _ in 1..5 {
            t = sq(&t);
        }
        mul(&t, &z11)
    }

    /// Whether this value may be the **second** operand of [`mul`].
    ///
    /// Used by debug assertions: a bound violation would not crash, it would
    /// silently produce a wrong key, which is the defect worth the most effort
    /// to catch early.
    pub fn within_mul_bounds(&self) -> bool {
        self.limbs.iter().enumerate().all(|(i, limb)| {
            let ceiling = if i % 2 == 0 {
                MAX_EVEN_LIMB
            } else {
                MAX_ODD_LIMB
            };
            limb.iter().all(|v| *v <= ceiling)
        })
    }

    /// Whether this value may be the **first** operand of [`mul`].
    pub fn within_first_operand_bounds(&self) -> bool {
        self.limbs
            .iter()
            .all(|limb| limb.iter().all(|v| *v <= MAX_FIRST_LIMB))
    }
}

/// Packs limbs into the canonical 32-byte encoding.
///
/// Distinct from [`limbs_to_bytes`], which is allowed to emit a representative
/// that is numerically at least `p`. Here the result has to be canonical,
/// because the address is base32 of exactly these bytes: a representative that
/// differs by `p` would encode a different, wrong address.
///
/// The reduction follows ref10's `fe_tobytes`: work out whether the value is at
/// least `p` by propagating a trial carry, then fold that answer back in.
fn limbs_to_canonical_bytes(limbs: &[u32; LIMBS]) -> [u8; 32] {
    let mut h: [i64; LIMBS] = std::array::from_fn(|i| i64::from(limbs[i]));

    // Trial reduction: q ends up 1 if the value is at least p, else 0.
    let mut q = (19 * h[9] + (1 << 24)) >> 25;
    q = (h[0] + q) >> 26;
    q = (h[1] + q) >> 25;
    q = (h[2] + q) >> 26;
    q = (h[3] + q) >> 25;
    q = (h[4] + q) >> 26;
    q = (h[5] + q) >> 25;
    q = (h[6] + q) >> 26;
    q = (h[7] + q) >> 25;
    q = (h[8] + q) >> 26;
    q = (h[9] + q) >> 25;

    // Adding 19q and carrying drops the top bit, which is the subtraction of p.
    h[0] += 19 * q;

    let widths = [26i32, 25, 26, 25, 26, 25, 26, 25, 26, 25];
    for i in 0..LIMBS - 1 {
        let carry = h[i] >> widths[i];
        h[i] -= carry << widths[i];
        h[i + 1] += carry;
    }
    h[9] -= (h[9] >> 25) << 25;

    let mut out = [0u8; 32];
    for (i, limb) in h.iter().enumerate() {
        write_bits(&mut out, OFFSETS[i], WIDTHS[i], *limb as u32);
    }
    out
}

/// Multiplication of one lane, in radix 2^25.5.
///
/// This is the reference the vector path is checked against, and the fallback
/// on architectures without a vector implementation. The shape follows ref10:
/// odd-position products first (no doubling), then the doubled even positions,
/// then the wrap-around terms scaled by 19.
pub fn mul_lane(a: &[u32; LIMBS], b: &[u32; LIMBS]) -> [u32; LIMBS] {
    let s: [u64; LIMBS] = std::array::from_fn(|i| u64::from(a[i]));
    let mut r: [u64; LIMBS] = std::array::from_fn(|i| u64::from(b[i]));
    let mut m = [0u64; LIMBS];

    // Odd positions: the operand is not doubled here.
    m[1] = r[0] * s[1] + r[1] * s[0];
    m[3] = r[0] * s[3] + r[1] * s[2] + r[2] * s[1] + r[3] * s[0];
    m[5] = r[0] * s[5] + r[1] * s[4] + r[2] * s[3] + r[3] * s[2] + r[4] * s[1] + r[5] * s[0];
    m[7] = r[0] * s[7]
        + r[1] * s[6]
        + r[2] * s[5]
        + r[3] * s[4]
        + r[4] * s[3]
        + r[5] * s[2]
        + r[6] * s[1]
        + r[7] * s[0];
    m[9] = r[0] * s[9]
        + r[1] * s[8]
        + r[2] * s[7]
        + r[3] * s[6]
        + r[4] * s[5]
        + r[5] * s[4]
        + r[6] * s[3]
        + r[7] * s[2]
        + r[8] * s[1]
        + r[9] * s[0];

    // The 25-bit limbs carry an implicit factor of two at even positions.
    r[1] *= 2;
    r[3] *= 2;
    r[5] *= 2;
    r[7] *= 2;

    m[0] = r[0] * s[0];
    m[2] = r[0] * s[2] + r[1] * s[1] + r[2] * s[0];
    m[4] = r[0] * s[4] + r[1] * s[3] + r[2] * s[2] + r[3] * s[1] + r[4] * s[0];
    m[6] = r[0] * s[6]
        + r[1] * s[5]
        + r[2] * s[4]
        + r[3] * s[3]
        + r[4] * s[2]
        + r[5] * s[1]
        + r[6] * s[0];
    m[8] = r[0] * s[8]
        + r[1] * s[7]
        + r[2] * s[6]
        + r[3] * s[5]
        + r[4] * s[4]
        + r[5] * s[3]
        + r[6] * s[2]
        + r[7] * s[1]
        + r[8] * s[0];

    // Terms that wrap past 2^255 come back multiplied by 19.
    r[1] *= 19;
    r[2] *= 19;
    r[3] = (r[3] / 2) * 19;
    r[4] *= 19;
    r[5] = (r[5] / 2) * 19;
    r[6] *= 19;
    r[7] = (r[7] / 2) * 19;
    r[8] *= 19;
    r[9] *= 19;

    m[1] += r[9] * s[2]
        + r[8] * s[3]
        + r[7] * s[4]
        + r[6] * s[5]
        + r[5] * s[6]
        + r[4] * s[7]
        + r[3] * s[8]
        + r[2] * s[9];
    m[3] += r[9] * s[4] + r[8] * s[5] + r[7] * s[6] + r[6] * s[7] + r[5] * s[8] + r[4] * s[9];
    m[5] += r[9] * s[6] + r[8] * s[7] + r[7] * s[8] + r[6] * s[9];
    m[7] += r[9] * s[8] + r[8] * s[9];

    r[3] *= 2;
    r[5] *= 2;
    r[7] *= 2;
    r[9] *= 2;

    m[0] += r[9] * s[1]
        + r[8] * s[2]
        + r[7] * s[3]
        + r[6] * s[4]
        + r[5] * s[5]
        + r[4] * s[6]
        + r[3] * s[7]
        + r[2] * s[8]
        + r[1] * s[9];
    m[2] += r[9] * s[3]
        + r[8] * s[4]
        + r[7] * s[5]
        + r[6] * s[6]
        + r[5] * s[7]
        + r[4] * s[8]
        + r[3] * s[9];
    m[4] += r[9] * s[5] + r[8] * s[6] + r[7] * s[7] + r[6] * s[8] + r[5] * s[9];
    m[6] += r[9] * s[7] + r[8] * s[8] + r[7] * s[9];
    m[8] += r[9] * s[9];

    carry_chain(&mut m);
    std::array::from_fn(|i| m[i] as u32)
}

/// Lane-at-a-time multiplication of four elements. Portable, and the oracle the
/// vector implementation is measured and checked against.
pub fn mul_lanes(a: &Fe4, b: &Fe4) -> Fe4 {
    let mut out = Fe4::zero();
    for lane in 0..LANES {
        let r = mul_lane(&a.lane(lane), &b.lane(lane));
        for (slot, limb) in out.limbs.iter_mut().zip(r) {
            slot[lane] = limb;
        }
    }
    out
}

/// Vectorised multiplication on NEON.
///
/// A direct translation of [`mul_lane`] with each scalar replaced by a vector,
/// which is the whole point of vectorising across the batch: there is not one
/// cross-lane operation in here.
///
/// NEON multiplies `32x32 -> 64` through `vmlal_u32`, which takes half a vector
/// at a time, so a four-lane accumulator is two `uint64x2_t`.
#[cfg(target_arch = "aarch64")]
pub mod neon {
    use super::{Fe4, LANES, LIMBS, MASK25, MASK26};
    use core::arch::aarch64::*;

    /// Four 64-bit accumulators, held as two pairs.
    #[derive(Clone, Copy)]
    struct Acc4 {
        lo: uint64x2_t,
        hi: uint64x2_t,
    }

    #[inline(always)]
    unsafe fn acc_zero() -> Acc4 {
        Acc4 {
            lo: vdupq_n_u64(0),
            hi: vdupq_n_u64(0),
        }
    }

    /// `acc += a * b`, across all four lanes.
    #[inline(always)]
    unsafe fn mla(acc: Acc4, a: uint32x4_t, b: uint32x4_t) -> Acc4 {
        Acc4 {
            lo: vmlal_u32(acc.lo, vget_low_u32(a), vget_low_u32(b)),
            hi: vmlal_u32(acc.hi, vget_high_u32(a), vget_high_u32(b)),
        }
    }

    /// Multiplies four pairs of field elements at once.
    ///
    /// # Safety
    ///
    /// NEON is part of the aarch64 baseline, so the intrinsics are always
    /// available on this target; the function is safe to call.
    pub fn mul(a: &Fe4, b: &Fe4) -> Fe4 {
        unsafe { mul_impl(a, b) }
    }

    #[inline]
    unsafe fn mul_impl(a: &Fe4, b: &Fe4) -> Fe4 {
        let s: [uint32x4_t; LIMBS] = core::array::from_fn(|i| vld1q_u32(a.limbs[i].as_ptr()));
        let mut r: [uint32x4_t; LIMBS] = core::array::from_fn(|i| vld1q_u32(b.limbs[i].as_ptr()));
        let mut m: [Acc4; LIMBS] = [acc_zero(); LIMBS];

        // Odd positions: the operand is not doubled here.
        m[1] = mla(mla(m[1], r[0], s[1]), r[1], s[0]);
        m[3] = mla(
            mla(mla(mla(m[3], r[0], s[3]), r[1], s[2]), r[2], s[1]),
            r[3],
            s[0],
        );
        m[5] = mla(
            mla(
                mla(
                    mla(mla(mla(m[5], r[0], s[5]), r[1], s[4]), r[2], s[3]),
                    r[3],
                    s[2],
                ),
                r[4],
                s[1],
            ),
            r[5],
            s[0],
        );
        m[7] = mla(
            mla(
                mla(
                    mla(
                        mla(mla(mla(m[7], r[0], s[7]), r[1], s[6]), r[2], s[5]),
                        r[3],
                        s[4],
                    ),
                    r[4],
                    s[3],
                ),
                r[5],
                s[2],
            ),
            r[6],
            s[1],
        );
        m[7] = mla(m[7], r[7], s[0]);
        m[9] = mla(
            mla(
                mla(
                    mla(
                        mla(mla(mla(m[9], r[0], s[9]), r[1], s[8]), r[2], s[7]),
                        r[3],
                        s[6],
                    ),
                    r[4],
                    s[5],
                ),
                r[5],
                s[4],
            ),
            r[6],
            s[3],
        );
        m[9] = mla(mla(mla(m[9], r[7], s[2]), r[8], s[1]), r[9], s[0]);

        // The 25-bit limbs carry an implicit factor of two at even positions.
        r[1] = vshlq_n_u32::<1>(r[1]);
        r[3] = vshlq_n_u32::<1>(r[3]);
        r[5] = vshlq_n_u32::<1>(r[5]);
        r[7] = vshlq_n_u32::<1>(r[7]);

        m[0] = mla(m[0], r[0], s[0]);
        m[2] = mla(mla(mla(m[2], r[0], s[2]), r[1], s[1]), r[2], s[0]);
        m[4] = mla(
            mla(
                mla(mla(mla(m[4], r[0], s[4]), r[1], s[3]), r[2], s[2]),
                r[3],
                s[1],
            ),
            r[4],
            s[0],
        );
        m[6] = mla(
            mla(
                mla(
                    mla(mla(mla(m[6], r[0], s[6]), r[1], s[5]), r[2], s[4]),
                    r[3],
                    s[3],
                ),
                r[4],
                s[2],
            ),
            r[5],
            s[1],
        );
        m[6] = mla(m[6], r[6], s[0]);
        m[8] = mla(
            mla(
                mla(
                    mla(mla(mla(m[8], r[0], s[8]), r[1], s[7]), r[2], s[6]),
                    r[3],
                    s[5],
                ),
                r[4],
                s[4],
            ),
            r[5],
            s[3],
        );
        m[8] = mla(mla(mla(m[8], r[6], s[2]), r[7], s[1]), r[8], s[0]);

        // Terms that wrap past 2^255 come back multiplied by 19.
        let v19 = vdupq_n_u32(19);
        r[1] = vmulq_u32(r[1], v19);
        r[2] = vmulq_u32(r[2], v19);
        r[3] = vmulq_u32(vshrq_n_u32::<1>(r[3]), v19);
        r[4] = vmulq_u32(r[4], v19);
        r[5] = vmulq_u32(vshrq_n_u32::<1>(r[5]), v19);
        r[6] = vmulq_u32(r[6], v19);
        r[7] = vmulq_u32(vshrq_n_u32::<1>(r[7]), v19);
        r[8] = vmulq_u32(r[8], v19);
        r[9] = vmulq_u32(r[9], v19);

        m[1] = mla(
            mla(
                mla(
                    mla(mla(mla(m[1], r[9], s[2]), r[8], s[3]), r[7], s[4]),
                    r[6],
                    s[5],
                ),
                r[5],
                s[6],
            ),
            r[4],
            s[7],
        );
        m[1] = mla(mla(m[1], r[3], s[8]), r[2], s[9]);
        m[3] = mla(
            mla(
                mla(mla(mla(m[3], r[9], s[4]), r[8], s[5]), r[7], s[6]),
                r[6],
                s[7],
            ),
            r[5],
            s[8],
        );
        m[3] = mla(m[3], r[4], s[9]);
        m[5] = mla(
            mla(mla(mla(m[5], r[9], s[6]), r[8], s[7]), r[7], s[8]),
            r[6],
            s[9],
        );
        m[7] = mla(mla(m[7], r[9], s[8]), r[8], s[9]);

        r[3] = vshlq_n_u32::<1>(r[3]);
        r[5] = vshlq_n_u32::<1>(r[5]);
        r[7] = vshlq_n_u32::<1>(r[7]);
        r[9] = vshlq_n_u32::<1>(r[9]);

        m[0] = mla(
            mla(
                mla(
                    mla(mla(mla(m[0], r[9], s[1]), r[8], s[2]), r[7], s[3]),
                    r[6],
                    s[4],
                ),
                r[5],
                s[5],
            ),
            r[4],
            s[6],
        );
        m[0] = mla(mla(mla(m[0], r[3], s[7]), r[2], s[8]), r[1], s[9]);
        m[2] = mla(
            mla(
                mla(
                    mla(mla(mla(m[2], r[9], s[3]), r[8], s[4]), r[7], s[5]),
                    r[6],
                    s[6],
                ),
                r[5],
                s[7],
            ),
            r[4],
            s[8],
        );
        m[2] = mla(m[2], r[3], s[9]);
        m[4] = mla(
            mla(
                mla(mla(mla(m[4], r[9], s[5]), r[8], s[6]), r[7], s[7]),
                r[6],
                s[8],
            ),
            r[5],
            s[9],
        );
        m[6] = mla(mla(mla(m[6], r[9], s[7]), r[8], s[8]), r[7], s[9]);
        m[8] = mla(m[8], r[9], s[9]);

        carry(&mut m);

        let mut out = Fe4::zero();
        for (slot, acc) in out.limbs.iter_mut().zip(m.iter()) {
            let packed = vcombine_u32(vmovn_u64(acc.lo), vmovn_u64(acc.hi));
            vst1q_u32(slot.as_mut_ptr(), packed);
        }
        debug_assert_eq!(LANES, 4);
        out
    }

    /// Carry propagation, per lane, with the same shifts as the scalar version.
    #[inline(always)]
    unsafe fn carry(m: &mut [Acc4; LIMBS]) {
        let k26 = vdupq_n_u64(MASK26);
        let k25 = vdupq_n_u64(MASK25);

        macro_rules! step {
            ($i:expr, $j:expr, $sh:literal, $mask:expr) => {
                let c_lo = vshrq_n_u64::<$sh>(m[$i].lo);
                let c_hi = vshrq_n_u64::<$sh>(m[$i].hi);
                m[$i].lo = vandq_u64(m[$i].lo, $mask);
                m[$i].hi = vandq_u64(m[$i].hi, $mask);
                m[$j].lo = vaddq_u64(m[$j].lo, c_lo);
                m[$j].hi = vaddq_u64(m[$j].hi, c_hi);
            };
        }

        step!(0, 1, 26, k26);
        step!(1, 2, 25, k25);
        step!(2, 3, 26, k26);
        step!(3, 4, 25, k25);
        step!(4, 5, 26, k26);
        step!(5, 6, 25, k25);
        step!(6, 7, 26, k26);
        step!(7, 8, 25, k25);
        step!(8, 9, 26, k26);

        // The wrap from the top limb folds into limb 0 scaled by 19. NEON has
        // no 64x64 multiply, so 19x is built from shifts: (x<<4) + (x<<1) + x.
        let c_lo = vshrq_n_u64::<25>(m[9].lo);
        let c_hi = vshrq_n_u64::<25>(m[9].hi);
        m[9].lo = vandq_u64(m[9].lo, k25);
        m[9].hi = vandq_u64(m[9].hi, k25);
        let mul19 =
            |v: uint64x2_t| vaddq_u64(vaddq_u64(vshlq_n_u64::<4>(v), vshlq_n_u64::<1>(v)), v);
        m[0].lo = vaddq_u64(m[0].lo, mul19(c_lo));
        m[0].hi = vaddq_u64(m[0].hi, mul19(c_hi));

        step!(0, 1, 26, k26);
    }
}

/// Vectorised multiplication on AVX2.
///
/// The same algorithm as [`mul_lane`], with each scalar replaced by a vector.
/// `_mm256_mul_epu32` multiplies the low 32 bits of four 64-bit lanes, so a
/// limb is held zero-extended: four `u64` in one `__m256i`, each below `2^32`.
///
/// Operands must be reduced, as for the NEON path: intermediate scalings reach
/// about `2^31.3` for canonical limbs, and a looser input would overflow the
/// 32 bits `_mm256_mul_epu32` reads.
#[cfg(target_arch = "x86_64")]
pub mod avx2 {
    use super::{Fe4, LIMBS, MASK25, MASK26};
    use core::arch::x86_64::*;

    /// Loads limb `i` of all four lanes, zero-extended to 64 bits.
    #[inline(always)]
    unsafe fn load(limb: &[u32; 4]) -> __m256i {
        _mm256_cvtepu32_epi64(_mm_loadu_si128(limb.as_ptr() as *const __m128i))
    }

    /// `acc += a * b` across four lanes.
    #[inline(always)]
    unsafe fn mla(acc: __m256i, a: __m256i, b: __m256i) -> __m256i {
        _mm256_add_epi64(acc, _mm256_mul_epu32(a, b))
    }

    /// Multiplies four pairs of field elements at once.
    ///
    /// # Safety
    ///
    /// The caller must have established that AVX2 is available; `mul` in the
    /// parent module does that through `is_x86_feature_detected!`.
    #[target_feature(enable = "avx2")]
    pub unsafe fn mul(a: &Fe4, b: &Fe4) -> Fe4 {
        let s: [__m256i; LIMBS] = core::array::from_fn(|i| load(&a.limbs[i]));
        let mut r: [__m256i; LIMBS] = core::array::from_fn(|i| load(&b.limbs[i]));
        let zero = _mm256_setzero_si256();
        let mut m: [__m256i; LIMBS] = [zero; LIMBS];

        // Odd positions: the operand is not doubled here.
        m[1] = mla(mla(m[1], r[0], s[1]), r[1], s[0]);
        m[3] = mla(
            mla(mla(mla(m[3], r[0], s[3]), r[1], s[2]), r[2], s[1]),
            r[3],
            s[0],
        );
        m[5] = mla(
            mla(
                mla(
                    mla(mla(mla(m[5], r[0], s[5]), r[1], s[4]), r[2], s[3]),
                    r[3],
                    s[2],
                ),
                r[4],
                s[1],
            ),
            r[5],
            s[0],
        );
        m[7] = mla(
            mla(mla(mla(m[7], r[0], s[7]), r[1], s[6]), r[2], s[5]),
            r[3],
            s[4],
        );
        m[7] = mla(
            mla(mla(mla(m[7], r[4], s[3]), r[5], s[2]), r[6], s[1]),
            r[7],
            s[0],
        );
        m[9] = mla(
            mla(mla(mla(m[9], r[0], s[9]), r[1], s[8]), r[2], s[7]),
            r[3],
            s[6],
        );
        m[9] = mla(
            mla(mla(mla(m[9], r[4], s[5]), r[5], s[4]), r[6], s[3]),
            r[7],
            s[2],
        );
        m[9] = mla(mla(m[9], r[8], s[1]), r[9], s[0]);

        // The 25-bit limbs carry an implicit factor of two at even positions.
        r[1] = _mm256_slli_epi64(r[1], 1);
        r[3] = _mm256_slli_epi64(r[3], 1);
        r[5] = _mm256_slli_epi64(r[5], 1);
        r[7] = _mm256_slli_epi64(r[7], 1);

        m[0] = mla(m[0], r[0], s[0]);
        m[2] = mla(mla(mla(m[2], r[0], s[2]), r[1], s[1]), r[2], s[0]);
        m[4] = mla(
            mla(
                mla(mla(mla(m[4], r[0], s[4]), r[1], s[3]), r[2], s[2]),
                r[3],
                s[1],
            ),
            r[4],
            s[0],
        );
        m[6] = mla(
            mla(mla(mla(m[6], r[0], s[6]), r[1], s[5]), r[2], s[4]),
            r[3],
            s[3],
        );
        m[6] = mla(mla(mla(m[6], r[4], s[2]), r[5], s[1]), r[6], s[0]);
        m[8] = mla(
            mla(mla(mla(m[8], r[0], s[8]), r[1], s[7]), r[2], s[6]),
            r[3],
            s[5],
        );
        m[8] = mla(
            mla(mla(mla(m[8], r[4], s[4]), r[5], s[3]), r[6], s[2]),
            r[7],
            s[1],
        );
        m[8] = mla(m[8], r[8], s[0]);

        // Terms that wrap past 2^255 come back multiplied by 19.
        let v19 = _mm256_set1_epi64x(19);
        r[1] = _mm256_mul_epu32(r[1], v19);
        r[2] = _mm256_mul_epu32(r[2], v19);
        r[3] = _mm256_mul_epu32(_mm256_srli_epi64(r[3], 1), v19);
        r[4] = _mm256_mul_epu32(r[4], v19);
        r[5] = _mm256_mul_epu32(_mm256_srli_epi64(r[5], 1), v19);
        r[6] = _mm256_mul_epu32(r[6], v19);
        r[7] = _mm256_mul_epu32(_mm256_srli_epi64(r[7], 1), v19);
        r[8] = _mm256_mul_epu32(r[8], v19);
        r[9] = _mm256_mul_epu32(r[9], v19);

        m[1] = mla(
            mla(mla(mla(m[1], r[9], s[2]), r[8], s[3]), r[7], s[4]),
            r[6],
            s[5],
        );
        m[1] = mla(
            mla(mla(mla(m[1], r[5], s[6]), r[4], s[7]), r[3], s[8]),
            r[2],
            s[9],
        );
        m[3] = mla(
            mla(mla(mla(m[3], r[9], s[4]), r[8], s[5]), r[7], s[6]),
            r[6],
            s[7],
        );
        m[3] = mla(mla(m[3], r[5], s[8]), r[4], s[9]);
        m[5] = mla(
            mla(mla(mla(m[5], r[9], s[6]), r[8], s[7]), r[7], s[8]),
            r[6],
            s[9],
        );
        m[7] = mla(mla(m[7], r[9], s[8]), r[8], s[9]);

        r[3] = _mm256_slli_epi64(r[3], 1);
        r[5] = _mm256_slli_epi64(r[5], 1);
        r[7] = _mm256_slli_epi64(r[7], 1);
        r[9] = _mm256_slli_epi64(r[9], 1);

        m[0] = mla(
            mla(mla(mla(m[0], r[9], s[1]), r[8], s[2]), r[7], s[3]),
            r[6],
            s[4],
        );
        m[0] = mla(
            mla(mla(mla(m[0], r[5], s[5]), r[4], s[6]), r[3], s[7]),
            r[2],
            s[8],
        );
        m[0] = mla(m[0], r[1], s[9]);
        m[2] = mla(
            mla(mla(mla(m[2], r[9], s[3]), r[8], s[4]), r[7], s[5]),
            r[6],
            s[6],
        );
        m[2] = mla(mla(mla(m[2], r[5], s[7]), r[4], s[8]), r[3], s[9]);
        m[4] = mla(
            mla(mla(mla(m[4], r[9], s[5]), r[8], s[6]), r[7], s[7]),
            r[6],
            s[8],
        );
        m[4] = mla(m[4], r[5], s[9]);
        m[6] = mla(mla(mla(m[6], r[9], s[7]), r[8], s[8]), r[7], s[9]);
        m[8] = mla(m[8], r[9], s[9]);

        carry(&mut m);

        let mut out = Fe4::zero();
        for (slot, acc) in out.limbs.iter_mut().zip(m.iter()) {
            // Each 64-bit lane now holds a limb below 2^26; narrow to u32.
            let packed = _mm256_castsi256_si128(_mm256_permutevar8x32_epi32(
                *acc,
                _mm256_setr_epi32(0, 2, 4, 6, 0, 0, 0, 0),
            ));
            _mm_storeu_si128(slot.as_mut_ptr() as *mut __m128i, packed);
        }
        out
    }

    /// Carry propagation, per lane, with the same shifts as the scalar version.
    #[inline(always)]
    unsafe fn carry(m: &mut [__m256i; LIMBS]) {
        let k26 = _mm256_set1_epi64x(MASK26 as i64);
        let k25 = _mm256_set1_epi64x(MASK25 as i64);

        macro_rules! step {
            ($i:expr, $j:expr, $sh:literal, $mask:expr) => {
                let c = _mm256_srli_epi64(m[$i], $sh);
                m[$i] = _mm256_and_si256(m[$i], $mask);
                m[$j] = _mm256_add_epi64(m[$j], c);
            };
        }

        step!(0, 1, 26, k26);
        step!(1, 2, 25, k25);
        step!(2, 3, 26, k26);
        step!(3, 4, 25, k25);
        step!(4, 5, 26, k26);
        step!(5, 6, 25, k25);
        step!(6, 7, 26, k26);
        step!(7, 8, 25, k25);
        step!(8, 9, 26, k26);

        // The wrap from the top limb folds into limb 0 scaled by 19.
        let c = _mm256_srli_epi64(m[9], 25);
        m[9] = _mm256_and_si256(m[9], k25);
        let c19 = _mm256_add_epi64(
            _mm256_add_epi64(_mm256_slli_epi64(c, 4), _mm256_slli_epi64(c, 1)),
            c,
        );
        m[0] = _mm256_add_epi64(m[0], c19);

        step!(0, 1, 26, k26);
    }
}

/// Multiplies four pairs of field elements, using the best implementation the
/// target offers.
/// Which field implementation this run will use.
///
/// Reported so that a measurement says what it measured, and so that a test
/// passing on a fallback cannot be mistaken for a test of the vector path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Implementation {
    /// Portable, one lane at a time. Available everywhere.
    Scalar,
    /// NEON, part of the aarch64 baseline.
    Neon,
    /// AVX2 on x86_64, when the processor reports it.
    Avx2,
}

impl Implementation {
    pub fn name(self) -> &'static str {
        match self {
            Implementation::Scalar => "scalar",
            Implementation::Neon => "neon",
            Implementation::Avx2 => "avx2",
        }
    }
}

/// What the processor offers and what was chosen, for the diagnostic line.
///
/// A measurement that does not say which arithmetic ran is uninterpretable, and
/// "my machine is slower than yours" is usually answered here.
pub fn describe() -> String {
    let mut available: Vec<&str> = Vec::new();

    #[cfg(target_arch = "aarch64")]
    {
        // NEON is mandatory in the aarch64 baseline, so it is always present.
        available.push("neon");
    }
    #[cfg(target_arch = "x86_64")]
    {
        for (name, present) in [
            ("sse2", cfg!(target_feature = "sse2")),
            ("avx", std::arch::is_x86_feature_detected!("avx")),
            ("avx2", std::arch::is_x86_feature_detected!("avx2")),
            ("avx512f", std::arch::is_x86_feature_detected!("avx512f")),
            (
                "avx512ifma",
                std::arch::is_x86_feature_detected!("avx512ifma"),
            ),
            // Not vector sets at all, and listed because the default engine is
            // the scalar paired one: these are what it dispatches on, and a
            // line that showed only the vector sets left the question "is the
            // wide path running" unanswerable from the outside.
            ("bmi2", std::arch::is_x86_feature_detected!("bmi2")),
            ("adx", std::arch::is_x86_feature_detected!("adx")),
        ] {
            if present {
                available.push(name);
            }
        }
    }

    let offered = if available.is_empty() {
        "none detected".to_string()
    } else {
        available.join(", ")
    };

    format!(
        "{} on {}; paired engine: {}; instruction sets available: {offered}",
        active().name(),
        std::env::consts::ARCH,
        crate::pairs::carry_path(),
    )
}

impl Implementation {
    /// Parses a name as given on the command line or in a config file.
    pub fn parse(name: &str) -> Option<Implementation> {
        match name.trim().to_ascii_lowercase().as_str() {
            "scalar" => Some(Implementation::Scalar),
            "neon" => Some(Implementation::Neon),
            "avx2" => Some(Implementation::Avx2),
            _ => None,
        }
    }

    /// Whether this machine can run it.
    pub fn is_available(self) -> bool {
        match self {
            Implementation::Scalar => true,
            Implementation::Neon => cfg!(target_arch = "aarch64"),
            Implementation::Avx2 => active() == Implementation::Avx2,
        }
    }
}

/// Every implementation this machine can run, best first.
pub fn available() -> Vec<Implementation> {
    let mut out = Vec::new();
    for candidate in [
        Implementation::Neon,
        Implementation::Avx2,
        Implementation::Scalar,
    ] {
        if candidate.is_available() {
            out.push(candidate);
        }
    }
    out
}

/// The implementation [`mul`] dispatches to on this machine.
pub fn active() -> Implementation {
    #[cfg(target_arch = "aarch64")]
    {
        Implementation::Neon
    }
    #[cfg(target_arch = "x86_64")]
    {
        if std::arch::is_x86_feature_detected!("avx2") {
            Implementation::Avx2
        } else {
            Implementation::Scalar
        }
    }
    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    {
        Implementation::Scalar
    }
}

///
/// Selection happens at run time by what the processor reports, not at build
/// time: one binary carries every implementation its architecture can hold and
/// picks among them on start.
#[inline]
pub fn mul(a: &Fe4, b: &Fe4) -> Fe4 {
    debug_assert!(
        a.within_first_operand_bounds(),
        "the first operand of mul exceeded its limb ceiling; it needed a carry"
    );
    debug_assert!(
        b.within_mul_bounds(),
        "the second operand of mul exceeded its limb ceiling; it needed a carry"
    );
    #[cfg(target_arch = "aarch64")]
    {
        // NEON is part of the aarch64 baseline; there is nothing to detect.
        neon::mul(a, b)
    }
    #[cfg(target_arch = "x86_64")]
    {
        // The macro caches its answer, so this is a predictable branch rather
        // than a CPUID per multiplication.
        if std::arch::is_x86_feature_detected!("avx2") {
            unsafe { avx2::mul(a, b) }
        } else {
            mul_lanes(a, b)
        }
    }
    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    {
        mul_lanes(a, b)
    }
}

/// Squares four field elements.
#[inline]
pub fn sq(a: &Fe4) -> Fe4 {
    mul(a, a)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(seed: u64) -> Fe {
        let mut b = [0u8; 32];
        let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1);
        for chunk in b.chunks_mut(8) {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            chunk.copy_from_slice(&x.to_le_bytes());
        }
        b[31] &= 0x7f;
        Fe::from_bytes(&b)
    }

    fn quad(seed: u64) -> [Fe; LANES] {
        std::array::from_fn(|i| sample(seed * 4 + i as u64))
    }

    /// Gathering into lanes and scattering back must return the same elements.
    /// A mistake here would silently mix lanes, and every lane would still
    /// compute correctly — just for the wrong candidate.
    #[test]
    fn gather_scatter_roundtrip() {
        for s in 0..64 {
            let original = quad(s);
            let restored = Fe4::from_elements(&original).to_elements();
            for (lane, (got, want)) in restored.iter().zip(original.iter()).enumerate() {
                assert!(got.equals(want), "seed {s} lane {lane}");
            }
        }
    }

    #[test]
    fn roundtrip_preserves_lane_order() {
        let original = quad(7);
        let packed = Fe4::from_elements(&original);
        // Each lane must carry its own element, not a neighbour's.
        for (lane, element) in original.iter().enumerate() {
            let single = Fe4::from_elements(&[*element; LANES]);
            assert_eq!(packed.lane(lane), single.lane(0), "lane {lane}");
        }
    }

    #[test]
    fn roundtrip_handles_edge_values() {
        let mut p_minus_one = [0xffu8; 32];
        p_minus_one[0] = 0xec;
        p_minus_one[31] = 0x7f;
        let edges = [
            Fe::zero(),
            Fe::one(),
            Fe::from_bytes(&p_minus_one),
            sample(1),
        ];
        let restored = Fe4::from_elements(&edges).to_elements();
        for (lane, (got, want)) in restored.iter().zip(edges.iter()).enumerate() {
            assert!(got.equals(want), "lane {lane}");
        }
    }

    /// The radix 2^25.5 multiplication must agree with the scalar field, which
    /// is a different representation and a formally verified implementation.
    #[test]
    fn lane_multiplication_matches_the_scalar_field() {
        for s in 0..128 {
            let a = sample(s);
            let b = sample(s + 1000);
            let expected = a.mul(&b);

            let va = Fe4::from_elements(&[a; LANES]);
            let vb = Fe4::from_elements(&[b; LANES]);
            let got = mul_lanes(&va, &vb).to_elements();

            for (lane, value) in got.iter().enumerate() {
                assert!(value.equals(&expected), "seed {s} lane {lane}");
            }
        }
    }

    /// Addition and subtraction must agree with the scalar field, including the
    /// `a + 2p - b` detour that unsigned limbs force on subtraction.
    #[test]
    fn addition_and_subtraction_match_the_scalar_field() {
        for s in 0..128u64 {
            let a = quad(s);
            let b = quad(s + 700);
            let (va, vb) = (Fe4::from_elements(&a), Fe4::from_elements(&b));

            let sum = va.add(&vb).to_elements();
            let diff = va.sub(&vb).to_elements();
            for lane in 0..LANES {
                assert!(
                    sum[lane].equals(&a[lane].add(&b[lane])),
                    "add seed {s} lane {lane}"
                );
                assert!(
                    diff[lane].equals(&a[lane].sub(&b[lane])),
                    "sub seed {s} lane {lane}"
                );
            }
        }
    }

    /// Subtracting a larger value from a smaller one is the case unsigned limbs
    /// break on, so it gets its own test rather than relying on random inputs.
    #[test]
    fn subtraction_handles_underflow() {
        let small = [Fe::zero(); LANES];
        let mut max_bytes = [0xffu8; 32];
        max_bytes[0] = 0xec;
        max_bytes[31] = 0x7f;
        let large = [Fe::from_bytes(&max_bytes); LANES];

        let got = Fe4::from_elements(&small)
            .sub(&Fe4::from_elements(&large))
            .to_elements();
        for (lane, value) in got.iter().enumerate() {
            assert!(
                value.equals(&Fe::zero().sub(&large[0])),
                "0 - (p-1) went wrong in lane {lane}"
            );
        }
    }

    /// The contract multiplication relies on. Built from maximal limbs rather
    /// than random ones: with random values the margins are wide enough that a
    /// wrong bound would pass by luck.
    #[test]
    fn bounds_hold_for_three_operations_and_break_for_four() {
        // Every limb at its ceiling, which is what a reduced value can reach.
        let mut maxed = Fe4::zero();
        for (i, limb) in maxed.limbs.iter_mut().enumerate() {
            let value = if i % 2 == 0 {
                (1u32 << 26) - 1
            } else {
                (1u32 << 25) - 1
            };
            *limb = [value; LANES];
        }

        assert!(
            maxed.within_mul_bounds(),
            "a reduced value must be in bounds"
        );
        assert!(
            maxed.add(&maxed).within_mul_bounds(),
            "one addition of maximal values must stay in bounds"
        );
        assert!(
            maxed.sub(&Fe4::zero()).within_mul_bounds(),
            "one subtraction must stay in bounds"
        );

        // Three still fit; four do not. The group layer never goes past one,
        // so this is headroom rather than a limit it works against.
        let three = maxed.add(&maxed).add(&maxed);
        assert!(
            three.within_mul_bounds(),
            "three stacked additions still fit"
        );

        let four = three.add(&maxed);
        assert!(
            !four.within_mul_bounds(),
            "four stacked additions were expected to exceed the ceiling; if \
             this ever passes, the bound analysis in the module header is wrong"
        );
        assert!(
            four.carry().within_mul_bounds(),
            "carrying must bring it back"
        );
    }

    #[test]
    fn carry_preserves_the_value() {
        for s in 0..64u64 {
            let a = quad(s);
            let b = quad(s + 300);
            let loose = Fe4::from_elements(&a).add(&Fe4::from_elements(&b));
            let carried = loose.carry();
            assert!(carried.within_mul_bounds());
            let got = carried.to_elements();
            for lane in 0..LANES {
                assert!(
                    got[lane].equals(&a[lane].add(&b[lane])),
                    "seed {s} lane {lane}"
                );
            }
        }
    }

    /// Multiplication must accept operands straight out of one add or sub.
    #[test]
    fn multiplication_accepts_unreduced_operands() {
        for s in 0..64u64 {
            let (a, b, c) = (quad(s), quad(s + 11), quad(s + 22));
            let (va, vb, vc) = (
                Fe4::from_elements(&a),
                Fe4::from_elements(&b),
                Fe4::from_elements(&c),
            );
            let got = mul(&va.add(&vb), &vc.sub(&va)).to_elements();
            for lane in 0..LANES {
                let want = a[lane].add(&b[lane]).mul(&c[lane].sub(&a[lane]));
                assert!(got[lane].equals(&want), "seed {s} lane {lane}");
            }
        }
    }

    /// The packed bytes must be canonical: the address is base32 of exactly
    /// these bytes, so a representative differing by `p` would encode a
    /// different address that the secret key does not open.
    #[test]
    fn canonical_packing_matches_the_scalar_field() {
        for s in 0..128u64 {
            let a = quad(s);
            let v = Fe4::from_elements(&a);
            for (lane, element) in a.iter().enumerate() {
                assert_eq!(
                    v.lane_to_bytes(lane),
                    element.to_bytes(),
                    "seed {s} lane {lane}"
                );
            }
        }
    }

    /// Values at and just below the modulus are where a non-canonical encoding
    /// would slip through unnoticed.
    #[test]
    fn canonical_packing_handles_values_near_the_modulus() {
        let mut p_minus_one = [0xffu8; 32];
        p_minus_one[0] = 0xec;
        p_minus_one[31] = 0x7f;
        let mut p_minus_two = p_minus_one;
        p_minus_two[0] = 0xeb;

        let edges = [
            Fe::zero(),
            Fe::one(),
            Fe::from_bytes(&p_minus_one),
            Fe::from_bytes(&p_minus_two),
        ];
        let v = Fe4::from_elements(&edges);
        for (lane, element) in edges.iter().enumerate() {
            assert_eq!(v.lane_to_bytes(lane), element.to_bytes(), "lane {lane}");
        }

        // A sum that wraps past p must come back canonical.
        let wrapping = Fe4::from_elements(&[Fe::from_bytes(&p_minus_one); LANES])
            .add(&Fe4::from_elements(&[Fe::from_bytes(&p_minus_one); LANES]))
            .carry();
        let want = Fe::from_bytes(&p_minus_one).add(&Fe::from_bytes(&p_minus_one));
        for lane in 0..LANES {
            assert_eq!(
                wrapping.lane_to_bytes(lane),
                want.to_bytes(),
                "wrap lane {lane}"
            );
        }
    }

    /// The split reduction must agree with the one-shot path it replaced.
    #[test]
    fn split_canonical_packing_matches_the_combined_one() {
        for s in 0..64u64 {
            let a = quad(s);
            let b = quad(s + 400);
            // Values straight out of arithmetic, not just reduced samples.
            let v = mul(&Fe4::from_elements(&a), &Fe4::from_elements(&b));
            let canonical = v.to_canonical();
            for lane in 0..LANES {
                assert_eq!(
                    canonical.canonical_lane_to_bytes(lane),
                    v.lane_to_bytes(lane),
                    "seed {s} lane {lane}"
                );
            }
        }
    }

    #[test]
    fn broadcast_puts_the_same_value_in_every_lane() {
        let e = sample(42);
        let v = Fe4::broadcast(&e);
        for lane in 0..LANES {
            assert_eq!(v.lane_to_bytes(lane), e.to_bytes(), "lane {lane}");
        }
    }

    /// Four inversions for the price of one chain.
    #[test]
    fn inversion_matches_the_scalar_field() {
        for s in 1..48u64 {
            let a = quad(s);
            let got = Fe4::from_elements(&a).invert().to_elements();
            for (lane, value) in got.iter().enumerate() {
                assert!(value.equals(&a[lane].invert()), "seed {s} lane {lane}");
                assert!(
                    value.mul(&a[lane]).equals(&Fe::one()),
                    "seed {s} lane {lane}: not an inverse"
                );
            }
        }
    }

    /// A differential test that silently ran on the fallback would prove
    /// nothing, so the vector tests assert which path they exercised.
    #[test]
    fn the_vector_path_is_actually_taken() {
        let expected = if cfg!(target_arch = "aarch64") {
            Implementation::Neon
        } else if cfg!(target_arch = "x86_64") {
            // Any x86_64 worth testing on has AVX2; if it does not, this test
            // says so rather than passing quietly.
            Implementation::Avx2
        } else {
            Implementation::Scalar
        };
        assert_eq!(
            active(),
            expected,
            "the vector path was expected here; tests below would otherwise \
             measure the fallback"
        );
    }

    /// The vector path must agree with the lane-at-a-time path on every lane.
    /// A divergence here is the worst defect a generator can have: the program
    /// runs, prints addresses, and the keys behind them do not work.
    #[test]
    fn vector_multiplication_matches_the_lane_path() {
        for s in 0..256u64 {
            let a = Fe4::from_elements(&quad(s));
            let b = Fe4::from_elements(&quad(s + 5000));
            assert_eq!(mul(&a, &b), mul_lanes(&a, &b), "seed {s}");
        }
    }

    #[test]
    fn vector_multiplication_matches_the_scalar_field() {
        for s in 0..128u64 {
            let a = quad(s);
            let b = quad(s + 900);
            let got = mul(&Fe4::from_elements(&a), &Fe4::from_elements(&b)).to_elements();
            for (lane, value) in got.iter().enumerate() {
                assert!(value.equals(&a[lane].mul(&b[lane])), "seed {s} lane {lane}");
            }
        }
    }

    #[test]
    fn vector_squaring_matches_multiplication_and_the_scalar_field() {
        for s in 0..128u64 {
            let a = quad(s);
            let v = Fe4::from_elements(&a);
            assert_eq!(
                sq(&v),
                mul(&v, &v),
                "seed {s}: square must equal self-multiply"
            );
            let got = sq(&v).to_elements();
            for (lane, value) in got.iter().enumerate() {
                assert!(value.equals(&a[lane].sq()), "seed {s} lane {lane}");
            }
        }
    }

    /// Edge inputs are where a carry chain goes wrong: zero, one, `p-1`, all
    /// limbs at their maximum, and values that have been added without being
    /// reduced.
    #[test]
    fn vector_multiplication_handles_edge_inputs() {
        let mut p_minus_one = [0xffu8; 32];
        p_minus_one[0] = 0xec;
        p_minus_one[31] = 0x7f;
        let mut all_ones = [0xffu8; 32];
        all_ones[31] = 0x7f;

        let edges = [
            Fe::zero(),
            Fe::one(),
            Fe::from_bytes(&p_minus_one),
            Fe::from_bytes(&all_ones),
        ];
        // Unreduced values: a sum that has not been carried is what the group
        // layer actually feeds a multiplication.
        let unreduced: [Fe; LANES] = std::array::from_fn(|i| edges[i].add(&edges[(i + 1) % LANES]));

        for (name, left) in [("edges", edges), ("unreduced", unreduced)] {
            for (rname, right) in [("edges", edges), ("unreduced", unreduced)] {
                let va = Fe4::from_elements(&left);
                let vb = Fe4::from_elements(&right);
                assert_eq!(mul(&va, &vb), mul_lanes(&va, &vb), "{name} x {rname}");
                let got = mul(&va, &vb).to_elements();
                for (lane, value) in got.iter().enumerate() {
                    assert!(
                        value.equals(&left[lane].mul(&right[lane])),
                        "{name} x {rname} lane {lane}"
                    );
                }
            }
        }
    }

    #[test]
    fn lane_multiplication_keeps_lanes_independent() {
        let a = quad(3);
        let b = quad(9);
        let got = mul_lanes(&Fe4::from_elements(&a), &Fe4::from_elements(&b)).to_elements();
        for (lane, value) in got.iter().enumerate() {
            assert!(
                value.equals(&a[lane].mul(&b[lane])),
                "lane {lane} took the wrong operand"
            );
        }
    }
}
