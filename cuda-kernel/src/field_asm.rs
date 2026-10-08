//! Field operations for the device, radix 2^32 in eight 32-bit limbs.
//!
//! Every index is a literal. That is not a matter of taste: an array the
//! kernel indexes with a loop variable cannot live in registers, and the whole
//! working set lands in local memory, which is off chip. Written out, the same
//! arithmetic spills nothing.

use super::field::Fe;

/// The modulus, least significant limb first.
const P: Fe = [
    0xffff_ffed,
    0xffff_ffff,
    0xffff_ffff,
    0xffff_ffff,
    0xffff_ffff,
    0xffff_ffff,
    0xffff_ffff,
    0x7fff_ffff,
];

#[inline(always)]
pub unsafe fn mul(f: &Fe, g: &Fe) -> Fe {
    let mut p0: u32 = 0;
    let mut p1: u32 = 0;
    let mut p2: u32 = 0;
    let mut p3: u32 = 0;
    let mut p4: u32 = 0;
    let mut p5: u32 = 0;
    let mut p6: u32 = 0;
    let mut p7: u32 = 0;
    let mut p8: u32 = 0;
    let mut p9: u32 = 0;
    let mut p10: u32 = 0;
    let mut p11: u32 = 0;
    let mut p12: u32 = 0;
    let mut p13: u32 = 0;
    let mut p14: u32 = 0;
    let mut p15: u32 = 0;
    let mut t: u64;
    let mut cy: u32;
    cy = 0;
    t = f[0] as u64 * g[0] as u64 + p0 as u64 + cy as u64; p0 = t as u32; cy = (t >> 32) as u32;
    t = f[0] as u64 * g[1] as u64 + p1 as u64 + cy as u64; p1 = t as u32; cy = (t >> 32) as u32;
    t = f[0] as u64 * g[2] as u64 + p2 as u64 + cy as u64; p2 = t as u32; cy = (t >> 32) as u32;
    t = f[0] as u64 * g[3] as u64 + p3 as u64 + cy as u64; p3 = t as u32; cy = (t >> 32) as u32;
    t = f[0] as u64 * g[4] as u64 + p4 as u64 + cy as u64; p4 = t as u32; cy = (t >> 32) as u32;
    t = f[0] as u64 * g[5] as u64 + p5 as u64 + cy as u64; p5 = t as u32; cy = (t >> 32) as u32;
    t = f[0] as u64 * g[6] as u64 + p6 as u64 + cy as u64; p6 = t as u32; cy = (t >> 32) as u32;
    t = f[0] as u64 * g[7] as u64 + p7 as u64 + cy as u64; p7 = t as u32; cy = (t >> 32) as u32;
    p8 = cy;
    cy = 0;
    t = f[1] as u64 * g[0] as u64 + p1 as u64 + cy as u64; p1 = t as u32; cy = (t >> 32) as u32;
    t = f[1] as u64 * g[1] as u64 + p2 as u64 + cy as u64; p2 = t as u32; cy = (t >> 32) as u32;
    t = f[1] as u64 * g[2] as u64 + p3 as u64 + cy as u64; p3 = t as u32; cy = (t >> 32) as u32;
    t = f[1] as u64 * g[3] as u64 + p4 as u64 + cy as u64; p4 = t as u32; cy = (t >> 32) as u32;
    t = f[1] as u64 * g[4] as u64 + p5 as u64 + cy as u64; p5 = t as u32; cy = (t >> 32) as u32;
    t = f[1] as u64 * g[5] as u64 + p6 as u64 + cy as u64; p6 = t as u32; cy = (t >> 32) as u32;
    t = f[1] as u64 * g[6] as u64 + p7 as u64 + cy as u64; p7 = t as u32; cy = (t >> 32) as u32;
    t = f[1] as u64 * g[7] as u64 + p8 as u64 + cy as u64; p8 = t as u32; cy = (t >> 32) as u32;
    p9 = cy;
    cy = 0;
    t = f[2] as u64 * g[0] as u64 + p2 as u64 + cy as u64; p2 = t as u32; cy = (t >> 32) as u32;
    t = f[2] as u64 * g[1] as u64 + p3 as u64 + cy as u64; p3 = t as u32; cy = (t >> 32) as u32;
    t = f[2] as u64 * g[2] as u64 + p4 as u64 + cy as u64; p4 = t as u32; cy = (t >> 32) as u32;
    t = f[2] as u64 * g[3] as u64 + p5 as u64 + cy as u64; p5 = t as u32; cy = (t >> 32) as u32;
    t = f[2] as u64 * g[4] as u64 + p6 as u64 + cy as u64; p6 = t as u32; cy = (t >> 32) as u32;
    t = f[2] as u64 * g[5] as u64 + p7 as u64 + cy as u64; p7 = t as u32; cy = (t >> 32) as u32;
    t = f[2] as u64 * g[6] as u64 + p8 as u64 + cy as u64; p8 = t as u32; cy = (t >> 32) as u32;
    t = f[2] as u64 * g[7] as u64 + p9 as u64 + cy as u64; p9 = t as u32; cy = (t >> 32) as u32;
    p10 = cy;
    cy = 0;
    t = f[3] as u64 * g[0] as u64 + p3 as u64 + cy as u64; p3 = t as u32; cy = (t >> 32) as u32;
    t = f[3] as u64 * g[1] as u64 + p4 as u64 + cy as u64; p4 = t as u32; cy = (t >> 32) as u32;
    t = f[3] as u64 * g[2] as u64 + p5 as u64 + cy as u64; p5 = t as u32; cy = (t >> 32) as u32;
    t = f[3] as u64 * g[3] as u64 + p6 as u64 + cy as u64; p6 = t as u32; cy = (t >> 32) as u32;
    t = f[3] as u64 * g[4] as u64 + p7 as u64 + cy as u64; p7 = t as u32; cy = (t >> 32) as u32;
    t = f[3] as u64 * g[5] as u64 + p8 as u64 + cy as u64; p8 = t as u32; cy = (t >> 32) as u32;
    t = f[3] as u64 * g[6] as u64 + p9 as u64 + cy as u64; p9 = t as u32; cy = (t >> 32) as u32;
    t = f[3] as u64 * g[7] as u64 + p10 as u64 + cy as u64; p10 = t as u32; cy = (t >> 32) as u32;
    p11 = cy;
    cy = 0;
    t = f[4] as u64 * g[0] as u64 + p4 as u64 + cy as u64; p4 = t as u32; cy = (t >> 32) as u32;
    t = f[4] as u64 * g[1] as u64 + p5 as u64 + cy as u64; p5 = t as u32; cy = (t >> 32) as u32;
    t = f[4] as u64 * g[2] as u64 + p6 as u64 + cy as u64; p6 = t as u32; cy = (t >> 32) as u32;
    t = f[4] as u64 * g[3] as u64 + p7 as u64 + cy as u64; p7 = t as u32; cy = (t >> 32) as u32;
    t = f[4] as u64 * g[4] as u64 + p8 as u64 + cy as u64; p8 = t as u32; cy = (t >> 32) as u32;
    t = f[4] as u64 * g[5] as u64 + p9 as u64 + cy as u64; p9 = t as u32; cy = (t >> 32) as u32;
    t = f[4] as u64 * g[6] as u64 + p10 as u64 + cy as u64; p10 = t as u32; cy = (t >> 32) as u32;
    t = f[4] as u64 * g[7] as u64 + p11 as u64 + cy as u64; p11 = t as u32; cy = (t >> 32) as u32;
    p12 = cy;
    cy = 0;
    t = f[5] as u64 * g[0] as u64 + p5 as u64 + cy as u64; p5 = t as u32; cy = (t >> 32) as u32;
    t = f[5] as u64 * g[1] as u64 + p6 as u64 + cy as u64; p6 = t as u32; cy = (t >> 32) as u32;
    t = f[5] as u64 * g[2] as u64 + p7 as u64 + cy as u64; p7 = t as u32; cy = (t >> 32) as u32;
    t = f[5] as u64 * g[3] as u64 + p8 as u64 + cy as u64; p8 = t as u32; cy = (t >> 32) as u32;
    t = f[5] as u64 * g[4] as u64 + p9 as u64 + cy as u64; p9 = t as u32; cy = (t >> 32) as u32;
    t = f[5] as u64 * g[5] as u64 + p10 as u64 + cy as u64; p10 = t as u32; cy = (t >> 32) as u32;
    t = f[5] as u64 * g[6] as u64 + p11 as u64 + cy as u64; p11 = t as u32; cy = (t >> 32) as u32;
    t = f[5] as u64 * g[7] as u64 + p12 as u64 + cy as u64; p12 = t as u32; cy = (t >> 32) as u32;
    p13 = cy;
    cy = 0;
    t = f[6] as u64 * g[0] as u64 + p6 as u64 + cy as u64; p6 = t as u32; cy = (t >> 32) as u32;
    t = f[6] as u64 * g[1] as u64 + p7 as u64 + cy as u64; p7 = t as u32; cy = (t >> 32) as u32;
    t = f[6] as u64 * g[2] as u64 + p8 as u64 + cy as u64; p8 = t as u32; cy = (t >> 32) as u32;
    t = f[6] as u64 * g[3] as u64 + p9 as u64 + cy as u64; p9 = t as u32; cy = (t >> 32) as u32;
    t = f[6] as u64 * g[4] as u64 + p10 as u64 + cy as u64; p10 = t as u32; cy = (t >> 32) as u32;
    t = f[6] as u64 * g[5] as u64 + p11 as u64 + cy as u64; p11 = t as u32; cy = (t >> 32) as u32;
    t = f[6] as u64 * g[6] as u64 + p12 as u64 + cy as u64; p12 = t as u32; cy = (t >> 32) as u32;
    t = f[6] as u64 * g[7] as u64 + p13 as u64 + cy as u64; p13 = t as u32; cy = (t >> 32) as u32;
    p14 = cy;
    cy = 0;
    t = f[7] as u64 * g[0] as u64 + p7 as u64 + cy as u64; p7 = t as u32; cy = (t >> 32) as u32;
    t = f[7] as u64 * g[1] as u64 + p8 as u64 + cy as u64; p8 = t as u32; cy = (t >> 32) as u32;
    t = f[7] as u64 * g[2] as u64 + p9 as u64 + cy as u64; p9 = t as u32; cy = (t >> 32) as u32;
    t = f[7] as u64 * g[3] as u64 + p10 as u64 + cy as u64; p10 = t as u32; cy = (t >> 32) as u32;
    t = f[7] as u64 * g[4] as u64 + p11 as u64 + cy as u64; p11 = t as u32; cy = (t >> 32) as u32;
    t = f[7] as u64 * g[5] as u64 + p12 as u64 + cy as u64; p12 = t as u32; cy = (t >> 32) as u32;
    t = f[7] as u64 * g[6] as u64 + p13 as u64 + cy as u64; p13 = t as u32; cy = (t >> 32) as u32;
    t = f[7] as u64 * g[7] as u64 + p14 as u64 + cy as u64; p14 = t as u32; cy = (t >> 32) as u32;
    p15 = cy;
    // 2^256 = 38 (mod p): the top half folds back into the bottom.
    let mut c: u64 = 0;
    t = p0 as u64 + 38u64 * p8 as u64 + c; p0 = t as u32; c = t >> 32;
    t = p1 as u64 + 38u64 * p9 as u64 + c; p1 = t as u32; c = t >> 32;
    t = p2 as u64 + 38u64 * p10 as u64 + c; p2 = t as u32; c = t >> 32;
    t = p3 as u64 + 38u64 * p11 as u64 + c; p3 = t as u32; c = t >> 32;
    t = p4 as u64 + 38u64 * p12 as u64 + c; p4 = t as u32; c = t >> 32;
    t = p5 as u64 + 38u64 * p13 as u64 + c; p5 = t as u32; c = t >> 32;
    t = p6 as u64 + 38u64 * p14 as u64 + c; p6 = t as u32; c = t >> 32;
    t = p7 as u64 + 38u64 * p15 as u64 + c; p7 = t as u32; c = t >> 32;
    let mut v = (c as u32).wrapping_mul(38);
    let mut round = 0;
    while round < 2 {
        t = p0 as u64 + v as u64; p0 = t as u32; v = (t >> 32) as u32;
        t = p1 as u64 + v as u64; p1 = t as u32; v = (t >> 32) as u32;
        t = p2 as u64 + v as u64; p2 = t as u32; v = (t >> 32) as u32;
        t = p3 as u64 + v as u64; p3 = t as u32; v = (t >> 32) as u32;
        t = p4 as u64 + v as u64; p4 = t as u32; v = (t >> 32) as u32;
        t = p5 as u64 + v as u64; p5 = t as u32; v = (t >> 32) as u32;
        t = p6 as u64 + v as u64; p6 = t as u32; v = (t >> 32) as u32;
        t = p7 as u64 + v as u64; p7 = t as u32; v = (t >> 32) as u32;
        v = v.wrapping_mul(38);
        round += 1;
    }
    [p0, p1, p2, p3, p4, p5, p6, p7]
}

