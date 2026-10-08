//! Does the predicted probability match what a run actually finds?
//!
//! The estimate of waiting time rests entirely on `FilterSet::probability`. If
//! it is wrong, every number the run prints about time is wrong with it, and
//! nothing else would catch that — a wrong estimate still produces correct
//! keys, just later or sooner than promised.
//!
//! So this counts. It generates candidates with the same engine the search
//! uses, matches them against the same filter set, and compares the hit rate
//! with the prediction. Nothing is written to disk.
//!
//! Run with: cargo run --release --example hit-rate

use onion_gen::batch4::BatchEngine4;
use onion_gen::curve::{self, Point};
use onion_gen::filter::{FilterSet, KeyVerdict};
use onion_gen::key;

/// Poisson: the count has standard deviation `sqrt(expected)`, so this many
/// hits puts the two-sigma band at about 3%.
///
/// Every case is chosen to reach it in seconds on one thread. A four-symbol
/// prefix would need two billion candidates for the same confidence, which is
/// minutes — and it would be measuring the same arithmetic as a three-symbol
/// one, only more slowly.
const TARGET_HITS: f64 = 4000.0;

fn main() {
    let cases: [(&str, &[&str]); 6] = [
        ("prefix, 3 symbols", &["abc"]),
        ("three prefixes", &["abc", "bcd", "cde"]),
        ("class on one position", &["ab[c-f]"]),
        ("wildcard in the middle", &["a?c"]),
        // The one the tail arithmetic is about: the last symbol is fixed and
        // the one before it has four values, so this is far commoner than a
        // three-symbol prefix.
        ("suffix, 3 symbols", &["suffix:qad"]),
        // The one where the estimate is an upper bound, because the placements
        // overlap. If the sum is far off, it shows here and nowhere else.
        ("substring, 3 symbols", &["contains:abc"]),
    ];

    println!(
        "{:<24} {:>12} {:>10} {:>10} {:>8}",
        "case", "candidates", "predicted", "found", "found/pred"
    );
    let mut worst = 0.0f64;
    for (name, filters) in cases {
        let set = FilterSet::parse_all(filters.iter().copied()).expect("valid filters");
        let p = set.probability();
        let want = (TARGET_HITS / p) as u64;
        let (examined, found) = count(&set, want);
        let predicted = examined as f64 * p;
        let ratio = found as f64 / predicted;
        worst = worst.max((ratio - 1.0).abs());
        println!("{name:<24} {examined:>12} {predicted:>10.0} {found:>10} {ratio:>8.3}");
    }
    println!(
        "\nthe worst disagreement is {:.1}%, against a two-sigma band of about 3%",
        worst * 100.0
    );
    placements();
    one_offset(47);
    one_offset(48);
}

/// Walks the same chain the search walks and counts what matches.
fn count(set: &FilterSet, at_least: u64) -> (u64, u64) {
    let seed = [0x17u8; 32];
    let scalar = key::secret_scalar(&seed);
    let start = curve::scalar_base_mult(&scalar, &Point::basepoint(), &curve::two_d());
    let mut engine = BatchEngine4::new(4096);
    let mut chains = BatchEngine4::split_chains(&start);

    let mut examined = 0u64;
    let mut found = 0u64;
    while examined < at_least {
        engine.run(&mut chains);
        let batch = engine.packed().len();
        for i in 0..batch {
            let candidate = engine.packed()[i];
            // The same two-step the search does: decide on the key where the
            // key is enough, and build the address where it is not. Counting
            // only the key-side span would miss every suffix and the last few
            // placements of every substring — which is exactly the difference
            // between measuring the estimate and measuring the shortcut.
            match set.examine_key(&candidate) {
                KeyVerdict::Matched(_) => found += 1,
                KeyVerdict::No => {}
                KeyVerdict::NeedsAddress => {
                    let full = engine.packed_with_sign(i);
                    let address = key::address_bytes(&full);
                    if set.match_whole_address(&candidate, &address).is_some() {
                        found += 1;
                    }
                }
            }
        }
        examined += batch as u64;
    }
    (examined, found)
}

