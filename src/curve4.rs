//! ed25519 group arithmetic over four independent points at a time.
//!
//! The scalar module works one point at a time; this one works on four, with
//! each lane carrying its own chain. The chain itself is strictly sequential
//! (`A -> A+8G -> A+16G`), so the parallelism has to come from running several
//! chains side by side rather than from splitting one.
//!
//! Limb bounds are the thing to keep in view here. Multiplication accepts
//! operands that are one addition or subtraction away from reduced, and the
//! only place two operations stack is the doubling of `t0`, which is why that
//! one is followed by a carry.

use crate::curve::{self, Point};
use crate::field4::{self, Fe4};

/// Four points in extended coordinates, one per lane.
#[derive(Clone, Copy)]
pub struct Point4 {
    pub x: Fe4,
    pub y: Fe4,
    pub z: Fe4,
    pub t: Fe4,
}

/// A precomputed addend, the same for every lane.
///
/// All four chains step by the same constant, so this is built once and
/// broadcast.
#[derive(Clone, Copy)]
pub struct Cached4 {
    pub y_plus_x: Fe4,
    pub y_minus_x: Fe4,
    pub z: Fe4,
    pub t2d: Fe4,
}

/// The intermediate form produced by addition, unreduced.
#[derive(Clone, Copy)]
pub struct P1P14 {
    pub x: Fe4,
    pub y: Fe4,
    pub z: Fe4,
    pub t: Fe4,
}

impl Point4 {
    /// Gathers four scalar points into lanes.
    pub fn from_points(points: &[Point; 4]) -> Self {
        Point4 {
            x: Fe4::from_elements(&std::array::from_fn(|i| points[i].x)),
            y: Fe4::from_elements(&std::array::from_fn(|i| points[i].y)),
            z: Fe4::from_elements(&std::array::from_fn(|i| points[i].z)),
            t: Fe4::from_elements(&std::array::from_fn(|i| points[i].t)),
        }
    }

    /// Scatters back into scalar points. Used by tests and by the one-off work
    /// at the start of a chain, never in the hot loop.
    pub fn to_points(&self) -> [Point; 4] {
        let x = self.x.to_elements();
        let y = self.y.to_elements();
        let z = self.z.to_elements();
        let t = self.t.to_elements();
        std::array::from_fn(|i| Point {
            x: x[i],
            y: y[i],
            z: z[i],
            t: t[i],
        })
    }
}

impl Cached4 {
    /// Builds the addend from one point, the same in every lane.
    pub fn broadcast(point: &Point, two_d: &crate::field::Fe) -> Self {
        Cached4 {
            y_plus_x: Fe4::broadcast(&point.y.add(&point.x)),
            y_minus_x: Fe4::broadcast(&point.y.sub(&point.x)),
            z: Fe4::broadcast(&point.z),
            t2d: Fe4::broadcast(&point.t.mul(two_d)),
        }
    }
}

/// `p3 + cached -> p1p1`, four lanes at a time. Four field multiplications.
///
/// This is the hot loop. The single carry is not optional: `t0` is doubled and
/// then both added to and subtracted from, and without reducing it first the
/// operands would leave the range multiplication accepts.
#[inline]
pub fn add(p: &Point4, q: &Cached4) -> P1P14 {
    let a = p.y.add(&p.x);
    let b = p.y.sub(&p.x);
    let zz = field4::mul(&a, &q.y_plus_x);
    let yy = field4::mul(&b, &q.y_minus_x);
    let tt = field4::mul(&q.t2d, &p.t);
    let t0 = field4::mul(&p.z, &q.z);
    let t0 = t0.add(&t0).carry();
    P1P14 {
        x: zz.sub(&yy),
        y: zz.add(&yy),
        z: t0.add(&tt),
        t: t0.sub(&tt),
    }
}