#[inline(always)]
pub unsafe fn sq(f: &Fe) -> Fe {
    mul(f, f)
}

#[inline(always)]
/// `a + b`, folding a carry out of the top back in with 38.
pub fn add(f: &Fe, g: &Fe) -> Fe {
    let mut r0: u32;
    let mut r1: u32;
    let mut r2: u32;
    let mut r3: u32;
    let mut r4: u32;
    let mut r5: u32;
    let mut r6: u32;
    let mut r7: u32;
    let mut t: u64;
    let mut b: u32 = 0;
    t = f[0] as u64 + g[0] as u64 + b as u64; r0 = t as u32; b = (t >> 32) as u32;
    t = f[1] as u64 + g[1] as u64 + b as u64; r1 = t as u32; b = (t >> 32) as u32;
    t = f[2] as u64 + g[2] as u64 + b as u64; r2 = t as u32; b = (t >> 32) as u32;
    t = f[3] as u64 + g[3] as u64 + b as u64; r3 = t as u32; b = (t >> 32) as u32;
    t = f[4] as u64 + g[4] as u64 + b as u64; r4 = t as u32; b = (t >> 32) as u32;
    t = f[5] as u64 + g[5] as u64 + b as u64; r5 = t as u32; b = (t >> 32) as u32;
    t = f[6] as u64 + g[6] as u64 + b as u64; r6 = t as u32; b = (t >> 32) as u32;
    t = f[7] as u64 + g[7] as u64 + b as u64; r7 = t as u32; b = (t >> 32) as u32;
    let mut v = b.wrapping_mul(38);
    let mut round = 0;
    while round < 2 {
        t = r0 as u64 + v as u64; r0 = t as u32; v = (t >> 32) as u32;
        t = r1 as u64 + v as u64; r1 = t as u32; v = (t >> 32) as u32;
        t = r2 as u64 + v as u64; r2 = t as u32; v = (t >> 32) as u32;
        t = r3 as u64 + v as u64; r3 = t as u32; v = (t >> 32) as u32;
        t = r4 as u64 + v as u64; r4 = t as u32; v = (t >> 32) as u32;
        t = r5 as u64 + v as u64; r5 = t as u32; v = (t >> 32) as u32;
        t = r6 as u64 + v as u64; r6 = t as u32; v = (t >> 32) as u32;
        t = r7 as u64 + v as u64; r7 = t as u32; v = (t >> 32) as u32;
        v = v.wrapping_mul(38);
        round += 1;
    }
    [r0, r1, r2, r3, r4, r5, r6, r7]
}

