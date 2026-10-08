//! Group arithmetic on the device, the same formulas the processor path uses.
//!
//! Extended coordinates for the running point, a cached form for the constant
//! step, whose Z has been scaled to one on the host. Seven multiplications per
//! addition: the canonical count is eight, and scaling the step away removes
//! the product of the two Z coordinates.

use super::field_asm as field;
use super::field::Fe;

#[derive(Clone, Copy)]
pub struct Point {
    pub x: Fe,
    pub y: Fe,
    pub z: Fe,
    pub t: Fe,
}

/// Loads one of the three elements of the constant step.
///
/// The step is the same for every thread, so it stays in memory and is read
/// when needed: every thread of a warp asks for the same address, which the
/// cache answers once. Holding it in registers instead cost forty of them per
/// thread and pushed the working set into local memory, which is off-chip —
/// measured at 108 spilling accesses per step.
#[inline(always)]
unsafe fn cached(q: *const u32, which: usize) -> Fe {
    let mut v: Fe = [0; 8];
    let mut k = 0;
    while k < 8 {
        v[k] = *q.add(which * 8 + k);
        k += 1;
    }
    v
}

// `inline(always)` and not `inline`: at the PTX level a call is a real call,
// with its arguments passed through local memory, which is off-chip. Left as a
// hint the backend declined it, and `chain_resume` carried a 360-byte stack
// frame and 268 local accesses for a point that belongs in registers.
#[inline(always)]
pub unsafe fn step(p: &Point, q: *const u32) -> Point {
    let a = field::add(&p.y, &p.x);
    let b = field::sub(&p.y, &p.x);
    let zz = field::mul(&a, &cached(q, 0));
    let yy = field::mul(&b, &cached(q, 1));
    let tt = field::mul(&cached(q, 2), &p.t);
    // The step is held with its Z scaled to one on the host, so this is 2 * Z1
    // rather than 2 * Z1 * Z2: that is the multiply that goes away, seven
    // instead of eight. The carry stays — doubling leaves the value loose, and
    // both of the values built from it below would otherwise exceed what the
    // scaling by 19 inside `mul` can hold.
    let t0 = field::reduce(&field::add(&p.z, &p.z));

    // The intermediate form, folded straight back into extended coordinates.
    let x = field::sub(&zz, &yy);
    let y = field::add(&zz, &yy);
    let z = field::add(&t0, &tt);
    let t = field::sub(&t0, &tt);

    Point {
        x: field::mul(&x, &t),
        y: field::mul(&y, &z),
        z: field::mul(&z, &t),
        t: field::mul(&x, &y),
    }
}
