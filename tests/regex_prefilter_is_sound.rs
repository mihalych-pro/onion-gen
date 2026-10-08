//! The prefilter accepts exactly what the expression accepts.
//!
//! This is the test the whole regular-expression change hangs on. The speed of
//! the form comes from extracting a literal and rejecting candidates by it
//! before the engine runs; if that literal is not truly obligatory, the run
//! silently stops finding keys it should have found. There is no symptom. The
//! rate stays the same, the output is merely emptier than it should be, and
//! the only way to notice is to already suspect it.
//!
//! So the comparison here is deliberately not "our fast path against our slow
//! path". Both sides of such a comparison share the extraction, which is the
//! very thing in doubt — that is exactly how a matching defect in this project
//! survived three changes. The right-hand side here is the expression applied
//! directly to the finished address, with nothing of ours in between.

use onion_gen::expression::{Expression, Haystack, Literals};
use onion_gen::filter::{FilterSet, KeyVerdict, KEY_ONLY_SYMBOLS};
use onion_gen::key::{self, ADDRESS_LEN};
use regex::bytes::RegexBuilder;

/// Expressions chosen to exercise the cases where extraction could go wrong:
/// alternations whose branches share nothing, optional and starred pieces that
/// make a literal non-obligatory, anchors at both ends, bounded and unbounded
/// repetition, and classes wide enough that no literal exists.
const EXPRESSIONS: &[&str] = &[
    "^shop",
    "^shop[a-z]{3}",
    "^shop.*",
    "^(shop|store)",
    "^my(shop|store)",
    "^(a|b|c)[a-z]{3}",
    "^s[a-z]op",
    "^sh.p",
    "^shop.*store",
    "^[a-z]{4}shop",
    "^zz+",
    "^(shop|shopping)",
    "^[bcdfghjklmnpqrstvwxyz]{6}",
    "^a[2-7]{3}shop",
    "^(ab)?cd",
    "^x*yz",
    "^(q|)rs",
    "^[a-z]{2}[2-7]{2}[a-z]{2}",
    "shop",
    "(cat|dog)food",
    "sh.p",
    "[2-7]{4}",
    "z{3}",
    "shop$",
    "^.*shop$",
    "[a-z]d$",
    "^s.*p$",
    "(?i)ABC",
    "^(a|ab|abc)x",
    "^..shop",
    "^[a-z]shop[2-7]",
    "qqq|www",
    "^(?:a[2-7]|b[2-7])c",
    "^\\w{4}",
    "^a.{10}z",
    // The boundary between the key text and the full address. Each of these is
    // anchored and bounded, which is what sends an expression down the cheap
    // path, and each also reads the end of the haystack, which is what must
    // pull it back off that path. Matched against 49 symbols instead of 56
    // they answer differently, so they are the only expressions here that can
    // tell a correct haystack choice from a wrong one.
    "^[a-z]{49}$",
    "^[a-z]{49}\\b",
    "^.{49}$",
    "^[a-z]{48}[2-7]$",
    "^\\w{49}\\b",
    "^[a-z]{20}$",
];

#[test]
fn the_prefilter_accepts_exactly_what_the_expression_accepts() {
    // A million in release, where this runs in a few seconds; fewer under a
    // debug build, where the engine is some fifty times slower and the test
    // would otherwise be skipped by whoever is waiting for it.
    let rounds = if cfg!(debug_assertions) {
        20_000
    } else {
        1_000_000
    };
    let addresses = random_addresses(rounds);

    let mut checked = 0usize;
    let mut agreed_positive = 0usize;

    for source in EXPRESSIONS {
        let expression = Expression::parse(source)
            .unwrap_or_else(|e| panic!("{source} should compile, got {e}"));
        // The ground truth: the same expression, compiled separately, applied
        // to the finished address with no prefilter in front of it.
        //
        // Built with the same Unicode setting as the product uses, because
        // otherwise it would be a different expression — `(?i)` alone means
        // two different things — and a comparison between two different
        // expressions proves nothing. What makes this side independent is
        // that the engine owes nothing to our literal extraction, not that it
        // is configured differently.
        let truth = RegexBuilder::new(source)
            .unicode(false)
            .build()
            .expect("ground truth compiles");

        let mut missed = Vec::new();
        let mut invented = Vec::new();

        for address in &addresses {
            let ours = guarded_match(&expression, address);
            let theirs = truth.is_match(address);
            if ours != theirs {
                if theirs {
                    missed.push(String::from_utf8_lossy(address).into_owned());
                } else {
                    invented.push(String::from_utf8_lossy(address).into_owned());
                }
            }
            checked += 1;
            if theirs {
                agreed_positive += usize::from(ours);
            }
        }

        assert!(
            missed.is_empty(),
            "{source} ({:?}, {:?}): the prefilter rejected {} address(es) the expression accepts, \
             first {:?}",
            expression.literals(),
            expression.haystack(),
            missed.len(),
            missed.first()
        );
        assert!(
            invented.is_empty(),
            "{source} ({:?}, {:?}): the prefilter accepted {} address(es) the expression rejects, \
             first {:?}",
            expression.literals(),
            expression.haystack(),
            invented.len(),
            invented.first()
        );
    }

    // A run where nothing ever matched would pass every assertion above while
    // proving nothing at all.
    assert!(
        agreed_positive > 0,
        "no expression matched anything in {checked} comparisons; the test proves nothing"
    );
}

