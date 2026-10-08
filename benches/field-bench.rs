//! Field multiplication: the scalar path the engine uses today against the
//! vector path.
//!
//! The comparison the microbenchmark in scripts/bench/neon/ makes is against a
//! hand-written C baseline. This one measures our own code, which is what
//! actually has to get faster, and reports nanoseconds per field
//! multiplication so the two are directly comparable.
//!
//!     cargo run --release --example field-bench

use onion_gen::field::Fe;
use onion_gen::field4::{self, Fe4, LANES};
use std::time::Instant;

/// Enough iterations that a single measurement runs for seconds, not
/// microseconds.
const ROUNDS: usize = 20_000_000;

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

/// Independent chains per measurement.
///
/// A single chain of dependent multiplications measures latency, not
/// throughput: each result feeds the next, so the processor cannot overlap
/// them. The hot loop has many independent candidates in flight, so throughput
/// is what matters — and comparing a serial vector chain against four
/// interleaved scalar ones would flatter the scalar path by exactly the amount
/// of instruction-level parallelism it was handed.
const CHAINS: usize = 4;

/// Nanoseconds per field multiplication for the scalar path.
///
/// Four multiplications per round, so one round does the same work as one
/// vector call and the two numbers mean the same thing.
fn scalar_ns() -> f64 {
    let mut acc: [[Fe; LANES]; CHAINS] =
        std::array::from_fn(|c| std::array::from_fn(|i| sample((c * LANES + i) as u64 + 1)));
    let b: [Fe; LANES] = std::array::from_fn(|i| sample(i as u64 + 100));

    let rounds = ROUNDS / (LANES * CHAINS);
    let start = Instant::now();
    for _ in 0..rounds {
        for chain in acc.iter_mut() {
            for lane in 0..LANES {
                chain[lane] = chain[lane].mul(&b[lane]);
            }
        }
    }
    let elapsed = start.elapsed().as_secs_f64();
    std::hint::black_box(acc);
    elapsed * 1e9 / (rounds * LANES * CHAINS) as f64
}

/// Nanoseconds per field multiplication for a four-lane path.
fn vector_ns(mul: fn(&Fe4, &Fe4) -> Fe4) -> f64 {
    let mut acc: [Fe4; CHAINS] = std::array::from_fn(|c| {
        Fe4::from_elements(&std::array::from_fn(|i| sample((c * LANES + i) as u64 + 1)))
    });
    let b = Fe4::from_elements(&std::array::from_fn(|i| sample(i as u64 + 100)));

    let rounds = ROUNDS / (LANES * CHAINS);
    let start = Instant::now();
    for _ in 0..rounds {
        for slot in acc.iter_mut() {
            *slot = mul(slot, &b);
        }
    }
    let elapsed = start.elapsed().as_secs_f64();
    std::hint::black_box(acc);
    elapsed * 1e9 / (rounds * LANES * CHAINS) as f64
}

fn main() {
    // A warm-up pass so the first configuration is not charged for a cold cache
    // or an unsettled clock.
    std::hint::black_box(vector_ns(field4::mul_lanes));

    let scalar = scalar_ns();
    let lanes = vector_ns(field4::mul_lanes);
    let vector = vector_ns(field4::mul);

    println!("field multiplications: {ROUNDS}");
    println!(
        "{:<44} {:>8} {:>10}",
        "implementation", "ns/mul", "vs scalar"
    );
    println!(
        "{:<44} {scalar:>8.2} {:>9.2}x",
        "scalar, fiat-crypto radix 2^51 (in use today)", 1.0
    );
    println!(
        "{:<44} {lanes:>8.2} {:>9.2}x",
        "four lanes, radix 2^25.5, one lane at a time",
        scalar / lanes
    );
    // The label comes from what actually dispatched, not from the target: a
    // benchmark that misreports which path it timed is worse than no benchmark.
    println!(
        "{:<44} {vector:>8.2} {:>9.2}x",
        format!("four lanes, radix 2^25.5, {}", field4::active().name()),
        scalar / vector
    );
    println!();
    println!("dispatched to: {}", field4::describe());
}
