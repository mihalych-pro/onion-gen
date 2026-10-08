//! The chain scheme against the paired scheme, on the same field arithmetic.
//!
//! The engine walks a chain: every candidate costs a full point addition in
//! extended coordinates. The paired scheme instead keeps a table of affine
//! offsets `Q(m) = 8m·G` and reads two candidates out of one base point:
//!
//! ```text
//! y(P+Q) = (x1y1 - x2y2) / (x1y2 - y1x2)
//! y(P-Q) = (x1y1 + x2y2) / (x1y2 + y1x2)
//! ```
//!
//! Both fractions share `x1y2` and `y1x2`, so two multiplications produce two
//! candidates. The run checks the identity against the engine's own point
//! addition before timing anything.

use onion_gen::curve::{self, Point};
use onion_gen::field::Fe;
use std::time::Instant;

/// An offset from the table: affine, with the product of its coordinates.
#[derive(Clone)]
struct Affine {
    x: Fe,
    y: Fe,
    xy: Fe,
}

impl Affine {
    fn from_point(p: &Point) -> Affine {
        let zinv = p.z.invert();
        let x = p.x.mul(&zinv);
        let y = p.y.mul(&zinv);
        Affine {
            x,
            y,
            xy: x.mul(&y),
        }
    }
}

fn multiple_of_basepoint(k: u64) -> Point {
    let mut scalar = [0u8; 32];
    scalar[..8].copy_from_slice(&k.to_le_bytes());
    curve::scalar_base_mult(&scalar, &Point::basepoint(), &curve::two_d())
}

/// `y` of a point, as the engine would read it.
fn affine_y(p: &Point) -> Fe {
    p.y.mul(&p.z.invert())
}

fn check_identity() {
    let base = multiple_of_basepoint(7_919);
    let pa = Affine::from_point(&base);
    let two_d = curve::two_d();

    for m in 1..6u64 {
        let q_point = multiple_of_basepoint(8 * m);
        let q = Affine::from_point(&q_point);

        // The formula.
        let x1y2 = pa.x.mul(&q.y);
        let y1x2 = pa.y.mul(&q.x);
        let sum_y = pa.xy.sub(&q.xy).mul(&x1y2.sub(&y1x2).invert());
        let dif_y = pa.xy.add(&q.xy).mul(&x1y2.add(&y1x2).invert());

        // The engine's own addition, for the same two points.
        let sum = curve::to_p3(&curve::add(&base, &curve::to_cached(&q_point, &two_d)));
        let neg = Point {
            x: Fe::zero().sub(&q_point.x),
            y: q_point.y,
            z: q_point.z,
            t: Fe::zero().sub(&q_point.t),
        };
        let dif = curve::to_p3(&curve::add(&base, &curve::to_cached(&neg, &two_d)));

        assert_eq!(
            sum_y.to_bytes(),
            affine_y(&sum).to_bytes(),
            "P+Q disagrees at m={m}"
        );
        assert_eq!(
            dif_y.to_bytes(),
            affine_y(&dif).to_bytes(),
            "P-Q disagrees at m={m}"
        );

        // The companion identity for x, which the checksum path needs for its
        // sign bit. From the curve addition law, scaled by two so that the
        // existing `2d` constant can be used directly:
        //   x(P+Q) = 2(x1y2 + y1x2) / (2 + 2d*x1x2*y1y2)
        //   x(P-Q) = 2(y1x2 - x1y2) / (2 - 2d*x1x2*y1y2)
        let two = Fe::one().add(&Fe::one());
        let prod = pa.x.mul(&q.x).mul(&pa.y.mul(&q.y));
        let dp = two_d.mul(&prod);
        let sum_x = x1y2
            .add(&y1x2)
            .add(&x1y2.add(&y1x2))
            .mul(&two.add(&dp).invert());
        let dif_x = x1y2
            .sub(&y1x2)
            .add(&x1y2.sub(&y1x2))
            .mul(&two.sub(&dp).invert());
        let want_sum_x = sum.x.mul(&sum.z.invert());
        let want_dif_x = dif.x.mul(&dif.z.invert());
        assert_eq!(sum_x.to_bytes(), want_sum_x.to_bytes(), "x(P+Q) at m={m}");
        assert_eq!(dif_x.to_bytes(), want_dif_x.to_bytes(), "x(P-Q) at m={m}");
    }
    println!("identity holds for both forms");
}