#[inline(always)]
/// `a - b`. A borrow out of the top means the result ran below zero, and
/// `2^256 - 38` represents zero, so taking 38 away corrects it.
pub fn sub(f: &Fe, g: &Fe) -> Fe {
    let mut r0: u32;
    let mut r1: u32;
    let mut r2: u32;
    let mut r3: u32;
    let mut r4: u32;
    let mut r5: u32;
    let mut r6: u32;
    let mut r7: u32;
    let mut t: u64;
    let mut b: u32 = 0;
    t = (f[0] as u64).wrapping_sub(g[0] as u64).wrapping_sub(b as u64); r0 = t as u32; b = ((t >> 32) & 1) as u32;
    t = (f[1] as u64).wrapping_sub(g[1] as u64).wrapping_sub(b as u64); r1 = t as u32; b = ((t >> 32) & 1) as u32;
    t = (f[2] as u64).wrapping_sub(g[2] as u64).wrapping_sub(b as u64); r2 = t as u32; b = ((t >> 32) & 1) as u32;
    t = (f[3] as u64).wrapping_sub(g[3] as u64).wrapping_sub(b as u64); r3 = t as u32; b = ((t >> 32) & 1) as u32;
    t = (f[4] as u64).wrapping_sub(g[4] as u64).wrapping_sub(b as u64); r4 = t as u32; b = ((t >> 32) & 1) as u32;
    t = (f[5] as u64).wrapping_sub(g[5] as u64).wrapping_sub(b as u64); r5 = t as u32; b = ((t >> 32) & 1) as u32;
    t = (f[6] as u64).wrapping_sub(g[6] as u64).wrapping_sub(b as u64); r6 = t as u32; b = ((t >> 32) & 1) as u32;
    t = (f[7] as u64).wrapping_sub(g[7] as u64).wrapping_sub(b as u64); r7 = t as u32; b = ((t >> 32) & 1) as u32;
    let mut v = b.wrapping_mul(38);
    let mut round = 0;
    while round < 2 {
        t = (r0 as u64).wrapping_sub(v as u64); r0 = t as u32; v = ((t >> 32) & 1) as u32;
        t = (r1 as u64).wrapping_sub(v as u64); r1 = t as u32; v = ((t >> 32) & 1) as u32;
        t = (r2 as u64).wrapping_sub(v as u64); r2 = t as u32; v = ((t >> 32) & 1) as u32;
        t = (r3 as u64).wrapping_sub(v as u64); r3 = t as u32; v = ((t >> 32) & 1) as u32;
        t = (r4 as u64).wrapping_sub(v as u64); r4 = t as u32; v = ((t >> 32) & 1) as u32;
        t = (r5 as u64).wrapping_sub(v as u64); r5 = t as u32; v = ((t >> 32) & 1) as u32;
        t = (r6 as u64).wrapping_sub(v as u64); r6 = t as u32; v = ((t >> 32) & 1) as u32;
        t = (r7 as u64).wrapping_sub(v as u64); r7 = t as u32; v = ((t >> 32) & 1) as u32;
        v = v.wrapping_mul(38);
        round += 1;
    }
    [r0, r1, r2, r3, r4, r5, r6, r7]
}

