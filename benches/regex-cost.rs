//! What supporting regular expressions would cost, measured against our own
//! matcher rather than against the reference implementation's.
//!
//! Three questions, in the order the change asks them:
//!
//! 1. Of expressions a person would actually write for a vanity address, how
//!    many carry a mandatory literal that our matcher could prefilter on?
//! 2. What does an expression without one cost, taking a regex engine as it
//!    comes?
//! 3. What does one with a literal cost, when the literal is caught by our
//!    substring matcher and the engine runs only on the survivors?
//!
//! Run with: cargo run --release --example regex-cost

use std::time::Instant;

use onion_gen::base32;
use onion_gen::filter::{FilterSet, KeyVerdict, KEY_ONLY_SYMBOLS};
use onion_gen::key;
use regex::bytes::Regex;

/// Expressions of the kind a person writes when hunting a vanity address,
/// together with the mandatory literal an engine could extract from each.
///
/// "Mandatory" means every string the expression matches contains it. An
/// alternation has one only if every branch shares it, which is why most of
/// them do not.
const EXPRESSIONS: &[(&str, Option<&str>)] = &[
    ("^shop", Some("shop")),
    ("^shop[0-9a-z]*", Some("shop")),
    ("shop", Some("shop")),
    ("^(shop|store)", None),
    ("^s[a-z]op", None),
    ("^sh.p", None),
    ("^shop.*store", Some("shop")),
    ("^[a-z]{4}shop", Some("shop")),
    ("shop$", Some("shop")),
    ("^shop[2-7]{2}", Some("shop")),
    ("^(a|b|c)[a-z]{3}", None),
    ("^[a-z]{2}[2-7]{2}[a-z]{2}", None),
    ("^zz+", Some("zz")),
    ("^(shop|shopping)", Some("shop")),
    ("^my(shop|store)", Some("my")),
    ("^[bcdfghjklmnpqrstvwxyz]{6}", None),
];

fn main() {
    let candidates = build_candidates(1 << 16);

    println!("== 1. How many expressions carry a prefilterable literal ==\n");
    let with = EXPRESSIONS.iter().filter(|(_, lit)| lit.is_some()).count();
    for (expr, lit) in EXPRESSIONS {
        println!(
            "  {:<34} {}",
            expr,
            match lit {
                Some(l) => format!("literal {l:?}"),
                None => "none".to_string(),
            }
        );
    }
    println!(
        "\n  {with} of {} carry one: {:.0}%\n",
        EXPRESSIONS.len(),
        100.0 * with as f64 / EXPRESSIONS.len() as f64
    );

    println!("== 2. Cost per candidate, single thread, best of three ==\n");

    let prefix = FilterSet::parse_all(["abcdefghij"]).unwrap();
    let substring = FilterSet::parse_all(["contains:shop"]).unwrap();
    let free = Regex::new("^[a-z]{2}[2-7]{2}[a-z]{2}").unwrap();
    let anchored = Regex::new("^shop[a-z]{3}").unwrap();
    let literal = Regex::new("shop").unwrap();

    // The floor: what these candidates cost our own matcher today.
    best("our prefix matcher, by bits", &candidates, |c, _| {
        prefix.match_key(c).is_some()
    });
    // The same question for a substring, which is what a regex literal would
    // be handed to.
    best("our substring matcher, by bits", &candidates, |c, _| {
        substring.match_key(c).is_some()
    });
    // A regex has to see text, and text costs a base32 encoding our path never
    // pays. Measured alone so the two costs can be told apart. Into a stack
    // buffer: a String per candidate would be measuring the allocator.
    best("base32 encoding alone", &candidates, |c, buf| {
        encode_key(c, buf);
        buf[0] == 0
    });
    best("encode + literal search", &candidates, |c, buf| {
        encode_key(c, buf);
        literal.is_match(buf)
    });
    best("encode + regex, no literal", &candidates, |c, buf| {
        encode_key(c, buf);
        free.is_match(buf)
    });
    best("encode + regex, with literal", &candidates, |c, buf| {
        encode_key(c, buf);
        anchored.is_match(buf)
    });
    best(
        "our substring prefilter, then regex",
        &candidates,
        |c, buf| {
            if substring.match_key(c).is_none() {
                return false;
            }
            encode_key(c, buf);
            anchored.is_match(buf)
        },
    );

    println!("\n== 3. What the built form costs, decided from the key ==\n");
    // The rows above measure the pieces. These measure the product: the same
    // entry point the run calls for a set it can settle without a checksum.
    for (label, spec) in [
        ("prefix, the floor", "abcdefghij"),
        ("regex, the same ten symbols", "regex:^abcdefghij"),
        ("regex, one literal", "regex:^shop"),
        ("regex, two literals", "regex:^my(shop|store)"),
        ("regex, 216 literals, shared head", "regex:^a[2-7]{3}shop"),
        ("regex, 216 literals, nothing shared", "regex:^[2-7]{3}shop"),
        ("regex, no literal", "regex:^[bcdfghjklmnp]{4}"),
    ] {
        let set = FilterSet::parse_all([spec]).expect("a valid filter");
        assert!(
            !set.needs_checksum(),
            "{spec} reaches the checksum, so it belongs in section 4"
        );
        best(label, &candidates, |c, _| set.match_key(c).is_some());
    }

    println!("\n== 4. What it costs when the checksum is unavoidable ==\n");
    // A section of its own, because these numbers are not comparable with the
    // ones above and one table would invite exactly that comparison: every
    // candidate that reaches the address here pays a SHA3-256 that no row in
    // section 3 pays.
    //
    // It exists because the first version of this example did not have it. The
    // substring row was measured through `match_key`, which never asks an
    // unanchored expression anything, and duly reported 3.8 ns for work that
    // was not being done.
    for (label, spec) in [
        ("regex, substring", "regex:zz[2-7]z"),
        ("regex, anchored, unbounded", "regex:^shop.*"),
        ("regex, end-anchored", "regex:^zz.*qd$"),
    ] {
        let set = FilterSet::parse_all([spec]).expect("a valid filter");
        // The assertion is the point: it is what makes the row below the path
        // the run really takes for this set, rather than a path this example
        // chose for it. `contains:shop` is not here for exactly that reason —
        // the run settles it through the cheaper checksum probe instead, so
        // measuring it this way would price work it never does.
        assert!(
            set.prefers_address(),
            "{spec} does not take the whole-address path, so this row would lie"
        );
        best(label, &candidates, |c, _| whole(&set, c));
    }
}