/// Where the set and a plain string search disagree, and at which offset.
///
/// The estimate counts every placement of a substring, including the few that
/// straddle the boundary between the symbols the key produces and the ones the
/// checksum produces. Whether the search finds those is a different question
/// from whether the estimate counts them, and this answers it.
fn placements() {
    let set = FilterSet::parse_all(["contains:abc"]).expect("valid");
    let seed = [0x29u8; 32];
    let scalar = key::secret_scalar(&seed);
    let start = curve::scalar_base_mult(&scalar, &Point::basepoint(), &curve::two_d());
    let mut engine = BatchEngine4::new(4096);
    let mut chains = BatchEngine4::split_chains(&start);

    let mut by_offset = [0u64; 56];
    let mut missed = [0u64; 56];
    let mut examined = 0u64;
    while examined < 3_000_000 {
        engine.run(&mut chains);
        let batch = engine.packed().len();
        for i in 0..batch {
            let candidate = engine.packed()[i];
            let full = engine.packed_with_sign(i);
            let address = key::address_bytes(&full);
            // The truth: a plain search over the printed address, which is the
            // base32 text and not the thirty-five bytes the matcher works on.
            let text = key::address(&full);
            let Some(at) = text.as_bytes().windows(3).position(|w| w == b"abc") else {
                continue;
            };
            by_offset[at] += 1;
            let found = match set.examine_key(&candidate) {
                KeyVerdict::Matched(_) => true,
                KeyVerdict::No => false,
                KeyVerdict::NeedsAddress => set.match_whole_address(&candidate, &address).is_some(),
            };
            if !found {
                missed[at] += 1;
            }
        }
        examined += batch as u64;
    }

    println!("\nwhere a substring lands, and whether the set sees it:");
    println!("{:>7} {:>10} {:>8}", "offset", "occurred", "missed");
    for (at, count) in by_offset.iter().enumerate() {
        if *count > 0 || missed[at] > 0 {
            println!("{at:>7} {count:>10} {:>8}", missed[at]);
        }
    }
    let total: u64 = by_offset.iter().sum();
    let lost: u64 = missed.iter().sum();
    println!("occurrences {total}, missed {lost}");
}

/// Two candidates, one at each of the offsets that disagree, printed in full.
fn one_offset(target: usize) {
    let set = FilterSet::parse_all(["contains:abc"]).expect("valid");
    let seed = [0x31u8; 32];
    let scalar = key::secret_scalar(&seed);
    let start = curve::scalar_base_mult(&scalar, &Point::basepoint(), &curve::two_d());
    let mut engine = BatchEngine4::new(4096);
    let mut chains = BatchEngine4::split_chains(&start);
    let mut seen = 0;
    while seen < 2 {
        engine.run(&mut chains);
        for i in 0..engine.packed().len() {
            let candidate = engine.packed()[i];
            let full = engine.packed_with_sign(i);
            let text = key::address(&full);
            let Some(at) = text.as_bytes().windows(3).position(|w| w == b"abc") else {
                continue;
            };
            if at != target {
                continue;
            }
            let verdict = match set.examine_key(&candidate) {
                KeyVerdict::Matched(_) => "Matched",
                KeyVerdict::No => "No",
                KeyVerdict::NeedsAddress => "NeedsAddress",
            };
            println!("offset {at}: examine_key says {verdict}");
            println!("  address   {text}");
            let mut key_text = [0u8; 51];
            onion_gen::base32::encode_into(&candidate, &mut key_text);
            println!(
                "  key text  {}",
                std::str::from_utf8(&key_text).expect("ascii")
            );
            seen += 1;
            if seen == 2 {
                return;
            }
        }
    }
}
