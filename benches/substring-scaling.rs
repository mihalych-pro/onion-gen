//! Where the automaton starts beating one search per filter.
//!
//! A substring is matched today by encoding the candidate once and running
//! `memmem` for each filter. That is linear in the number of filters. An
//! Aho-Corasick automaton walks the text once whatever the number, but pays a
//! table lookup per byte instead of a SIMD scan, so it must lose on small sets.
//!
//! The crossover is a number, not a matter of taste, and the implementation
//! switches on it.
//!
//! Run with: cargo run --release --example substring-scaling

use std::time::Instant;

use aho_corasick::{AhoCorasick, AhoCorasickKind};
use memchr::memmem;
use onion_gen::base32::{self, ALPHABET};
use onion_gen::filter::KEY_ONLY_SYMBOLS;

/// Long enough that no candidate ever matches, so both paths do the full work
/// on every candidate rather than stopping early on a hit.
const NEEDLE_LEN: usize = 8;
const CANDIDATES: usize = 1 << 16;

fn main() {
    let candidates = build_candidates(CANDIDATES);

    println!(
        "Substrings of {NEEDLE_LEN} symbols over {} candidates, best of three, \
         single thread.\n",
        candidates.len()
    );
    println!(
        "{:>8}  {:>12}  {:>12}  {:>12}  {:>12}",
        "filters", "one by one", "DFA", "contiguous NFA", "noncontiguous"
    );
    let mut dfa_memory: Vec<(usize, usize, usize)> = Vec::new();

    for n in [1usize, 10, 100, 256, 1000, 4000] {
        let needles = build_needles(n);
        let finders: Vec<memmem::Finder> = needles
            .iter()
            .map(|nd| memmem::Finder::new(nd).into_owned())
            .collect();
        // The alphabet is 32 symbols over 51 bytes, so no byte is rare and a
        // prefilter fires on almost every candidate. Worth measuring without.
        let bare = AhoCorasick::builder()
            .prefilter(false)
            .kind(Some(AhoCorasickKind::ContiguousNFA))
            .build(&needles)
            .expect("needles are valid");
        let loose = AhoCorasick::builder()
            .prefilter(false)
            .kind(Some(AhoCorasickKind::NoncontiguousNFA))
            .build(&needles)
            .expect("needles are valid");
        let dfa = AhoCorasick::builder()
            .prefilter(false)
            .kind(Some(AhoCorasickKind::DFA))
            .build(&needles)
            .expect("needles are valid");

        // Both paths answer the same question: which filter matched, if any.
        let one_by_one = best(&candidates, |text| {
            finders.iter().position(|f| f.find(text).is_some())
        });
        let with_automaton = best(&candidates, |text| {
            loose.find(text).map(|m| m.pattern().as_usize())
        });
        let without_prefilter = best(&candidates, |text| {
            bare.find(text).map(|m| m.pattern().as_usize())
        });
        let with_dfa = best(&candidates, |text| {
            dfa.find(text).map(|m| m.pattern().as_usize())
        });
        dfa_memory.push((n, dfa.memory_usage(), bare.memory_usage()));
        println!(
            "{n:>8}  {:>9.1} ns  {:>9.1} ns  {:>9.1} ns  {:>9.1} ns",
            one_by_one, with_dfa, without_prefilter, with_automaton
        );
    }
    report_memory(&dfa_memory);
}

/// Nanoseconds per candidate, best of three runs.
fn best(candidates: &[[u8; 32]], mut probe: impl FnMut(&[u8]) -> Option<usize>) -> f64 {
    let mut text = [0u8; KEY_ONLY_SYMBOLS];
    let mut rate = f64::MAX;
    for _ in 0..3 {
        let start = Instant::now();
        let mut found = 0usize;
        for c in candidates {
            base32::encode_into(c, &mut text);
            if probe(&text).is_some() {
                found += 1;
            }
        }
        let per = start.elapsed().as_secs_f64() * 1e9 / candidates.len() as f64;
        // Keeps the loop from being optimised away.
        std::hint::black_box(found);
        if per < rate {
            rate = per;
        }
    }
    rate
}

fn build_candidates(n: usize) -> Vec<[u8; 32]> {
    let mut state = 0x243f_6a88_85a3_08d3u64;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let mut key = [0u8; 32];
        for chunk in key.chunks_mut(8) {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            chunk.copy_from_slice(&state.to_le_bytes()[..chunk.len()]);
        }
        out.push(key);
    }
    out
}

/// Distinct needles, so the automaton cannot collapse them.
fn build_needles(n: usize) -> Vec<Vec<u8>> {
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    (0..n)
        .map(|_| {
            (0..NEEDLE_LEN)
                .map(|_| {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    ALPHABET[(state % 32) as usize]
                })
                .collect()
        })
        .collect()
}

/// Printed after the table: the DFA trades memory for a flat search, and the
/// diagnostics have to be able to report how much.
fn report_memory(rows: &[(usize, usize, usize)]) {
    println!("\nmemory:          DFA   contiguous NFA");
    for (n, dfa, nfa) in rows {
        println!(
            "  {n:>6} filters  {:>8.1} KiB  {:>8.1} KiB",
            *dfa as f64 / 1024.0,
            *nfa as f64 / 1024.0
        );
    }
}
