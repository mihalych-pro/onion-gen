//! Measures what the bitmap index adds on top of the sorted exact check.
//!
//! Both configurations use the same sorted lookup; they differ only in whether
//! the bitmap rejects a candidate before it. So the "no index" column is not a
//! linear scan — it is the exact check on its own.
//!
//! An end-to-end run cannot answer this: on short prefixes hits become frequent
//! enough that writing keys dominates the measurement
//! (docs/analysis/benchmark-plan.md, section 3). Here nothing is written and no
//! curve arithmetic runs, so what is timed is the matcher.
//!
//!     cargo run --release --example match-bench

use onion_gen::filter::{Filter, FilterSet};
use std::time::Instant;

const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
const SAMPLES: usize = 4_000_000;

/// Deterministic pseudo-random keys: the same stream for every configuration,
/// so the comparison is not affected by which keys happened to be drawn.
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
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut seen = std::collections::HashSet::new();
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

fn time(set: &FilterSet, sample: &[[u8; 32]]) -> f64 {
    // A warm-up pass so the first configuration is not charged for cold caches.
    let mut sink = 0usize;
    for k in sample.iter().take(100_000) {
        sink += usize::from(set.match_key(k).is_some());
    }
    let start = Instant::now();
    for k in sample {
        sink += usize::from(set.match_key(k).is_some());
    }
    let elapsed = start.elapsed().as_secs_f64();
    std::hint::black_box(sink);
    sample.len() as f64 / elapsed
}

fn main() {
    let sample = keys(SAMPLES);
    println!(
        "{:>8} {:>8} {:>16} {:>16} {:>10}  verdict",
        "symbols", "filters", "with index/s", "no index/s", "index adds"
    );

    for length in [2usize, 3, 4, 5, 6] {
        for count in [1usize, 1000] {
            let parsed = filters(count, length);
            // Width 7 is below the minimum the index accepts, which is how the
            // index-free configuration is obtained without a separate code path.
            let scan = FilterSet::with_index_bits(parsed.clone(), 7);
            let indexed = FilterSet::new(parsed);

            let scan_rate = time(&scan, &sample);
            let indexed_rate = time(&indexed, &sample);
            let speedup = indexed_rate / scan_rate;
            let used = indexed.index_memory().is_some();

            println!(
                "{length:>8} {count:>8} {indexed_rate:>16.0} {scan_rate:>16.0} {speedup:>9.2}x  {}",
                if used { "indexed" } else { "index disabled" }
            );
        }
    }
}