/// What the product does: reject by the literal, then run the engine on the
/// text the expression asked for.
fn guarded_match(expression: &Expression, address: &[u8; ADDRESS_LEN]) -> bool {
    match expression.literals() {
        // Deliberately ignoring the common prefix the product also keeps:
        // this side is meant to be the plainest possible reading of what
        // `Literals` promises, so that the product's shortcuts are tested
        // against the promise rather than against themselves. The shortcuts
        // get their own test below, through the matcher that uses them.
        Literals::Prefix(words, _) => {
            if !words.iter().any(|w| address.starts_with(w)) {
                return false;
            }
        }
        Literals::Substring(words) => {
            if !words
                .iter()
                .any(|w| address.windows(w.len()).any(|s| s == w.as_slice()))
            {
                return false;
            }
        }
        Literals::None(_) => {}
    }
    let text: &[u8] = match expression.haystack() {
        Haystack::KeyText => &address[..KEY_ONLY_SYMBOLS],
        Haystack::FullAddress => address,
    };
    expression.find(text).is_some()
}

/// Random base32 text, not protocol-shaped addresses.
///
/// The protocol pins the last two symbols, which would make every `$`-anchored
/// expression see the same tail a million times over. Letting the tail vary
/// tests the thing that could break.
fn random_addresses(n: usize) -> Vec<[u8; ADDRESS_LEN]> {
    let mut state = 0x853c_49e6_748f_ea9bu64;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let mut address = [0u8; ADDRESS_LEN];
        for slot in address.iter_mut() {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            *slot = onion_gen::base32::ALPHABET[(state % 32) as usize];
        }
        // Every so often, plant a word an expression is looking for, so that
        // the positive side of the comparison is exercised rather than left to
        // a one-in-a-billion accident.
        if i % 7 == 0 {
            let seeds: [&[u8]; 6] = [b"shop", b"store", b"catfood", b"abc", b"zzz", b"myshop"];
            let seed = seeds[(state % 6) as usize];
            let at = (state as usize / 6) % (ADDRESS_LEN - seed.len());
            address[at..at + seed.len()].copy_from_slice(seed);
        }
        out.push(address);
    }
    out
}

