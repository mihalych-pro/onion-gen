//! A regular expression finds what it should, through the run's own dispatch.
//!
//! The matcher has a test of its own that compares its answers with the
//! expression applied to the finished address. This one exists because that
//! test proved not to be enough: it calls the matching entry points directly,
//! and the run does not. The run first decides, once per batch, which of four
//! loops to walk — prefix-only, general forms, checksum, whole address — and a
//! form the decision does not know about is routed into a loop that never asks
//! for it. That is exactly what happened: the expression matched perfectly
//! every time it was asked, and a full run over three billion candidates found
//! nothing, because it was never asked.
//!
//! So the entry point here is `run::run`, the same one the binary calls, and
//! the check is on the keys it actually wrote.

use onion_gen::filter::FilterSet;
use onion_gen::run::{self, RunOptions};
use regex::bytes::RegexBuilder;
use std::fs;
use std::path::PathBuf;

fn temp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "onion-gen-regex-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ))
}

/// Runs until `wanted` keys are written, then returns the addresses found.
fn addresses_for(spec: &str, wanted: u64) -> Vec<String> {
    let out = temp(&spec.replace(
        [
            '^', '$', '[', ']', '{', '}', '*', '.', '|', '(', ')', ':', '\\', '/',
        ],
        "_",
    ));
    let _ = fs::remove_dir_all(&out);

    let filters = FilterSet::parse_all([spec]).expect("a valid filter");
    let mut opts = RunOptions::new(out.clone());
    opts.threads = 2;
    opts.limit = Some(wanted);
    opts.print_addresses = false;
    opts.explain_devices = false;
    opts.root_seed = [0x17u8; 32];

    let summary = run::run(&opts, &filters).expect("the run");
    assert_eq!(
        summary.hits, wanted,
        "{spec}: the run ended with {} hits instead of {wanted}",
        summary.hits
    );

    let found: Vec<String> = fs::read_dir(&out)
        .expect("an output directory")
        .map(|e| {
            e.expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|name| name.ends_with(".onion"))
        .collect();
    let _ = fs::remove_dir_all(&out);
    assert_eq!(
        found.len() as u64,
        wanted,
        "{spec}: wrote {} keys",
        found.len()
    );
    found
}

/// Every address written must satisfy the expression, checked against a
/// separately compiled copy of it rather than against anything of ours.
fn assert_every_address_matches(spec: &str, wanted: u64) {
    let source = spec.strip_prefix("regex:").expect("a regex filter");
    let truth = RegexBuilder::new(source)
        .unicode(false)
        .build()
        .expect("the expression compiles");
    for name in addresses_for(spec, wanted) {
        let address = name.trim_end_matches(".onion");
        assert!(
            truth.is_match(address.as_bytes()),
            "{spec}: wrote {address}, which the expression does not match"
        );
    }
}

/// The cheap path: anchored, with obligatory literals, decided from the key.
#[test]
fn an_anchored_expression_finds_matching_addresses() {
    assert_every_address_matches("regex:^ab[2-7]", 3);
}

/// The substring path: unanchored, so the literal has no fixed position and
/// the match may land anywhere in the address.
#[test]
fn an_unanchored_expression_finds_matching_addresses() {
    assert_every_address_matches("regex:zz[2-7]z", 2);
}

/// The expensive path: `$` reaches the checksum symbols, so every candidate
/// that gets this far costs a SHA3-256.
///
/// `qd$` rather than any other tail: the protocol fixes the last symbol at `d`
/// and allows only `a`, `i`, `q`, `y` before it, so most tails would search
/// forever.
#[test]
fn an_end_anchored_expression_finds_matching_addresses() {
    assert_every_address_matches("regex:^zz.*qd$", 2);
}

/// A class wide enough that no literal can be extracted, so the engine sees
/// every candidate and nothing guards it.
#[test]
fn an_expression_without_a_literal_finds_matching_addresses() {
    assert_every_address_matches("regex:^[bcdfghjklmnp]{4}", 3);
}

/// An expression beside ordinary prefixes. The set then holds two kinds at
/// once, and the run must ask for both.
///
/// One thread and a fixed root seed, so the run is the same every time: with
/// two threads the blocks are claimed in whatever order they are reached, and
/// which filter fills the quota first becomes a coin toss. The two filters are
/// also chosen to be of comparable rarity — `zz` at one in 1024 against
/// `^ab[2-7]` at one in 5461 — because an earlier pairing let the commoner one
/// take all twelve slots often enough to fail on its own.
#[test]
fn a_mixed_set_finds_both_kinds() {
    let out = temp("mixed");
    let _ = fs::remove_dir_all(&out);
    let filters = FilterSet::parse_all(["zz", "regex:^ab[2-7]"]).expect("valid filters");
    let mut opts = RunOptions::new(out.clone());
    opts.threads = 1;
    opts.limit = Some(12);
    opts.print_addresses = false;
    opts.explain_devices = false;
    opts.root_seed = [0x29u8; 32];

    run::run(&opts, &filters).expect("the run");

    let truth = RegexBuilder::new("^ab[2-7]")
        .unicode(false)
        .build()
        .expect("compiles");
    let mut by_prefix = 0;
    let mut by_regex = 0;
    for entry in fs::read_dir(&out).expect("an output directory") {
        let name = entry
            .expect("an entry")
            .file_name()
            .to_string_lossy()
            .into_owned();
        let Some(address) = name.strip_suffix(".onion") else {
            continue;
        };
        if address.starts_with("zz") {
            by_prefix += 1;
        } else if truth.is_match(address.as_bytes()) {
            by_regex += 1;
        } else {
            panic!("wrote {address}, which satisfies neither filter");
        }
    }
    let _ = fs::remove_dir_all(&out);
    assert!(
        by_prefix > 0,
        "the prefix found nothing: {by_prefix}/{by_regex}"
    );
    assert!(
        by_regex > 0,
        "the expression found nothing: {by_prefix}/{by_regex}"
    );
}
