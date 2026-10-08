//! What each candidate feature is worth, and what computing them costs.
//!
//! Three questions, in the order the change asks them:
//!
//! 1. How often does each feature occur? A feature present in most addresses
//!    distinguishes nothing, however pretty it sounds, and the only way to know
//!    which ones those are is to count.
//! 2. What does scoring cost per find? "Only on finds" is not the same as
//!    free: a three-symbol filter produces thousands of finds a second, and the
//!    palindrome scan is quadratic.
//! 3. What does a total score mean? Not the sum of the parts — the families
//!    overlap — so the answer is measured over whole addresses instead.
//!
//! Run with: cargo run --release --example score-calibration

use std::time::Instant;

use onion_gen::key;
use onion_gen::score::{self, Family};

/// Big enough that the rare tail is counted rather than glimpsed. At 300 000 a
/// run of six identical symbols turned up exactly once, and one observation
/// gives no frequency at all.
const SAMPLE: usize = 5_000_000;

/// Below this many occurrences a level is reported but not trusted: the
/// estimate is dominated by whether the sample happened to contain it.
const TRUST: usize = 30;

fn main() {
    let addresses = random_addresses(SAMPLE);

    println!("== 1. How often each feature occurs, {SAMPLE} random addresses ==\n");
    for family in [
        Family::Run,
        Family::Tiling,
        Family::Palindrome,
        Family::FewDigits,
    ] {
        let levels: Vec<usize> = addresses
            .iter()
            .map(|a| score::levels(a.as_bytes(), None).of(family).unwrap_or(0))
            .collect();
        println!("  {}:", family.name());
        let top = *levels.iter().max().unwrap_or(&0);
        let bottom = *levels.iter().min().unwrap_or(&0);
        for level in bottom..=top {
            let at_least = levels.iter().filter(|&&l| l >= level).count();
            if at_least == 0 {
                break;
            }
            let share = at_least as f64 / SAMPLE as f64;
            // Levels nearly everyone reaches say nothing; stop printing them
            // once they are below a fifth, where a feature starts to inform.
            if share > 0.9 {
                continue;
            }
            println!(
                "    at least {level:>2}: {share:>12.8}  {:>5.1} bits  ({at_least}{})",
                -share.log2(),
                if at_least < TRUST {
                    ", too few to trust"
                } else {
                    ""
                }
            );
        }
        println!();
    }

    println!("== 2. What scoring costs per find ==\n");
    let cost = |label: &str, f: &dyn Fn(&[u8]) -> usize| {
        let mut best = 0.0f64;
        for _ in 0..3 {
            let t = Instant::now();
            let mut sink = 0usize;
            for a in &addresses {
                sink += f(a.as_bytes());
            }
            let rate = addresses.len() as f64 / t.elapsed().as_secs_f64();
            std::hint::black_box(sink);
            best = best.max(rate);
        }
        println!(
            "  {label:<28} {:>8.2} M/s  {:>7.0} ns",
            best / 1e6,
            1e9 / best
        );
    };
    cost("longest run", &score::longest_run);
    cost("longest tiling", &score::longest_tiling);
    cost("longest palindrome", &score::longest_palindrome);
    cost("digit count", &score::digit_count);
    cost("all four together", &|a| score::levels(a, None).run);

    // The number scoring has to be compared against is not zero but the cost
    // of the thing it accompanies. A find already pays for a directory and
    // three files; if scoring is a rounding error against that, "only on
    // finds" really does mean what it sounds like.
    let dir = std::env::temp_dir().join(format!("onion-gen-score-cost-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut keys = 0usize;
    let started = Instant::now();
    let mut state = 0x1234_5678_9abc_def0u64;
    while started.elapsed().as_millis() < 700 {
        let mut k = [0u8; 32];
        for chunk in k.chunks_mut(8) {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            chunk.copy_from_slice(&state.to_le_bytes()[..chunk.len()]);
        }
        let secret = [0u8; 64];
        if onion_gen::output::write_key(&dir, &k, &secret).is_ok() {
            keys += 1;
        }
    }
    let per_key = started.elapsed().as_secs_f64() / keys as f64;
    let _ = std::fs::remove_dir_all(&dir);
    println!(
        "  {:<28} {:>8.0} /s  {:>7.0} ns   <- what a find already costs ({keys} keys)",
        "writing the key to disk",
        1.0 / per_key,
        per_key * 1e9
    );
    println!(
        "\n  scoring is {:.4}% of writing one key",
        100.0 * 254e-9 / per_key
    );

    println!("\n== 3. A feature measured and left out ==\n");
    // Recorded rather than dropped in silence. Two reasons, and the second is
    // the one that decides it. Below sixteen symbols the feature describes
    // most addresses rather than the good ones. Above it, it is not a second
    // feature at all: it and `few digits` are two views of how sparse the
    // digits are, and keeping both would count that once for the count and
    // again for the longest gap. The count is kept because it grades further —
    // 1.8 to 15.9 bits against 1.9 to 3.1.
    for want in [10usize, 12, 14, 16, 20] {
        let hits = addresses
            .iter()
            .filter(|a| longest_letter_run(a.as_bytes()) >= want)
            .count();
        let share = hits as f64 / SAMPLE as f64;
        println!(
            "  letters-only run of {want:>2}+: {share:>10.6}  {:>5.1} bits{}",
            -share.log2(),
            if share > 1.0 / 3.0 {
                "   <- too common to inform"
            } else {
                ""
            }
        );
    }

    println!("\n== 4. What a total score means ==\n");
    // The families overlap — a run of three identical symbols is also a
    // palindrome of three — so the total is not the rarity its parts add up
    // to. What it means is measured here over whole addresses, with the
    // overlap already inside the measurement, and checked on a second sample
    // the table was not built from.
    let first = calibrate(&addresses, "built on the first sample");
    let second_sample = random_addresses_from(SAMPLE, 0xdead_beef_cafe_f00d);
    let second = calibrate(&second_sample, "checked on an independent sample");

    println!("  agreement between the two:");
    for ((cut, a), (_, b)) in first.iter().zip(second.iter()) {
        // The sampling error on a share p over n draws is about sqrt(p/n);
        // three of those is the band a fair disagreement stays inside.
        let band = 3.0 * (a / SAMPLE as f64).sqrt();
        let ok = (a - b).abs() <= band;
        println!(
            "    score >= {cut:>4.0}: {a:>12.8} vs {b:>12.8}  band {band:>12.8}  {}",
            if ok { "ok" } else { "DISAGREES" }
        );
    }
}

fn calibrate(addresses: &[String], label: &str) -> Vec<(f64, f64)> {
    let mut totals: Vec<f64> = addresses
        .iter()
        .map(|a| score::score(a.as_bytes(), None).total)
        .collect();
    totals.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = totals.len() as f64;
    println!("  {label}:");
    let mut table = Vec::new();
    for cut in [
        0.0f64, 2.0, 4.0, 6.0, 8.0, 10.0, 12.0, 14.0, 16.0, 18.0, 20.0, 24.0, 28.0,
    ] {
        let at_least = totals.iter().filter(|&&t| t >= cut).count();
        let share = at_least as f64 / n;
        if at_least == 0 {
            break;
        }
        println!(
            "    score >= {cut:>4.0}: {share:>12.8}  one in {:>12.0}  ({at_least})",
            1.0 / share
        );
        table.push((cut, share));
    }
    println!();
    table
}

/// The longest stretch with no digit in it. Measured here and nowhere else:
/// the point of measuring it is to establish that it does not belong in the
/// product, so putting it in the product to find that out would be backwards.
fn longest_letter_run(address: &[u8]) -> usize {
    let mut best = 0usize;
    let mut cur = 0usize;
    for b in &address[..address.len() - 2] {
        cur = if b.is_ascii_digit() { 0 } else { cur + 1 };
        best = best.max(cur);
    }
    best
}

/// Real protocol addresses: a random key run through the same encoder the
/// product uses, so the checksum and the fixed tail are what they will be in a
/// finished address rather than what a synthesiser guessed.
fn random_addresses(n: usize) -> Vec<String> {
    random_addresses_from(n, 0x9e37_79b9_7f4a_7c15)
}

fn random_addresses_from(n: usize, seed: u64) -> Vec<String> {
    let mut state = seed;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let mut k = [0u8; 32];
        for chunk in k.chunks_mut(8) {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            chunk.copy_from_slice(&state.to_le_bytes()[..chunk.len()]);
        }
        out.push(key::address(&k));
    }
    out
}