/// The same comparison, but against the matcher the product actually runs.
///
/// The test above checks what `Literals` promises. This one checks that the
/// filter set keeps the promise while taking every shortcut it knows: reading
/// the literals off the packed key without encoding it, rejecting on the
/// prefix common to all of them first, and deciding from 49 symbols where it
/// can. Each of those is a place to lose a match, and none of them is visible
/// from the layer above.
#[test]
fn the_matcher_agrees_with_the_expression_applied_directly() {
    let rounds = if cfg!(debug_assertions) {
        5_000
    } else {
        200_000
    };
    let keys = random_keys(rounds);

    for source in EXPRESSIONS {
        let spec = format!("regex:{source}");
        let set = FilterSet::parse_all([&spec]).unwrap_or_else(|e| panic!("{spec}: {e:?}"));
        let truth = RegexBuilder::new(source)
            .unicode(false)
            .build()
            .expect("ground truth compiles");

        let mut missed = 0usize;
        let mut invented = 0usize;
        let mut positives = 0usize;
        let mut first_bad = None;

        for packed in &keys {
            // A random array stands in for a candidate and for the finished
            // key at once. That is sound as far as the matcher reads: the
            // first 49 symbols of the address are a function of the first 245
            // bits either way, and the checksum is computed from the array
            // itself on both sides of the comparison.
            let address = key::address(packed);
            let theirs = truth.is_match(address.as_bytes());
            let ours = match set.examine_key(packed) {
                KeyVerdict::Matched(_) => true,
                KeyVerdict::No => false,
                KeyVerdict::NeedsAddress => set
                    .match_whole_address(packed, &key::address_bytes(packed))
                    .is_some(),
            };
            if theirs {
                positives += 1;
            }
            if ours != theirs {
                if first_bad.is_none() {
                    first_bad = Some(address.clone());
                }
                if theirs {
                    missed += 1;
                } else {
                    invented += 1;
                }
            }
        }

        assert_eq!(
            (missed, invented),
            (0, 0),
            "{spec}: missed {missed} and invented {invented} of {positives} matches \
             in {rounds} keys, first disagreement at {first_bad:?}"
        );
    }
}

/// Random 32-byte arrays, with words the expressions look for planted in some
/// of them. Not points on the curve, which the matcher never checks and
/// generating would only make the test slower.
///
/// The planting is what gives the test its power. Left to chance, `^shop.*`
/// turns up once in a million addresses, so two hundred thousand keys carry
/// about a fifth of one match and a prefilter that rejected every candidate
/// would fail by a margin of one. Measured against a deliberately broken gate,
/// that is the difference between "missed 1 of 1" and a number that cannot be
/// mistaken for noise.
fn random_keys(n: usize) -> Vec<[u8; 32]> {
    const SEEDS: [&[u8]; 6] = [b"shop", b"store", b"zz3z", b"abc", b"myshop", b"catfood"];
    let mut state = 0x2545_f491_4f6c_dd1du64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let mut key = [0u8; 32];
        for chunk in key.chunks_mut(8) {
            chunk.copy_from_slice(&next().to_le_bytes()[..chunk.len()]);
        }
        if i % 5 == 0 {
            let word = SEEDS[(next() % 6) as usize];
            // Only where the key settles the symbol exactly. Symbol 49 carries
            // the deferred sign bit and symbols past it come from the
            // checksum, so planting there would write one thing and read back
            // another.
            let room = KEY_ONLY_SYMBOLS - word.len();
            let at = if i % 10 == 0 {
                0
            } else {
                (next() as usize) % room
            };
            for (j, &symbol) in word.iter().enumerate() {
                let value = onion_gen::base32::symbol_value(symbol).expect("base32");
                set_symbol(&mut key, at + j, value);
            }
        }
        out.push(key);
    }
    out
}

/// Writes one base32 symbol into the packed key: the inverse of the read the
/// matcher does.
///
/// Five bits never span more than two bytes, which is what makes a sixteen-bit
/// window enough.
fn set_symbol(key: &mut [u8; 32], i: usize, value: u8) {
    assert!(i < KEY_ONLY_SYMBOLS, "symbol {i} is not settled by the key");
    let bit = i * 5;
    let byte = bit / 8;
    let shift = bit % 8;
    let hi = u16::from(key[byte]) << 8;
    let lo = u16::from(key.get(byte + 1).copied().unwrap_or(0));
    let pos = 11 - shift;
    let window = ((hi | lo) & !(0x1fu16 << pos)) | (u16::from(value & 0x1f) << pos);
    key[byte] = (window >> 8) as u8;
    if byte + 1 < key.len() {
        key[byte + 1] = window as u8;
    }
}

/// The planting has to survive the round trip, or the test would be measuring
/// its own fixture rather than the matcher.
#[test]
fn a_planted_symbol_reads_back() {
    let mut key = [0x5au8; 32];
    for at in [0usize, 1, 3, 7, 8, 12, 47, 48] {
        for value in 0..32u8 {
            set_symbol(&mut key, at, value);
            let address = key::address(&key);
            let read = onion_gen::base32::symbol_value(address.as_bytes()[at]).expect("base32");
            assert_eq!(
                read, value,
                "symbol {at} planted as {value} read back as {read}"
            );
        }
    }
}