/// `p1p1 -> p3`, four lanes at a time. Four field multiplications and no
/// reductions of its own: multiplication takes the unreduced coordinates
/// directly.
#[inline]
pub fn to_p3(p: &P1P14) -> Point4 {
    Point4 {
        x: field4::mul(&p.x, &p.t),
        y: field4::mul(&p.y, &p.z),
        z: field4::mul(&p.z, &p.t),
        t: field4::mul(&p.x, &p.y),
    }
}

/// Advances four chains by one step.
#[inline]
pub fn step(p: &Point4, q: &Cached4) -> Point4 {
    to_p3(&add(p, q))
}

/// The constant every chain steps by, given how many `8G` steps separate
/// consecutive candidates within one lane.
///
/// With four lanes the lanes interleave, so each lane advances by `4 · 8G`
/// while the candidate at lane `j` of step `i` sits at offset `8(j + 4i)`.
pub fn chain_step(lanes: u64) -> Point {
    let two_d = curve::two_d();
    let eight_g = curve::eight_basepoint_cached();
    let mut acc = Point::identity();
    for _ in 0..lanes {
        acc = curve::to_p3(&curve::add(&acc, &eight_g));
    }
    let _ = two_d;
    acc
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::{eight_basepoint_cached, two_d, Point};
    use crate::key;

    fn seed_from(n: u64) -> [u8; 32] {
        let mut b = [0u8; 32];
        let mut x = n.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(0xC0FFEE);
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
        curve::scalar_base_mult(&sk, &Point::basepoint(), &two_d())
    }

    #[test]
    fn gather_scatter_roundtrip() {
        let points: [Point; 4] = std::array::from_fn(|i| start(i as u64));
        let restored = Point4::from_points(&points).to_points();
        for (lane, (got, want)) in restored.iter().zip(points.iter()).enumerate() {
            assert_eq!(key::pack(got), key::pack(want), "lane {lane}");
        }
    }

    /// The vector step must produce exactly what the scalar step produces, on
    /// every lane. An error here would give valid-looking keys for the wrong
    /// candidates.
    #[test]
    fn stepping_matches_the_scalar_path() {
        let two_d = two_d();
        let step_point = chain_step(1);
        let cached4 = Cached4::broadcast(&step_point, &two_d);
        let cached = eight_basepoint_cached();

        let mut scalar: [Point; 4] = std::array::from_fn(|i| start(i as u64));
        let mut vector = Point4::from_points(&scalar);

        for round in 0..16 {
            for p in scalar.iter_mut() {
                *p = curve::to_p3(&curve::add(p, &cached));
            }
            vector = step(&vector, &cached4);

            let got = vector.to_points();
            for lane in 0..4 {
                assert_eq!(
                    key::pack(&got[lane]),
                    key::pack(&scalar[lane]),
                    "round {round} lane {lane}"
                );
            }
        }
    }

    /// Four lanes stepping by `4 · 8G` must cover the same candidates as one
    /// chain stepping by `8G`, just interleaved.
    #[test]
    fn interleaved_lanes_cover_one_chain() {
        let two_d = two_d();
        let cached = eight_basepoint_cached();
        let base = start(7);

        // One chain, 32 candidates.
        let mut serial = Vec::new();
        let mut p = base;
        for _ in 0..32 {
            serial.push(key::pack(&p));
            p = curve::to_p3(&curve::add(&p, &cached));
        }

        // Four lanes: lane j starts at base + j·8G, each steps by 4·8G.
        let mut starts = [base; 4];
        for j in 1..4 {
            starts[j] = curve::to_p3(&curve::add(&starts[j - 1], &cached));
        }
        let cached4 = Cached4::broadcast(&chain_step(4), &two_d);
        let mut vector = Point4::from_points(&starts);

        for i in 0..8 {
            let got = vector.to_points();
            for (j, point) in got.iter().enumerate() {
                assert_eq!(
                    key::pack(point),
                    serial[j + 4 * i],
                    "step {i} lane {j} landed on the wrong candidate"
                );
            }
            vector = step(&vector, &cached4);
        }
    }
}
