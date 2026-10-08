//! ed25519 arithmetic in extended (projective) coordinates.
//!
//! The group layer is hand-written for the reason given in D1: public library
//! APIs do not expose projective coordinates, without which batch inversion
//! is impossible.

use crate::field::{Fe, FeLoose};

const fn hex_val(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        _ => 0,
    }
}

const fn hex32(s: &str) -> [u8; 32] {
    let b = s.as_bytes();
    let mut out = [0u8; 32];
    let mut i = 0;
    while i < 32 {
        out[i] = hex_val(b[i * 2]) * 16 + hex_val(b[i * 2 + 1]);
        i += 1;
    }
    out
}

const BASEPOINT_X: [u8; 32] =
    hex32("1ad5258f602d56c9b2a7259560c72c695cdcd6fd31e2a4c0fe536ecdd3366921");
const BASEPOINT_Y: [u8; 32] =
    hex32("5866666666666666666666666666666666666666666666666666666666666666");
const EIGHT_YPLUSX: [u8; 32] =
    hex32("8f3edd046659b7592c7088e27703b36c23c3d95e669c33b12fe5bc6160e71509");
const EIGHT_YMINUSX: [u8; 32] =
    hex32("d93492f3ed5da7e2f958b5e180763d96fb233c6eac41272cc3010e32a124903a");
const EIGHT_Z: [u8; 32] = hex32("0100000000000000000000000000000000000000000000000000000000000000");
const EIGHT_T2D: [u8; 32] =
    hex32("1a91a2c9d9f5c1e7d7a7cc8b7871a3b8322ab60e19126463954ecc2e5c7c9026");
const TWO_D: [u8; 32] = hex32("59f1b226949bd6eb56b183829a14e00030d1f3eef2808e19e7fcdf56dcd90624");

/// A point in extended coordinates: `x = X/Z`, `y = Y/Z`, `T = XY/Z`.
#[derive(Clone, Copy)]
pub struct Point {
    pub x: Fe,
    pub y: Fe,
    pub z: Fe,
    pub t: Fe,
}

/// A precomputed addend: the form in which a point is cheaper to add.
///
/// Its coordinates are kept loose because every use of them is a
/// multiplication; reducing them once at construction would only have to be
/// undone on the first use.
#[derive(Clone, Copy)]
pub struct Cached {
    pub y_plus_x: FeLoose,
    pub y_minus_x: FeLoose,
    pub z: FeLoose,
    pub t2d: FeLoose,
}

/// The intermediate form produced by addition.
///
/// Loose as well: `to_p3` consumes all four coordinates through
/// multiplications, so carrying them here would be wasted work.
#[derive(Clone, Copy)]
pub struct P1P1 {
    pub x: FeLoose,
    pub y: FeLoose,
    pub z: FeLoose,
    pub t: FeLoose,
}

impl Point {
    pub fn identity() -> Self {
        Point {
            x: Fe::zero(),
            y: Fe::one(),
            z: Fe::one(),
            t: Fe::zero(),
        }
    }

    /// The curve base point.
    pub fn basepoint() -> Self {
        let x = Fe::from_bytes(&BASEPOINT_X);
        let y = Fe::from_bytes(&BASEPOINT_Y);
        Point {
            x,
            y,
            z: Fe::one(),
            t: x.mul(&y),
        }
    }

    /// Identity check: `x = 0` and `y = 1` after normalization.
    pub fn is_identity(&self) -> bool {
        let zinv = self.z.invert();
        self.x.mul(&zinv).equals(&Fe::zero()) && self.y.mul(&zinv).equals(&Fe::one())
    }

    /// Affine coordinates — for tests and one-off checks only.
    pub fn to_affine(&self) -> (Fe, Fe) {
        let zinv = self.z.invert();
        (self.x.mul(&zinv), self.y.mul(&zinv))
    }
}

/// `8G` in cached form — the step of the search chain.
pub fn eight_basepoint_cached() -> Cached {
    Cached {
        y_plus_x: Fe::from_bytes(&EIGHT_YPLUSX).relax(),
        y_minus_x: Fe::from_bytes(&EIGHT_YMINUSX).relax(),
        z: Fe::from_bytes(&EIGHT_Z).relax(),
        t2d: Fe::from_bytes(&EIGHT_T2D).relax(),
    }
}

pub fn two_d() -> Fe {
    Fe::from_bytes(&TWO_D)
}