/// Batch inversion: one inversion for the whole slice.
fn batch_invert(values: &[Fe], out: &mut Vec<Fe>) {
    out.clear();
    let mut running = Fe::one();
    for v in values {
        out.push(running);
        running = running.mul(v);
    }
    let mut inv = running.invert();
    for i in (0..values.len()).rev() {
        out[i] = out[i].mul(&inv);
        inv = inv.mul(&values[i]);
    }
}

fn time_chain(batch: usize, rounds: usize) -> f64 {
    let two_d = curve::two_d();
    let step = curve::to_cached(&multiple_of_basepoint(8), &two_d);
    let mut acc = multiple_of_basepoint(7_919);
    let mut ys = Vec::with_capacity(batch);
    let mut zs = Vec::with_capacity(batch);
    let mut inv = Vec::with_capacity(batch);

    let start = Instant::now();
    for _ in 0..rounds {
        ys.clear();
        zs.clear();
        for _ in 0..batch {
            ys.push(acc.y);
            zs.push(acc.z);
            acc = curve::to_p3(&curve::add(&acc, &step));
        }
        batch_invert(&zs, &mut inv);
        for i in 0..batch {
            std::hint::black_box(ys[i].mul(&inv[i]).to_bytes());
        }
    }
    let secs = start.elapsed().as_secs_f64();
    (batch * rounds) as f64 / secs
}

fn time_paired(batch: usize, rounds: usize) -> f64 {
    let half = batch / 2;
    let table: Vec<Affine> = (1..=half as u64)
        .map(|m| Affine::from_point(&multiple_of_basepoint(8 * m)))
        .collect();

    let two_d = curve::two_d();
    // One addition advances the base past the whole table, both directions.
    let stride = curve::to_cached(&multiple_of_basepoint(8 * (half as u64 * 2 + 2)), &two_d);
    let mut base = multiple_of_basepoint(7_919);

    let mut num = vec![Fe::one(); batch];
    let mut den = vec![Fe::one(); batch];
    let mut inv = Vec::with_capacity(batch);

    let start = Instant::now();
    for _ in 0..rounds {
        let pa = Affine::from_point(&base);
        for (j, q) in table.iter().enumerate() {
            let x1y2 = pa.x.mul(&q.y);
            let y1x2 = pa.y.mul(&q.x);
            num[2 * j] = pa.xy.sub(&q.xy);
            den[2 * j] = x1y2.sub(&y1x2);
            num[2 * j + 1] = pa.xy.add(&q.xy);
            den[2 * j + 1] = x1y2.add(&y1x2);
        }
        batch_invert(&den, &mut inv);
        for i in 0..batch {
            std::hint::black_box(num[i].mul(&inv[i]).to_bytes());
        }
        base = curve::to_p3(&curve::add(&base, &stride));
    }
    let secs = start.elapsed().as_secs_f64();
    (batch * rounds) as f64 / secs
}

fn main() {
    check_identity();

    let batch = 2048;
    let rounds = 400;
    println!("batch {batch}, {rounds} rounds, one thread\n");

    // Warm up both so the first one does not pay for the caches.
    time_chain(batch, 20);
    time_paired(batch, 20);

    let chain = time_chain(batch, rounds);
    let paired = time_paired(batch, rounds);

    println!("chain   {:>10.2} M candidates/s", chain / 1e6);
    println!("paired  {:>10.2} M candidates/s", paired / 1e6);
    println!("ratio   {:>10.2}x", paired / chain);
}
