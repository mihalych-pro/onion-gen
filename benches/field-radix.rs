//! Two representations of the same field, timed against each other.
//!
//! The engine holds the saturated one: radix 2^64 over four limbs, folded with
//! `2^256 = 38 (mod p)`. The other is `fiat-crypto`'s radix 2^51 over five
//! limbs, which is what it replaced — fewer limbs and so fewer partial
//! products, at the cost of carry chains the redundant form avoids.
//!
//! Kept because the choice is only as good as the measurement behind it, and
//! a machine with different multiplier latency could invert it.
//!
//! The run checks the two against each other on random inputs before timing.

use onion_gen::field::Fe;
use std::time::Instant;

/// Radix 2^64 over four limbs, least significant first.
type Sat = [u64; 4];

const P: Sat = [
    0xffff_ffff_ffff_ffed,
    0xffff_ffff_ffff_ffff,
    0xffff_ffff_ffff_ffff,
    0x7fff_ffff_ffff_ffff,
];

#[inline(always)]
fn mul_sat(a: &Sat, b: &Sat) -> Sat {
    // Schoolbook into eight limbs.
    let mut r = [0u64; 8];
    for i in 0..4 {
        let mut c: u128 = 0;
        for j in 0..4 {
            let t = (r[i + j] as u128) + (a[i] as u128) * (b[j] as u128) + c;
            r[i + j] = t as u64;
            c = t >> 64;
        }
        let t = (r[i + 4] as u128) + c;
        r[i + 4] = t as u64;
    }

    // Fold the top half back in: 2^256 = 38 (mod p).
    let mut out = [0u64; 4];
    let mut c: u128 = 0;
    for k in 0..4 {
        let t = (r[k] as u128) + 38u128 * (r[k + 4] as u128) + c;
        out[k] = t as u64;
        c = t >> 64;
    }
    // The leftover carry folds once more and cannot carry again.
    let mut c2 = 38u128 * c;
    for limb in out.iter_mut() {
        let t = (*limb as u128) + c2;
        *limb = t as u64;
        c2 = t >> 64;
        if c2 == 0 {
            break;
        }
    }
    out
}

/// The same representation, written out instead of looped: one carry chain per
/// row of the schoolbook, and a fold with no data-dependent branches.
#[inline(always)]
fn mac(out: &mut u64, carry: &mut u64, a: u64, b: u64) {
    let t = (a as u128) * (b as u128) + (*out as u128) + (*carry as u128);
    *out = t as u64;
    *carry = (t >> 64) as u64;
}

#[inline(always)]
fn mul_unrolled(a: &Sat, b: &Sat) -> Sat {
    let mut r = [0u64; 8];
    let mut c = 0u64;
    mac(&mut r[0], &mut c, a[0], b[0]);
    mac(&mut r[1], &mut c, a[0], b[1]);
    mac(&mut r[2], &mut c, a[0], b[2]);
    mac(&mut r[3], &mut c, a[0], b[3]);
    r[4] = c;
    c = 0;
    mac(&mut r[1], &mut c, a[1], b[0]);
    mac(&mut r[2], &mut c, a[1], b[1]);
    mac(&mut r[3], &mut c, a[1], b[2]);
    mac(&mut r[4], &mut c, a[1], b[3]);
    r[5] = c;
    c = 0;
    mac(&mut r[2], &mut c, a[2], b[0]);
    mac(&mut r[3], &mut c, a[2], b[1]);
    mac(&mut r[4], &mut c, a[2], b[2]);
    mac(&mut r[5], &mut c, a[2], b[3]);
    r[6] = c;
    c = 0;
    mac(&mut r[3], &mut c, a[3], b[0]);
    mac(&mut r[4], &mut c, a[3], b[1]);
    mac(&mut r[5], &mut c, a[3], b[2]);
    mac(&mut r[6], &mut c, a[3], b[3]);
    r[7] = c;

    // Fold with 2^256 = 38, branch free.
    let mut out = [0u64; 4];
    let mut carry: u128 = 0;
    for k in 0..4 {
        let t = (r[k] as u128) + 38u128 * (r[k + 4] as u128) + carry;
        out[k] = t as u64;
        carry = t >> 64;
    }
    let mut v = 38u64.wrapping_mul(carry as u64);
    for limb in out.iter_mut() {
        let (s, o) = limb.overflowing_add(v);
        *limb = s;
        v = u64::from(o);
    }
    out
}