/// The matcher as the run drives it when the set needs the address.
///
/// `examine_key` first, and the address built only for a candidate the key
/// could not settle — which is the point of the arrangement, so it belongs
/// inside the measurement rather than around it.
fn whole(set: &FilterSet, packed: &[u8; 32]) -> bool {
    match set.examine_key(packed) {
        KeyVerdict::Matched(_) => true,
        KeyVerdict::No => false,
        KeyVerdict::NeedsAddress => set
            .match_whole_address(packed, &key::address_bytes(packed))
            .is_some(),
    }
}

/// Runs a per-candidate test three times and reports the best rate, so that a
/// cold walk over the candidate array does not become the measurement.
fn best(
    label: &str,
    candidates: &[[u8; 32]],
    mut f: impl FnMut(&[u8; 32], &mut [u8; KEY_ONLY_SYMBOLS]) -> bool,
) {
    let mut buf = [0u8; KEY_ONLY_SYMBOLS];
    let mut rate: f64 = 0.0;
    let mut hits = 0usize;
    for _ in 0..3 {
        let t = Instant::now();
        let mut found = 0usize;
        for c in candidates {
            if f(c, &mut buf) {
                found += 1;
            }
        }
        let r = candidates.len() as f64 / t.elapsed().as_secs_f64();
        if r > rate {
            rate = r;
        }
        hits = found;
    }
    println!(
        "  {label:<38} {:>8.1} M/s   {:>6.1} ns   (matched {hits})",
        rate / 1e6,
        1e9 / rate
    );
}

/// Random 32-byte keys. The matcher does not care whether they are on the
/// curve, and generating real ones would measure the engine instead.
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

/// The first 51 symbols: everything readable without the checksum, which is
/// what a regex would be run against to avoid a SHA3-256 per candidate.
fn encode_key(key: &[u8; 32], out: &mut [u8; KEY_ONLY_SYMBOLS]) {
    for (i, slot) in out.iter_mut().enumerate() {
        let bit = i * 5;
        let byte = bit / 8;
        let shift = bit % 8;
        let mut value = (key[byte] << shift) >> 3;
        if shift > 3 {
            value |= key[byte + 1] >> (11 - shift);
        }
        *slot = base32::ALPHABET[(value & 31) as usize];
    }
}