/// Saturated limbs are always normalised, so there is nothing to carry.
#[inline(always)]
pub fn reduce(f: &Fe) -> Fe {
    *f
}

/// `f^(p-2)`, which is `f^-1` for a non-zero `f`.
pub unsafe fn invert(f: &Fe) -> Fe {
    let z1 = *f;
    let z2 = sq(&z1);
    let z8 = sq(&sq(&z2));
    let z9 = mul(&z1, &z8);
    let z11 = mul(&z2, &z9);
    let z22 = sq(&z11);
    let z5 = mul(&z9, &z22);

    let mut t = sq(&z5);
    let mut i = 1;
    while i < 5 { t = sq(&t); i += 1; }
    let z10 = mul(&t, &z5);

    t = sq(&z10); i = 1;
    while i < 10 { t = sq(&t); i += 1; }
    let z20 = mul(&t, &z10);

    t = sq(&z20); i = 1;
    while i < 20 { t = sq(&t); i += 1; }
    let z40 = mul(&t, &z20);

    t = sq(&z40); i = 1;
    while i < 10 { t = sq(&t); i += 1; }
    let z50 = mul(&t, &z10);

    t = sq(&z50); i = 1;
    while i < 50 { t = sq(&t); i += 1; }
    let z100 = mul(&t, &z50);

    t = sq(&z100); i = 1;
    while i < 100 { t = sq(&t); i += 1; }
    let z200 = mul(&t, &z100);

    t = sq(&z200); i = 1;
    while i < 50 { t = sq(&t); i += 1; }
    let z250 = mul(&t, &z50);

    t = sq(&z250); i = 1;
    while i < 5 { t = sq(&t); i += 1; }
    mul(&t, &z11)
}