/// Canonical bytes, for comparing against `Fe`.
fn sat_to_bytes(v: &Sat) -> [u8; 32] {
    let mut x = *v;
    // At most two conditional subtractions bring the value below p.
    for _ in 0..3 {
        let mut borrow = 0i128;
        let mut t = [0u64; 4];
        for k in 0..4 {
            let d = (x[k] as i128) - (P[k] as i128) - borrow;
            t[k] = d as u64;
            borrow = if d < 0 { 1 } else { 0 };
        }
        if borrow == 0 {
            x = t;
        }
    }
    let mut out = [0u8; 32];
    for k in 0..4 {
        out[k * 8..k * 8 + 8].copy_from_slice(&x[k].to_le_bytes());
    }
    out
}

fn sat_from_bytes(b: &[u8; 32]) -> Sat {
    let mut v = [0u64; 4];
    for k in 0..4 {
        v[k] = u64::from_le_bytes(b[k * 8..k * 8 + 8].try_into().unwrap());
    }
    v[3] &= 0x7fff_ffff_ffff_ffff;
    v
}

fn sample(seed: u64) -> [u8; 32] {
    let mut b = [0u8; 32];
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1);
    for chunk in b.chunks_mut(8) {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        chunk.copy_from_slice(&x.to_le_bytes());
    }
    b[31] &= 0x7f;
    b
}

fn check() {
    for s in 0..200u64 {
        let ab = sample(s);
        let bb = sample(s ^ 0x5555);
        let want = Fe::from_bytes(&ab).mul(&Fe::from_bytes(&bb)).to_bytes();
        let got = sat_to_bytes(&mul_sat(&sat_from_bytes(&ab), &sat_from_bytes(&bb)));
        assert_eq!(got, want, "the two representations disagree at seed {s}");
        let un = sat_to_bytes(&mul_unrolled(&sat_from_bytes(&ab), &sat_from_bytes(&bb)));
        assert_eq!(un, want, "the unrolled form disagrees at seed {s}");
    }
    println!("representations agree on 200 random products");
}

fn main() {
    check();

    const N: usize = 4_000_000;
    let a = Fe::from_bytes(&sample(1));
    let b = Fe::from_bytes(&sample(2));
    let sa = sat_from_bytes(&sample(1));
    let sb = sat_from_bytes(&sample(2));

    // Eight independent chains, because the engine's multiplications do not
    // depend on each other and a single chain would time latency instead.
    const W: usize = 8;
    let mut fs: [Fe; W] = [a; W];
    for (k, f) in fs.iter_mut().enumerate() {
        *f = Fe::from_bytes(&sample(10 + k as u64));
    }
    let mut ss: [Sat; W] = [sa; W];
    for (k, v) in ss.iter_mut().enumerate() {
        *v = sat_from_bytes(&sample(10 + k as u64));
    }

    for _ in 0..20_000 {
        for f in fs.iter_mut() {
            *f = f.mul(&b);
        }
    }
    std::hint::black_box(&fs);

    let rounds = N / W;
    let start = Instant::now();
    for _ in 0..rounds {
        for f in fs.iter_mut() {
            *f = f.mul(&b);
        }
    }
    std::hint::black_box(&fs);
    let tight = start.elapsed().as_secs_f64();

    let start = Instant::now();
    for _ in 0..rounds {
        for v in ss.iter_mut() {
            *v = mul_sat(v, &sb);
        }
    }
    std::hint::black_box(&ss);
    let sat = start.elapsed().as_secs_f64();

    let mut us: [Sat; W] = ss;
    for _ in 0..20_000 {
        for v in us.iter_mut() {
            *v = mul_unrolled(v, &sb);
        }
    }
    let start = Instant::now();
    for _ in 0..rounds {
        for v in us.iter_mut() {
            *v = mul_unrolled(v, &sb);
        }
    }
    std::hint::black_box(&us);
    let unrolled = start.elapsed().as_secs_f64();

    // The engine calls to_bytes once per candidate, so its cost sits beside a
    // multiply rather than below it.
    let mut fs2: [Fe; W] = fs;
    for _ in 0..20_000 {
        for f in fs2.iter_mut() {
            std::hint::black_box(f.to_bytes());
        }
    }
    let start = Instant::now();
    for _ in 0..rounds {
        for f in fs2.iter_mut() {
            std::hint::black_box(f.to_bytes());
        }
    }
    let tobytes = start.elapsed().as_secs_f64();

    let n = (rounds * W) as f64;
    println!(
        "radix 2^51, five limbs (fiat-crypto)  {:7.2} ns",
        tight / n * 1e9
    );
    println!(
        "radix 2^64, four limbs (saturated)    {:7.2} ns",
        sat / n * 1e9
    );
    println!(
        "radix 2^64, four limbs (unrolled)    {:7.2} ns",
        unrolled / n * 1e9
    );
    println!(
        "best against fiat-crypto              {:7.2}x",
        tight / unrolled.min(sat)
    );
    println!(
        "Fe::to_bytes (once per candidate)     {:7.2} ns",
        tobytes / n * 1e9
    );
}
