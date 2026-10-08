//! Where a large dictionary actually costs throughput.
//!
//! Two effects pull in opposite directions as the filter count grows:
//! a narrow index lets more candidates through to the exact check, while a wide
//! one stops fitting in cache. This isolates both from the curve arithmetic.
//!
//!     cargo run --release --example index-width

use onion_gen::filter::{Filter, FilterSet};
use std::time::Instant;

const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
const SAMPLES: usize = 2_000_000;

fn keys(count: usize) -> Vec<[u8; 32]> {
    let mut out = Vec::with_capacity(count);
    let mut x: u64 = 0x2545_F491_4F6C_DD1D;
    for _ in 0..count {
        let mut k = [0u8; 32];
        for chunk in k.chunks_mut(8) {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            chunk.copy_from_slice(&x.to_le_bytes());
        }
        out.push(k);
    }
    out
}

fn filters(count: usize, length: usize) -> Vec<Filter> {
    let mut out = Vec::with_capacity(count);
    let mut seen = std::collections::HashSet::new();
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    while out.len() < count {
        let mut text = String::with_capacity(length);
        for _ in 0..length {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            text.push(ALPHABET[(x % 32) as usize] as char);
        }
        if seen.insert(text.clone()) {
            out.push(Filter::parse(&text).unwrap());
        }
    }
    out
}

fn main() {
    let sample = keys(SAMPLES);
    println!(
        "{:>10} {:>5} {:>9} {:>14} {:>12} {:>10}",
        "filters", "bits", "index", "keys/sec", "false hits", "occupancy"
    );

    for count in [1_000usize, 10_000, 100_000, 1_000_000] {
        let parsed = filters(count, 6);
        for bits in [12u32, 14, 16, 18, 20, 22, 24, 27] {
            let set = FilterSet::with_index_bits(parsed.clone(), bits);
            let Some(bytes) = set.index_memory() else {
                println!(
                    "{count:>10} {bits:>5} {:>9} {:>14} {:>12} {:>10}",
                    "off", "-", "-", "-"
                );
                continue;
            };

            // Warm-up, then the timed pass.
            let mut sink = 0usize;
            for k in sample.iter().take(50_000) {
                sink += usize::from(set.match_key(k).is_some());
            }
            let start = Instant::now();
            for k in &sample {
                sink += usize::from(set.match_key(k).is_some());
            }
            let rate = SAMPLES as f64 / start.elapsed().as_secs_f64();
            std::hint::black_box(sink);

            // Expected share of candidates the index lets through: the exact
            // check runs on each of them.
            let occupancy = count as f64 / (1u64 << bits) as f64;
            println!(
                "{count:>10} {bits:>5} {:>8} {rate:>14.0} {:>11.3}% {:>9.2}%",
                if bytes >= 1 << 20 {
                    format!("{} MiB", bytes >> 20)
                } else {
                    format!("{} KiB", bytes >> 10)
                },
                occupancy * 100.0,
                occupancy * 100.0
            );
        }
    }
}