/// The canonical encoding as eight little-endian words.
///
/// A value below `2^256` is below `2p + 38`, so two conditional subtractions
/// always suffice; a third costs eight words and removes the need to argue.
pub fn pack(f: &Fe) -> [u32; 8] {
    let mut r0 = f[0];
    let mut r1 = f[1];
    let mut r2 = f[2];
    let mut r3 = f[3];
    let mut r4 = f[4];
    let mut r5 = f[5];
    let mut r6 = f[6];
    let mut r7 = f[7];
    let mut t: u64;
    let mut b: u32;
    let mut round = 0;
    while round < 3 {
        b = 0;
        t = (r0 as u64).wrapping_sub(P[0] as u64).wrapping_sub(b as u64); let s0 = t as u32; b = ((t >> 32) & 1) as u32;
        t = (r1 as u64).wrapping_sub(P[1] as u64).wrapping_sub(b as u64); let s1 = t as u32; b = ((t >> 32) & 1) as u32;
        t = (r2 as u64).wrapping_sub(P[2] as u64).wrapping_sub(b as u64); let s2 = t as u32; b = ((t >> 32) & 1) as u32;
        t = (r3 as u64).wrapping_sub(P[3] as u64).wrapping_sub(b as u64); let s3 = t as u32; b = ((t >> 32) & 1) as u32;
        t = (r4 as u64).wrapping_sub(P[4] as u64).wrapping_sub(b as u64); let s4 = t as u32; b = ((t >> 32) & 1) as u32;
        t = (r5 as u64).wrapping_sub(P[5] as u64).wrapping_sub(b as u64); let s5 = t as u32; b = ((t >> 32) & 1) as u32;
        t = (r6 as u64).wrapping_sub(P[6] as u64).wrapping_sub(b as u64); let s6 = t as u32; b = ((t >> 32) & 1) as u32;
        t = (r7 as u64).wrapping_sub(P[7] as u64).wrapping_sub(b as u64); let s7 = t as u32; b = ((t >> 32) & 1) as u32;
        if b == 0 {
            r0 = s0;
            r1 = s1;
            r2 = s2;
            r3 = s3;
            r4 = s4;
            r5 = s5;
            r6 = s6;
            r7 = s7;
        }
        round += 1;
    }
    [r0, r1, r2, r3, r4, r5, r6, r7]
}