/// `p3 + cached -> p1p1`. Four field multiplications.
///
/// This is the hot loop: it runs once per candidate. Every addition and
/// subtraction here stays unreduced, and the only reductions are the ones
/// `carry_mul` performs anyway, plus the single doubling of `t0`.
#[inline(always)]
pub fn add(p: &Point, q: &Cached) -> P1P1 {
    let a = p.y.add_loose(&p.x);
    let b = p.y.sub_loose(&p.x);
    let zz = a.mul(&q.y_plus_x);
    let yy = b.mul(&q.y_minus_x);
    let tt = q.t2d.mul(&p.t.relax());
    let t0 = p.z.relax().mul(&q.z);
    let t0 = t0.add(&t0);
    P1P1 {
        x: zz.sub_loose(&yy),
        y: zz.add_loose(&yy),
        z: t0.add_loose(&tt),
        t: t0.sub_loose(&tt),
    }
}

/// Doubling. Needed only for the initial scalar multiplication; it is not
/// part of the hot loop.
pub fn dbl(p: &Point) -> P1P1 {
    let xx = p.x.sq();
    let zz = p.y.sq();
    let tt = p.z.sq();
    let tt = tt.add(&tt);
    let yy = p.x.add_loose(&p.y);
    let t0 = yy.sq();
    let y = zz.add(&xx);
    let z = zz.sub(&xx);
    P1P1 {
        x: t0.sub_loose(&y),
        y: y.relax(),
        z: z.relax(),
        t: tt.sub_loose(&z),
    }
}

/// `p1p1 -> p3`. Four field multiplications and no reductions of its own:
/// `carry_mul` takes the loose coordinates directly.
#[inline(always)]
pub fn to_p3(p: &P1P1) -> Point {
    Point {
        x: p.x.mul(&p.t),
        y: p.y.mul(&p.z),
        z: p.z.mul(&p.t),
        t: p.x.mul(&p.y),
    }
}

pub fn to_cached(p: &Point, two_d: &Fe) -> Cached {
    Cached {
        y_plus_x: p.y.add_loose(&p.x),
        y_minus_x: p.y.sub_loose(&p.x),
        z: p.z.relax(),
        t2d: p.t.mul(two_d).relax(),
    }
}

/// Scalar multiplication of the base point by double-and-add. It runs once
/// per chain start, so it is deliberately not optimized.
pub fn scalar_base_mult(scalar: &[u8; 32], base: &Point, two_d: &Fe) -> Point {
    let base_cached = to_cached(base, two_d);
    let mut acc = Point::identity();
    for i in (0..256).rev() {
        acc = to_p3(&dbl(&acc));
        if (scalar[i / 8] >> (i % 8)) & 1 == 1 {
            acc = to_p3(&add(&acc, &base_cached));
        }
    }
    acc
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Subgroup order: `L = 2^252 + 27742317777372353535851937790883648493`.
    const GROUP_ORDER: [u8; 32] = [
        0xed, 0xd3, 0xf5, 0x5c, 0x1a, 0x63, 0x12, 0x58, 0xd6, 0x9c, 0xf7, 0xa2, 0xde, 0xf9, 0xde,
        0x14, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x10,
    ];

    #[test]
    fn doubling_equals_adding_to_itself() {
        let two_d = two_d();
        let mut p = Point::basepoint();
        for step in 0..8 {
            let doubled = to_p3(&dbl(&p));
            let added = to_p3(&add(&p, &to_cached(&p, &two_d)));
            let (dx, dy) = doubled.to_affine();
            let (ax, ay) = added.to_affine();
            assert!(dx.equals(&ax) && dy.equals(&ay), "step {step}");
            p = doubled;
        }
    }

    #[test]
    fn basepoint_has_expected_order() {
        let p = scalar_base_mult(&GROUP_ORDER, &Point::basepoint(), &two_d());
        assert!(p.is_identity(), "L*B must be the identity element");
    }

    #[test]
    fn eight_basepoint_constant_matches_computed() {
        // The hardcoded 8G constant used by the hot loop must match the one
        // computed from the base point, or the whole search chain is shifted.
        let two_d = two_d();
        let b = Point::basepoint();
        let mut eight = to_p3(&dbl(&b));
        eight = to_p3(&dbl(&eight));
        eight = to_p3(&dbl(&eight));
        let computed = to_cached(&eight, &two_d);

        let zinv = computed.z.carry().invert().relax();
        let expected = eight_basepoint_cached();
        let einv = expected.z.carry().invert().relax();
        assert!(computed
            .y_plus_x
            .mul(&zinv)
            .equals(&expected.y_plus_x.mul(&einv)));
        assert!(computed
            .y_minus_x
            .mul(&zinv)
            .equals(&expected.y_minus_x.mul(&einv)));
        assert!(computed.t2d.mul(&zinv).equals(&expected.t2d.mul(&einv)));
    }

    #[test]
    fn identity_is_neutral() {
        let two_d = two_d();
        let b = Point::basepoint();
        let sum = to_p3(&add(&b, &to_cached(&Point::identity(), &two_d)));
        let (sx, sy) = sum.to_affine();
        let (bx, by) = b.to_affine();
        assert!(sx.equals(&bx) && sy.equals(&by));
    }
}
