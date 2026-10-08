//! The matcher against the printed address, at every position.
//!
//! The hot loop never builds the address; it reads the packed key, which is the
//! address minus its checksum and minus one bit. Every shortcut that follows
//! from that is an opportunity to miss a match and say nothing, and a missed
//! match looks exactly like bad luck.
//!
//! So this compares against the only thing that cannot be wrong: the address as
//! it would be printed, searched with an ordinary string search.
//!
//! It exists because that class of defect happened. The engine defers the
//! ed25519 sign bit, and in base32 order that bit lands inside symbol 49, where
//! it is worth two — so symbol 49 of a candidate is not symbol 49 of its
//! address. Any substring placement covering it with one of the sixteen symbols
//! whose value has that bit set was never found, which cost 7.6% of the
//! occurrences of a three-symbol substring and was invisible from inside.

use onion_gen::batch4::BatchEngine4;
use onion_gen::curve::{self, Point};
use onion_gen::filter::{FilterSet, KeyVerdict};
use onion_gen::key;

/// Walks candidates, and for each one compares what the set says with what a
/// plain search over the printed address says.
///
/// Returns how many occurrences there were at each address offset, how many of
/// them the set missed, and how many times it claimed a match where the address
/// has none.
fn compare(spec: &str, needle: &[u8], candidates: usize) -> (Vec<u64>, Vec<u64>, u64) {
    let set = FilterSet::parse_all([spec]).expect("a valid filter");
    let seed = [0x4du8; 32];
    let scalar = key::secret_scalar(&seed);
    let start = curve::scalar_base_mult(&scalar, &Point::basepoint(), &curve::two_d());
    let mut engine = BatchEngine4::new(4096);
    let mut chains = BatchEngine4::split_chains(&start);

    let mut occurred = vec![0u64; 56];
    let mut missed = vec![0u64; 56];
    let mut invented = 0u64;
    let mut examined = 0usize;
    while examined < candidates {
        engine.run(&mut chains);
        let batch = engine.packed().len();
        for i in 0..batch {
            let candidate = engine.packed()[i];
            let full = engine.packed_with_sign(i);
            let text = key::address(&full);
            let truth = text
                .as_bytes()
                .windows(needle.len())
                .position(|w| w == needle);
            let address = key::address_bytes(&full);
            let found = match set.examine_key(&candidate) {
                KeyVerdict::Matched(_) => true,
                KeyVerdict::No => false,
                KeyVerdict::NeedsAddress => set.match_whole_address(&candidate, &address).is_some(),
            };
            match truth {
                Some(at) => {
                    occurred[at] += 1;
                    if !found {
                        missed[at] += 1;
                    }
                }
                // Claiming a match the address does not have is the worse
                // half of the same mistake: it writes out a key whose address
                // is not what was asked for.
                None => {
                    if found {
                        invented += 1;
                    }
                }
            }
        }
        examined += batch;
    }
    (occurred, missed, invented)
}

fn assert_nothing_missed(spec: &str, needle: &[u8], candidates: usize) {
    let (occurred, missed, invented) = compare(spec, needle, candidates);
    let total: u64 = occurred.iter().sum();
    let lost: u64 = missed.iter().sum();
    assert!(
        total > 500,
        "{spec}: only {total} occurrences, too few to have tested anything"
    );
    assert_eq!(
        invented, 0,
        "{spec}: claimed {invented} matches the address does not have"
    );
    if lost > 0 {
        let where_: Vec<String> = missed
            .iter()
            .enumerate()
            .filter(|(_, m)| **m > 0)
            .map(|(at, m)| format!("offset {at}: {m} of {}", occurred[at]))
            .collect();
        panic!("{spec}: missed {lost} of {total} — {}", where_.join(", "));
    }
}

/// A substring is the form with the most placements, so it reaches every
/// position of the address including the ones the key does not settle.
#[test]
fn every_placement_of_a_substring_is_found() {
    assert_nothing_missed("contains:ab", b"ab", 200_000);
}

/// The same, where one position is a class rather than a literal: the class
/// path builds its own structures, so it can lose placements of its own.
#[test]
fn every_placement_of_a_substring_with_a_class_is_found() {
    let (occurred, missed, invented) = compare("contains:a[b]", b"ab", 200_000);
    let total: u64 = occurred.iter().sum();
    assert!(total > 500, "only {total} occurrences");
    assert_eq!(missed.iter().sum::<u64>(), 0, "missed {missed:?}");
    assert_eq!(invented, 0, "claimed {invented} matches that are not there");
}

/// Ten substrings, which is where the automaton replaces one search per
/// filter — a different code path with a different prefilter.
#[test]
fn the_automaton_finds_every_placement_too() {
    let specs: Vec<String> = ["ab", "cd", "ef", "gh", "ij", "kl", "mn", "op", "qr", "st"]
        .iter()
        .map(|s| format!("contains:{s}"))
        .collect();
    let set = FilterSet::parse_all(&specs).expect("valid filters");
    let seed = [0x4du8; 32];
    let scalar = key::secret_scalar(&seed);
    let start = curve::scalar_base_mult(&scalar, &Point::basepoint(), &curve::two_d());
    let mut engine = BatchEngine4::new(4096);
    let mut chains = BatchEngine4::split_chains(&start);

    let mut total = 0u64;
    let mut lost = 0u64;
    for _ in 0..12 {
        engine.run(&mut chains);
        for i in 0..engine.packed().len() {
            let candidate = engine.packed()[i];
            let full = engine.packed_with_sign(i);
            let text = key::address(&full);
            let truth = specs
                .iter()
                .any(|s| text.contains(s.trim_start_matches("contains:")));
            if !truth {
                continue;
            }
            total += 1;
            let address = key::address_bytes(&full);
            let found = match set.examine_key(&candidate) {
                KeyVerdict::Matched(_) => true,
                KeyVerdict::No => false,
                KeyVerdict::NeedsAddress => set.match_whole_address(&candidate, &address).is_some(),
            };
            if !found {
                lost += 1;
            }
        }
    }
    assert!(total > 500, "only {total} candidates matched at all");
    assert_eq!(lost, 0, "the automaton missed {lost} of {total}");
}
