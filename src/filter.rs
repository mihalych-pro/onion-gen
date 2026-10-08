//! Parsing, validating and matching address filters.
//!
//! This module holds the reference matcher: a straightforward scan that is
//! obviously correct. The indexed structures are checked against it, so the
//! scan stays the oracle even though it is not the fast path.

use crate::base32;
use crate::expression::{Expression, ExpressionError, Haystack, Literals};
use crate::key::ADDRESS_LEN;
use std::fmt;
use std::path::Path;

/// Why a filter was rejected.
///
/// An unreachable filter must never be accepted: the search would run forever
/// without a possible result. The reference implementation only enforces this
/// for two of its three filter families, which is a trap worth not repeating.
#[derive(Debug, PartialEq, Eq)]
pub enum FilterError {
    Empty,
    TooLong {
        length: usize,
    },
    BadSymbol {
        position: usize,
        character: char,
    },
    /// A form name before the colon that does not exist.
    UnknownForm {
        name: String,
    },
    /// `[` without a `]`, or the other way round.
    UnbalancedClass {
        position: usize,
    },
    /// `[]` — a class no symbol can satisfy.
    EmptyClass {
        position: usize,
    },
    /// A range whose end precedes its start, such as `[e-b]`: it can match
    /// nothing, and is far likelier a typo than an intent.
    EmptyRange {
        position: usize,
    },
    /// A pattern of nothing but wildcards: it matches every address, which is
    /// never what was meant.
    NothingFixed,
    /// The body of a `regex:` filter did not survive its own parsing.
    Expression(ExpressionError),
    /// A suffix the protocol cannot produce.
    ///
    /// The version byte fixes the last symbol at `d` and limits the one before
    /// it to four values, so most suffixes are unreachable. Accepting one would
    /// mean searching forever.
    UnreachableTail {
        position_from_end: usize,
        character: char,
        allowed: String,
    },
}

impl fmt::Display for FilterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FilterError::Empty => write!(f, "the filter is empty"),
            FilterError::TooLong { length } => write!(
                f,
                "the filter is {length} symbols long, an address has only {ADDRESS_LEN}"
            ),
            FilterError::BadSymbol {
                position,
                character,
            } => write!(
                f,
                "invalid character {character:?} at position {position}: \
                 the base32 alphabet is a-z and 2-7"
            ),
            FilterError::UnknownForm { name } => write!(
                f,
                "unknown form {name:?}: expected prefix, contains, suffix or regex"
            ),
            FilterError::Expression(e) => write!(f, "{e}"),
            FilterError::UnbalancedClass { position } => {
                write!(f, "unbalanced [ or ] at position {position}")
            }
            FilterError::EmptyClass { position } => {
                write!(f, "empty character class at position {position}")
            }
            FilterError::EmptyRange { position } => write!(
                f,
                "the range at position {position} ends before it starts; \
                 the alphabet runs a-z then 2-7"
            ),
            FilterError::NothingFixed => write!(
                f,
                "the pattern fixes no symbol, so every address would match it"
            ),
            FilterError::UnreachableTail {
                position_from_end,
                character,
                allowed,
            } => write!(
                f,
                "no address can end this way: {position_from_end} symbols from \
                 the end must be one of {allowed}, not {character:?}. The \
                 version byte of an Onion Service v3 address fixes its tail"
            ),
        }
    }
}

impl std::error::Error for FilterError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            FilterError::Expression(e) => Some(e),
            _ => None,
        }
    }
}

/// The number of leading address symbols the hot loop can read straight off a
/// candidate.
///
/// The address encodes `pubkey || checksum || version`, so its first 51 symbols
/// come from the key alone and symbol 52 already mixes in a checksum bit.
/// Matching within that span needs no SHA3-256 and no base32 string, which is
/// what keeps the hot loop cheap.
///
/// The limit is 49 rather than 51 because the engine defers the ed25519 sign
/// bit, leaving it clear in the packed key. In base32 order that is bit 248,
/// which falls inside symbol 49 and is worth two there — so symbol 49 of a
/// candidate is its address symbol with the value-2 bit forced off. Symbol 50
/// is clean, bits 250 to 254, but the span has to be contiguous.
pub const KEY_ONLY_SYMBOLS: usize = 49;

/// How many leading symbols the address takes from the public key at all.
///
/// Fifty-one, because the key is 255 bits of the encoding and symbol 52 already
/// carries a checksum bit. Two of them — 49 and 50 — the hot loop cannot read,
/// which is what [`KEY_ONLY_SYMBOLS`] is about; a prefilter may still use them
/// as long as it treats the deferred sign bit as unknown.
pub const KEY_DERIVED_SYMBOLS: usize = 51;

/// The symbol the deferred sign bit lands in.
///
/// The sign bit is the top bit of the last key byte, which base32 numbers as
/// bit 248; symbol 49 spans bits 245 to 249, so within it the bit is worth two.
/// That is where [`either_sign`] gets its shift of two from.
const SIGN_SYMBOL: usize = 49;

/// The same set of symbols, plus every symbol that differs from one of them
/// only in the deferred sign bit.
///
/// A prefilter reading symbol 49 of a candidate sees that symbol with the sign
/// bit forced off. Widening the set it accepts keeps it from ruling out an
/// address that the finished key would have matched — a prefilter is allowed to
/// say "maybe" too often and is never allowed to say "no" wrongly.
fn either_sign(set: SymbolSet) -> SymbolSet {
    // Values with the bit clear sit at positions 0 and 1 of every four, values
    // with it set at 2 and 3.
    const CLEAR: u32 = 0x3333_3333;
    const SET: u32 = 0xcccc_cccc;
    SymbolSet(set.0 | ((set.0 & CLEAR) << 2) | ((set.0 & SET) >> 2))
}

/// Where in the address a pattern has to sit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Anchor {
    /// From the first symbol. The default, and what a bare filter means.
    Start,
    /// Anywhere.
    Anywhere,
    /// Ending at the last symbol.
    End,
}

impl Anchor {
    /// The name used on the command line and in diagnostics.
    pub fn name(self) -> &'static str {
        match self {
            Anchor::Start => "prefix",
            Anchor::Anywhere => "contains",
            Anchor::End => "suffix",
        }
    }
}

/// The set of symbols acceptable at one position, as a bit per base32 symbol.
///
/// A fixed symbol sets one bit, `?` sets all thirty-two, `[abc]` sets three.
/// Keeping every case in one representation means the matcher has one shape
/// rather than a branch per form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SymbolSet(u32);

impl SymbolSet {
    const ALL: SymbolSet = SymbolSet(u32::MAX);

    fn single(value: u8) -> SymbolSet {
        SymbolSet(1 << value)
    }

    fn is_single(self) -> bool {
        self.0.count_ones() == 1
    }

    fn contains(self, value: u8) -> bool {
        self.0 & (1 << value) != 0
    }

    fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// What the protocol itself permits at one symbol position.
///
/// Symbols 0 to 53 are free: the first 51 come straight from the public key and
/// the next three from the checksum, and both are uniform over the alphabet.
/// The last two are not. The version byte is `0x03`, and base32 packs it into
/// the final ten bits: the last symbol is always `d`, and the one before it
/// keeps only two free bits, so four values of thirty-two.
///
/// This is what makes an estimate of search cost honest. Counting the tail as
/// free would understate a six-symbol suffix by a factor of 256.
fn protocol_allows(position: usize) -> SymbolSet {
    if position + 1 == ADDRESS_LEN {
        SymbolSet::single(value_of(TAIL_LAST))
    } else if position + 2 == ADDRESS_LEN {
        let mut set = 0u32;
        for symbol in TAIL_PENULTIMATE {
            set |= 1 << value_of(symbol);
        }
        SymbolSet(set)
    } else {
        SymbolSet::ALL
    }
}

/// The alphabet index of a base32 symbol.
fn value_of(symbol: u8) -> u8 {
    base32::ALPHABET
        .iter()
        .position(|c| *c == symbol)
        .expect("a symbol of the alphabet") as u8
}

/// The last symbol of every v3 address, fixed by the version byte.
const TAIL_LAST: u8 = b'd';
/// The only values the penultimate symbol can take.
const TAIL_PENULTIMATE: [u8; 4] = *b"aiqy";

/// Extracts the five bits of symbol `position` from a big-endian bit string.
///
/// The address is base32 of `pubkey || checksum || version`, most significant
/// bit first, so symbol `m` is bits `[5m, 5m+5)`. Reading them straight out of
/// the packed key is what keeps matching free of base32 encoding.
#[inline]
pub fn symbol_at(bytes: &[u8], position: usize) -> u8 {
    let bit = position * 5;
    let byte = bit / 8;
    let shift = bit % 8;
    // Five bits never span more than two bytes.
    let hi = u16::from(bytes[byte]) << 8;
    let lo = u16::from(*bytes.get(byte + 1).unwrap_or(&0));
    (((hi | lo) >> (11 - shift)) & 0x1f) as u8
}

/// A parsed pattern: what each position accepts, and where it sits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern {
    pub anchor: Anchor,
    pub symbols: Vec<SymbolSet>,
    /// Text as written, for reporting.
    pub text: String,
    /// One character per position: the lowest symbol each position accepts.
    ///
    /// For a literal it is the filter itself, which is what sorting,
    /// absorption and the length buckets work on. It lives here rather than in
    /// [`Filter`] because nothing in the hot loop reads it, and a string in
    /// `Filter` would be 24 bytes of every entry a dictionary scan walks past.
    pub canonical: String,
    /// The compiled expression, for the one form that is not a symbol set.
    ///
    /// It lives here rather than in [`Filter`] for the same reason the pattern
    /// itself does: `Pattern` is already behind a box that the hot loop never
    /// follows, so a set with no expression in it carries no extra byte per
    /// entry in the vector a dictionary scan walks.
    pub regex: Option<Box<Expression>>,
}

impl Pattern {
    /// Parses one filter.
    ///
    /// A bare filter is a prefix, which is what it has always been; the other
    /// forms are named explicitly. Wildcards and classes work inside any form,
    /// so there is no separate "pattern" form to remember.
    pub fn parse(raw: &str) -> Result<Pattern, FilterError> {
        let trimmed = raw.trim();
        // Taken before the symbol parser sees it: an expression is not a
        // sequence of symbol sets and shares none of the checks below. The
        // split is on the first colon only, so `regex:^(?:ab)c` keeps its own.
        if let Some(body) = strip_form(trimmed, crate::expression::FORM) {
            return Pattern::parse_regex(trimmed, body);
        }
        let (anchor, body) = match trimmed.split_once(':') {
            Some((name, rest)) => {
                let anchor = match name.trim().to_ascii_lowercase().as_str() {
                    "prefix" => Anchor::Start,
                    "contains" => Anchor::Anywhere,
                    "suffix" => Anchor::End,
                    other => {
                        return Err(FilterError::UnknownForm {
                            name: other.to_string(),
                        })
                    }
                };
                (anchor, rest.trim())
            }
            // A leading '^' is accepted and dropped: people used to regex write
            // it, and a prefix match is what it means anyway.
            None => (Anchor::Start, trimmed.trim_start_matches('^')),
        };

        let symbols = parse_symbols(body)?;
        if symbols.is_empty() {
            return Err(FilterError::Empty);
        }
        if symbols.len() > ADDRESS_LEN {
            return Err(FilterError::TooLong {
                length: symbols.len(),
            });
        }
        if symbols.iter().all(|s| *s == SymbolSet::ALL) {
            return Err(FilterError::NothingFixed);
        }
        check_reachable_tail(anchor, &symbols)?;

        let canonical = symbols
            .iter()
            .map(|set| describe(*set))
            .collect::<String>()
            .to_ascii_lowercase();
        Ok(Pattern {
            anchor,
            symbols,
            text: trimmed.to_string(),
            canonical,
            regex: None,
        })
    }

    /// Builds the pattern for a `regex:` filter.
    ///
    /// `symbols` stays empty, which every method that reads it has to account
    /// for: an empty symbol list would otherwise read as "matches everywhere",
    /// since `all()` over nothing is true. The guards are on
    /// [`Pattern::is_regex`] rather than on the emptiness, so that the reason
    /// is legible where it is enforced.
    fn parse_regex(raw: &str, body: &str) -> Result<Pattern, FilterError> {
        let expression = Expression::parse(body).map_err(FilterError::Expression)?;
        let anchor = match expression.literals() {
            Literals::Prefix(..) => Anchor::Start,
            _ => Anchor::Anywhere,
        };
        Ok(Pattern {
            anchor,
            symbols: Vec::new(),
            text: raw.to_string(),
            // Not a base32 string, and nothing that decodes it may reach here.
            canonical: expression.source().to_string(),
            regex: Some(Box::new(expression)),
        })
    }

    /// Whether this pattern is a regular expression rather than a symbol set.
    pub fn is_regex(&self) -> bool {
        self.regex.is_some()
    }

    /// The compiled expression, when there is one.
    pub fn expression(&self) -> Option<&Expression> {
        self.regex.as_deref()
    }

    /// How many symbols the pattern covers.
    pub fn len(&self) -> usize {
        self.symbols.len()
    }

    pub fn is_empty(&self) -> bool {
        self.symbols.is_empty()
    }

    /// Whether every position accepts exactly one symbol.
    ///
    /// Such a pattern can take the byte-comparison path, which is why plain
    /// prefixes keep costing what they cost today.
    pub fn is_literal(&self) -> bool {
        // An expression has no symbols, and `all()` over none is true, so
        // without this guard every expression would claim to be a literal
        // prefix and be routed onto the byte-comparison path, where its empty
        // `bytes` would be compared against the key.
        !self.is_regex() && self.symbols.iter().all(|s| s.is_single())
    }

    /// The positions in the address this pattern could occupy.
    pub fn candidate_offsets(&self) -> Vec<usize> {
        match self.anchor {
            Anchor::Start => vec![0],
            Anchor::End => vec![ADDRESS_LEN - self.symbols.len()],
            Anchor::Anywhere => (0..=ADDRESS_LEN - self.symbols.len()).collect(),
        }
    }

    /// The share of addresses this pattern matches.
    ///
    /// Per position it is "how many of the symbols this filter accepts are ones
    /// the protocol can produce there", over "how many the protocol can produce
    /// there" — which is why the tail costs nothing extra for a suffix that is
    /// reachable at all, and why an unreachable one comes out as zero rather
    /// than as a small number.
    ///
    /// For a substring the placements are summed rather than combined exactly.
    /// They overlap, so the sum is an upper bound; with terms this small the
    /// difference is far below what a run would ever notice.
    pub fn probability(&self) -> f64 {
        // An expression's share is not computable from its text without
        // enumerating what it accepts. Reporting zero would say "this can
        // never match", which is a different and false claim, so the set
        // carries the fact separately and the estimate says it is a bound.
        if self.is_regex() {
            return 0.0;
        }
        let total: f64 = self
            .candidate_offsets()
            .into_iter()
            .map(|start| self.probability_at(start))
            .sum();
        total.min(1.0)
    }

    fn probability_at(&self, start: usize) -> f64 {
        let mut p = 1.0f64;
        for (i, wanted) in self.symbols.iter().enumerate() {
            let allowed = protocol_allows(start + i);
            let room = allowed.0.count_ones();
            let hits = (wanted.0 & allowed.0).count_ones();
            p *= f64::from(hits) / f64::from(room);
        }
        p
    }

    /// Whether any placement of this pattern reaches symbols the checksum
    /// produces.
    ///
    /// The first 51 symbols of an address are a pure function of the public
    /// key; anything past them costs a SHA3-256 per candidate, which is why the
    /// distinction is worth carrying around.
    ///
    /// For a substring this is true because of its last few offsets only — the
    /// rest are checkable for free, so the flag means "some placements need it",
    /// not "this filter is unusable without it".
    pub fn needs_checksum(&self) -> bool {
        if let Some(expression) = self.expression() {
            return expression.needs_checksum();
        }
        self.candidate_offsets()
            .iter()
            .any(|start| start + self.symbols.len() > KEY_ONLY_SYMBOLS)
    }

    /// Whether *every* placement needs the checksum, so the filter can find
    /// nothing until it is computed.
    pub fn requires_checksum(&self) -> bool {
        if let Some(expression) = self.expression() {
            return expression.needs_checksum();
        }
        self.candidate_offsets()
            .iter()
            .all(|start| start + self.symbols.len() > KEY_ONLY_SYMBOLS)
    }
}

/// Splits `name:` off the front of a filter, if that is the form named.
///
/// Only the first colon is consumed, so the body keeps any of its own — which
/// a regular expression regularly has, in `(?:...)`.
fn strip_form<'a>(raw: &'a str, name: &str) -> Option<&'a str> {
    let (head, rest) = raw.split_once(':')?;
    head.trim().eq_ignore_ascii_case(name).then(|| rest.trim())
}

/// Parses the body of a filter into one symbol set per position.
fn parse_symbols(body: &str) -> Result<Vec<SymbolSet>, FilterError> {
    let bytes = body.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0usize;

    while i < bytes.len() {
        match bytes[i] {
            b'?' => {
                out.push(SymbolSet::ALL);
                i += 1;
            }
            b'[' => {
                let start = i;
                let end = body[i..]
                    .find(']')
                    .map(|offset| i + offset)
                    .ok_or(FilterError::UnbalancedClass { position: start })?;
                let mut set = SymbolSet(0);
                let class = &bytes[i + 1..end];
                let symbol_at_class = |k: usize| -> Result<u8, FilterError> {
                    base32::symbol_value(class[k].to_ascii_lowercase()).ok_or(
                        FilterError::BadSymbol {
                            position: i + 1 + k,
                            character: class[k] as char,
                        },
                    )
                };
                let mut j = 0usize;
                while j < class.len() {
                    let first = symbol_at_class(j)?;
                    // `b-e` spans the alphabet between its ends: written out it
                    // is `bcde`. Ranges exist because the useful ones, `a-z`
                    // and `2-7`, are tedious and error-prone to spell.
                    if j + 2 < class.len() && class[j + 1] == b'-' {
                        let last = symbol_at_class(j + 2)?;
                        if last < first {
                            return Err(FilterError::EmptyRange {
                                position: i + 1 + j,
                            });
                        }
                        for value in first..=last {
                            set.0 |= SymbolSet::single(value).0;
                        }
                        j += 3;
                    } else {
                        set.0 |= SymbolSet::single(first).0;
                        j += 1;
                    }
                }
                if set.is_empty() {
                    return Err(FilterError::EmptyClass { position: start });
                }
                out.push(set);
                i = end + 1;
            }
            b']' => return Err(FilterError::UnbalancedClass { position: i }),
            c => {
                let value =
                    base32::symbol_value(c.to_ascii_lowercase()).ok_or(FilterError::BadSymbol {
                        position: i,
                        character: c as char,
                    })?;
                out.push(SymbolSet::single(value));
                i += 1;
            }
        }
    }
    Ok(out)
}

/// Rejects a pattern that reaches the address tail with something the protocol
/// cannot produce.
///
/// This is the check the reference implementation lacks in its regex mode,
/// where an impossible filter is accepted in silence and searched for forever.
fn check_reachable_tail(anchor: Anchor, symbols: &[SymbolSet]) -> Result<(), FilterError> {
    if anchor != Anchor::End {
        return Ok(());
    }
    let n = symbols.len();

    let last = symbols[n - 1];
    let last_value = base32::symbol_value(TAIL_LAST).expect("d is in the alphabet");
    if !last.contains(last_value) {
        return Err(FilterError::UnreachableTail {
            position_from_end: 1,
            character: describe(last),
            allowed: format!("{:?}", TAIL_LAST as char),
        });
    }

    if n >= 2 {
        let penultimate = symbols[n - 2];
        let ok = TAIL_PENULTIMATE
            .iter()
            .any(|c| penultimate.contains(base32::symbol_value(*c).expect("in the alphabet")));
        if !ok {
            let allowed: String = TAIL_PENULTIMATE.iter().map(|c| *c as char).collect();
            return Err(FilterError::UnreachableTail {
                position_from_end: 2,
                character: describe(penultimate),
                allowed: format!("{allowed:?}"),
            });
        }
    }
    Ok(())
}

/// A readable stand-in for a symbol set in an error message.
fn describe(set: SymbolSet) -> char {
    (0u8..32)
        .find(|v| set.contains(*v))
        .map(|v| base32::ALPHABET[v as usize] as char)
        .unwrap_or('?')
}

/// A validated address prefix.
///
/// Two representations are kept: the text, for reporting and for the reference
/// matcher, and the decoded bytes with a trailing-bit mask, which is what the
/// hot loop compares against a packed key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filter {
    /// Decoded bytes of the first `min(len, KEY_ONLY_SYMBOLS)` symbols.
    ///
    /// Only meaningful for a literal prefix, which is the form that takes the
    /// byte-comparison path.
    bytes: Vec<u8>,
    /// Which bits of the final decoded byte are significant.
    mask: u8,
    /// The parsed form. A literal prefix carries one too, so that reporting and
    /// the general matcher have a single thing to look at.
    ///
    /// Boxed because the hot path does not read it: matching a literal prefix
    /// touches `bytes` and `mask` only, and the sorted lookup reaches into this
    /// struct on every bitmap hit. Inline, the pattern doubled the struct and
    /// cost about 1% of throughput to sets that never use a general form.
    pattern: Box<Pattern>,
}

impl PartialOrd for Filter {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Filter {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.pattern.canonical.cmp(&other.pattern.canonical)
    }
}

impl Filter {
    /// Parses one filter, rejecting anything no address could ever match.
    pub fn parse(raw: &str) -> Result<Self, FilterError> {
        let pattern = Pattern::parse(raw)?;
        // An expression's canonical form is its own text, which is not base32
        // and must never reach the decoder below.
        if pattern.is_regex() {
            return Ok(Filter {
                bytes: Vec::new(),
                mask: 0,
                pattern: Box::new(pattern),
            });
        }
        let text = pattern.canonical.clone();
        // Only the key-derived span is precompiled; a longer filter is rare
        // enough that re-checking it against the full address costs nothing.
        let head: String = text.chars().take(KEY_ONLY_SYMBOLS).collect();
        let (mut bytes, bits) = base32::decode_bits(&head).expect("validated above");
        let significant = bits.div_ceil(8);
        bytes.truncate(significant);
        let remainder = bits % 8;
        let mask = if remainder == 0 {
            0xff
        } else {
            0xffu8 << (8 - remainder)
        };
        // Clear the padding bits so a byte comparison needs no masking on the
        // filter side.
        if let Some(last) = bytes.last_mut() {
            *last &= mask;
        }

        Ok(Filter {
            bytes,
            mask,
            pattern: Box::new(pattern),
        })
    }

    /// The parsed form.
    pub fn pattern(&self) -> &Pattern {
        &self.pattern
    }

    /// Whether this filter takes the byte-comparison path.
    ///
    /// Only a literal prefix does. Everything else goes through the general
    /// matcher, which is kept out of the way of sets that do not use it — the
    /// requirement that prefix-only searches must not slow down.
    pub fn is_fast(&self) -> bool {
        self.pattern.anchor == Anchor::Start
            && self.pattern.is_literal()
            && !self.pattern.needs_checksum()
    }

    /// Whether checking this filter needs the checksum symbols.
    pub fn needs_checksum(&self) -> bool {
        self.pattern.needs_checksum()
    }

    /// Where this filter matches, if it does: the first offset in symbols.
    ///
    /// Reported for a form that is not anchored to the start, because "it
    /// matched" says little when the form could have matched anywhere.
    pub fn match_offset(&self, bytes: &[u8], have_checksum: bool) -> Option<usize> {
        if let Some(expression) = self.pattern.expression() {
            return offset_of_expression(expression, bytes, have_checksum);
        }
        let reach = if have_checksum {
            ADDRESS_LEN
        } else {
            KEY_ONLY_SYMBOLS
        };
        self.pattern
            .candidate_offsets()
            .into_iter()
            .filter(|start| start + self.pattern.symbols.len() <= reach)
            .find(|&start| {
                self.pattern
                    .symbols
                    .iter()
                    .enumerate()
                    .all(|(i, set)| set.contains(symbol_at(bytes, start + i)))
            })
    }

    /// Whether this filter is anchored to the start of the address, where its
    /// position carries no information.
    pub fn is_anchored_at_start(&self) -> bool {
        self.pattern.anchor == Anchor::Start
    }

    /// The general matcher: compares symbol sets at every offset the form
    /// allows.
    ///
    /// `bytes` may be the 32-byte packed key, in which case only positions
    /// below [`KEY_ONLY_SYMBOLS`] are readable, or the 35 encoded address bytes.
    pub fn matches_symbols(&self, bytes: &[u8], have_checksum: bool) -> bool {
        if let Some(expression) = self.pattern.expression() {
            return match_expression(expression, bytes, have_checksum);
        }
        let reach = if have_checksum {
            ADDRESS_LEN
        } else {
            KEY_ONLY_SYMBOLS
        };
        for start in self.pattern.candidate_offsets() {
            if start + self.pattern.symbols.len() > reach {
                continue;
            }
            if self
                .pattern
                .symbols
                .iter()
                .enumerate()
                .all(|(i, set)| set.contains(symbol_at(bytes, start + i)))
            {
                return true;
            }
        }
        false
    }

    /// Whether the key leaves any placement of this form still possible.
    ///
    /// Each placement is checked only as far as the key reaches. A placement
    /// that lies wholly past [`KEY_ONLY_SYMBOLS`] has nothing to check and so
    /// stays possible; one that starts inside the key is usually killed here,
    /// before a hash is paid for.
    ///
    /// This is a prefilter, so it reads two symbols further than the exact path
    /// can — the whole key-derived span — and widens symbol 49 to cover both
    /// values of the deferred sign bit. Stopping at 48 instead would have been
    /// correct too, and would have sent every eight-symbol suffix to the hash.
    #[inline]
    pub fn key_prefilter(&self, packed: &[u8; 32]) -> bool {
        // An expression has no placements to rule out, so the honest answer is
        // "still possible". Falling through would compute `any()` over an
        // empty iterator, say "impossible", and quietly drop every candidate.
        if self.pattern.is_regex() {
            return true;
        }
        let symbols = &self.pattern.symbols;
        self.pattern
            .candidate_offsets()
            .into_iter()
            // A placement that fits inside the key was already settled by the
            // key pass. Re-examining it here made a substring walk all 47 of
            // its offsets twice per candidate, which is most of what the
            // substring cost.
            .filter(|start| start + symbols.len() > KEY_ONLY_SYMBOLS)
            .any(|start| {
                let readable = KEY_DERIVED_SYMBOLS.saturating_sub(start).min(symbols.len());
                symbols[..readable].iter().enumerate().all(|(i, set)| {
                    let at = start + i;
                    let set = if at == SIGN_SYMBOL {
                        either_sign(*set)
                    } else {
                        *set
                    };
                    set.contains(symbol_at(packed, at))
                })
            })
    }

    /// Whether a packed public key matches this filter's key-derived span.
    ///
    /// This is the hot path: a byte comparison plus one masked byte, with no
    /// hashing, no base32 and no allocation. For filters longer than
    /// [`KEY_ONLY_SYMBOLS`] it is a prefilter — [`Filter::needs_full_check`]
    /// says whether the address must still be confirmed.
    #[inline]
    pub fn matches_key(&self, packed: &[u8; 32]) -> bool {
        let n = self.bytes.len();
        // An expression decodes to no bytes, and `n - 1` below would wrap.
        // It never reaches here through the set, which holds expressions in
        // their own vector; this is the guard for a caller that has one in
        // hand and does not know which form it is.
        if n == 0 {
            debug_assert!(self.pattern.is_regex());
            return self.matches_symbols(packed, false);
        }
        debug_assert!(n <= 32);
        let last = n - 1;
        packed[..last] == self.bytes[..last] && (packed[last] & self.mask) == self.bytes[last]
    }

    /// Whether matching the key span leaves part of this filter unverified.
    #[inline]
    pub fn needs_full_check(&self) -> bool {
        if let Some(expression) = self.pattern.expression() {
            return expression.needs_checksum();
        }
        self.pattern.canonical.len() > KEY_ONLY_SYMBOLS
    }

    /// The filter spelled out one symbol per position: a class prints as the
    /// lowest symbol it accepts and no anchor marker is printed at all.
    ///
    /// Deliberately not named `as_str`: this does not round-trip, and the one
    /// caller that treated it as the filter's text turned every class and every
    /// `contains:` in a filter file into a plain prefix.
    pub fn canonical(&self) -> &str {
        &self.pattern.canonical
    }

    /// The filter as it was written, which is what round-trips.
    pub fn source_text(&self) -> &str {
        &self.pattern.text
    }

    pub fn len(&self) -> usize {
        self.pattern.canonical.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pattern.canonical.is_empty()
    }

    /// Whether this filter absorbs `other`: every address matching `other`
    /// already matches this one.
    pub fn absorbs(&self, other: &Filter) -> bool {
        // Absorption is a statement about base32 prefixes. Two expressions
        // whose texts happen to share a prefix say nothing about the addresses
        // they accept, and `^a` would swallow `^ab|cd`.
        if self.pattern.is_regex() || other.pattern.is_regex() {
            return false;
        }
        other.pattern.canonical.starts_with(&self.pattern.canonical)
    }

    /// The compiled expression, for the one form that carries one.
    pub fn expression(&self) -> Option<&Expression> {
        self.pattern.expression()
    }
}

/// Runs an expression against whichever text the caller has.
///
/// `bytes` is either the 32-byte packed key or the 35 encoded address bytes,
/// matching the rest of this module. An expression that asked for the full
/// address cannot be answered from the key, and says so by not matching rather
/// than by matching something shorter.
fn match_expression(expression: &Expression, bytes: &[u8], have_checksum: bool) -> bool {
    offset_of_expression(expression, bytes, have_checksum).is_some()
}

fn offset_of_expression(
    expression: &Expression,
    bytes: &[u8],
    have_checksum: bool,
) -> Option<usize> {
    if expression.haystack() == Haystack::FullAddress && !have_checksum {
        return None;
    }
    let reach = if have_checksum {
        ADDRESS_LEN
    } else {
        KEY_ONLY_SYMBOLS
    };
    let mut text = [0u8; ADDRESS_LEN];
    for (i, slot) in text[..reach].iter_mut().enumerate() {
        *slot = base32::ALPHABET[symbol_at(bytes, i) as usize];
    }
    expression.find(&text[..reach])
}

/// Whether the obligatory literals of an expression still allow this key.
///
/// Reads the key directly rather than its base32 text: the point of extracting
/// a literal is to answer before anything is encoded, and an encoding costs
/// forty nanoseconds against this one's few.
#[inline]
fn literals_allow(expression: &Expression, packed: &[u8; 32]) -> bool {
    match expression.literals() {
        Literals::Prefix(words, common) => {
            // The shared head first: for a long list it is the whole cost,
            // since every literal starts with it and almost every candidate
            // fails it. `^a[2-7]{3}shop` walks 216 literals without this and
            // one comparison with it.
            if !starts_with_symbols(packed, common) {
                return false;
            }
            words.len() == 1 || words.iter().any(|word| starts_with_symbols(packed, word))
        }
        // A substring has no fixed position, so there is nothing to compare
        // against without the text. The set answers these over the encoded
        // buffer it already has.
        Literals::Substring(_) | Literals::None(_) => true,
    }
}

/// Past this many obligatory prefixes, walking them one by one stops paying.
///
/// Below it the walk wins outright: a handful of byte comparisons against a
/// key already in a register beats a load from a table that is not.
const REGEX_BITMAP_FROM: usize = 16;

/// A bitmap over an expression's obligatory prefixes, where one is worth it.
///
/// The literals are ordinary base32 prefixes, so they go through the same
/// parser and the same index a dictionary of prefixes would — there is no
/// second implementation of the structure here, only a second caller.
fn build_regex_index(expression: &Expression) -> Option<BitmapIndex> {
    let words = match expression.literals() {
        Literals::Prefix(words, _) => words,
        // A substring has no fixed position for a bitmap to key on, and there
        // is nothing to guard when no literal was proven at all.
        Literals::Substring(_) | Literals::None(_) => return None,
    };
    if words.len() < REGEX_BITMAP_FROM {
        return None;
    }
    let filters: Vec<Filter> = words
        .iter()
        .filter_map(|w| std::str::from_utf8(w).ok())
        .filter_map(|w| Filter::parse(w).ok())
        .collect();
    // Every literal has to be in the map. One that failed to parse would be a
    // branch the map does not know about, and the map is consulted instead of
    // the list, not alongside it.
    if filters.len() != words.len() {
        return None;
    }
    BitmapIndex::build(&filters, choose_index_bits(filters.len()))
}

/// Whether an expression's obligatory substrings appear in the text at all.
///
/// The counterpart of [`literals_allow`] for the unanchored case, where there
/// is no fixed position to compare against and the text is the only way to
/// ask. Cheaper than the engine by roughly the difference between a memchr
/// scan and a state machine, and it runs first.
#[inline]
fn substring_allows(expression: &Expression, text: &[u8]) -> bool {
    match expression.literals() {
        Literals::Substring(words) => words
            .iter()
            .any(|word| memchr::memmem::find(text, word).is_some()),
        Literals::Prefix(..) | Literals::None(_) => true,
    }
}

/// Whether the key's base32 text begins with these symbols, read from the
/// packed bits rather than from an encoding of them.
#[inline]
fn starts_with_symbols(packed: &[u8; 32], word: &[u8]) -> bool {
    // Truncated to what the key settles exactly, rather than refused when it
    // runs past. This is a prefilter, and a prefilter that answers "no" to a
    // literal it merely cannot finish reading would throw away candidates the
    // expression accepts. The extractor caps literals well below this, so the
    // line never triggers today — which is precisely why it should be right
    // rather than lucky.
    let readable = word.len().min(KEY_ONLY_SYMBOLS);
    word[..readable]
        .iter()
        .enumerate()
        .all(|(i, &want)| base32::ALPHABET[symbol_at(packed, i) as usize] == want)
}

/// Reads filter text from a file, one per line, skipping blanks and comments.
///
/// Returns the lines as written, validated. Callers that want both the text and
/// the parsed set must start here: a parsed filter cannot be turned back into
/// its text, so re-serialising one to hand it on loses the form.
pub fn read_filter_lines(path: &Path) -> Result<Vec<String>, FilterFileError> {
    let text = std::fs::read_to_string(path).map_err(FilterFileError::Io)?;
    let mut lines = Vec::new();
    for (number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
            continue;
        }
        if let Err(source) = Filter::parse(line) {
            return Err(FilterFileError::Invalid {
                line: number + 1,
                text: line.to_string(),
                source,
            });
        }
        lines.push(line.to_string());
    }
    Ok(lines)
}

/// Says whether a substring could still begin inside the key and run into the
/// checksum symbols, for a candidate whose key has already been searched.
///
/// A prefilter, so it may answer "maybe" when the answer is no; it must never
/// answer "no" when the answer is yes. False positives cost one hash that
/// turns out unnecessary.
///
/// Without it, a set holding an automaton had to build the address for every
/// candidate, because a needle can start as late as symbol 51 and finish past
/// it. With it, only the candidates whose key tail actually begins some needle
/// pay for the hash.
#[derive(Debug)]
struct TailPrefilter {
    bits: Vec<u64>,
    mask: u64,
    /// Which suffix lengths of the key text can begin a needle. Usually a
    /// handful: a needle of `L` symbols can overhang by one to five.
    lengths: Vec<u8>,
    /// A needle short enough to sit wholly past the key rules out nothing:
    /// those symbols come from the checksum and the key says nothing about
    /// them.
    always: bool,
}

impl TailPrefilter {
    /// How many symbols of an address the checksum produces.
    const TAIL: usize = ADDRESS_LEN - KEY_ONLY_SYMBOLS;

    fn build(needles: &[Vec<u8>]) -> Self {
        let entries = needles.len() * Self::TAIL;
        // Roughly one slot in fifty occupied, so a false positive is rare and
        // the table stays small enough to sit in cache.
        let slots = (entries * 64).next_power_of_two().max(1 << 12);
        let mut me = TailPrefilter {
            bits: vec![0u64; slots / 64],
            mask: (slots - 1) as u64,
            lengths: Vec::new(),
            always: false,
        };
        let mut seen = [false; ADDRESS_LEN];
        for needle in needles {
            let len = needle.len();
            if len > ADDRESS_LEN {
                continue;
            }
            let first = (KEY_ONLY_SYMBOLS + 1).saturating_sub(len);
            for start in first..=(ADDRESS_LEN - len) {
                if start >= KEY_ONLY_SYMBOLS {
                    me.always = true;
                    continue;
                }
                let k = KEY_ONLY_SYMBOLS - start;
                me.set(&needle[..k]);
                if !seen[k] {
                    seen[k] = true;
                    me.lengths.push(k as u8);
                }
            }
        }
        me.lengths.sort_unstable();
        me
    }

    /// A cheap multiplicative hash. Nothing here is adversarial: the inputs
    /// are base32 symbols and a collision costs one wasted hash.
    #[inline]
    fn hash(symbols: &[u8]) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for &c in symbols {
            h = (h ^ u64::from(c)).wrapping_mul(0x1000_0000_01b3);
        }
        h ^ (h >> 29)
    }

    fn set(&mut self, symbols: &[u8]) {
        let slot = (Self::hash(symbols) & self.mask) as usize;
        self.bits[slot >> 6] |= 1u64 << (slot & 63);
    }

    #[inline]
    fn possible(&self, text: &[u8]) -> bool {
        if self.always {
            return true;
        }
        self.lengths.iter().any(|&k| {
            let slot = (Self::hash(&text[text.len() - k as usize..]) & self.mask) as usize;
            self.bits[slot >> 6] & (1u64 << (slot & 63)) != 0
        })
    }

    fn memory(&self) -> usize {
        self.bits.len() * 8 + self.lengths.len()
    }
}

/// What the key alone settles about a candidate.
#[derive(Debug)]
pub enum KeyVerdict<'a> {
    /// A filter matched within the key-derived symbols.
    Matched(&'a Filter),
    /// Nothing matched, and nothing can: no need for the address.
    No,
    /// Nothing matched yet, but a form could still reach into the checksum.
    NeedsAddress,
}

/// How the literal substrings of a set are searched over the encoded candidate.
///
/// Searching once per filter is a SIMD scan: cheap at first, growing with the
/// filter count. An automaton walks the text once at a fixed cost whatever the
/// count. On x86 with eight-symbol substrings, one by one runs 109 ns at one
/// filter and 12 550 ns at a thousand; the automaton is a flat 200 ns until 128
/// and 419 ns at a thousand. They cross at ten.
#[derive(Debug)]
enum SubstringSearch {
    /// One SIMD search per filter, below the crossover.
    OneByOne(Vec<(u32, memchr::memmem::Finder<'static>)>),
    /// One automaton pass, flat in the number of filters.
    ///
    /// Built as an explicit DFA with the prefilter switched off. Neither is the
    /// default, and both matter here: the address alphabet is 32 symbols over
    /// 51 bytes, so no byte is rare, and the prefilter the crate would pick
    /// fires on nearly every candidate — measured at 424 ns against 197 for
    /// the same automaton without it.
    Automaton {
        owners: Vec<u32>,
        dfa: Box<aho_corasick::AhoCorasick>,
        tail: Box<TailPrefilter>,
    },
}

/// Where the automaton starts winning, measured rather than chosen.
const AUTOMATON_THRESHOLD: usize = 10;

impl SubstringSearch {
    /// `None` when the set holds no literal substring.
    ///
    /// Boxed and optional rather than an `Empty` variant: this sits in
    /// `FilterSet`, which the hot loop reads, and the variants are large. Held
    /// inline it grew the struct by forty bytes, and the forms that never use
    /// it measured a few percent slower for that alone.
    fn build(literals: Vec<(u32, Vec<u8>)>) -> Option<Box<Self>> {
        if literals.is_empty() {
            return None;
        }
        if literals.len() < AUTOMATON_THRESHOLD {
            return Some(Box::new(SubstringSearch::OneByOne(
                literals
                    .into_iter()
                    .map(|(owner, needle)| {
                        (owner, memchr::memmem::Finder::new(&needle).into_owned())
                    })
                    .collect(),
            )));
        }
        let (owners, needles): (Vec<u32>, Vec<Vec<u8>>) = literals.into_iter().unzip();
        let dfa = aho_corasick::AhoCorasick::builder()
            .prefilter(false)
            .kind(Some(aho_corasick::AhoCorasickKind::DFA))
            .build(&needles)
            .expect("the needles are validated filter bodies");
        let tail = TailPrefilter::build(&needles);
        Some(Box::new(SubstringSearch::Automaton {
            owners,
            dfa: Box::new(dfa),
            tail: Box::new(tail),
        }))
    }

    #[inline]
    fn is_automaton(&self) -> bool {
        matches!(self, SubstringSearch::Automaton { .. })
    }

    /// The filter index that matched, if any.
    #[inline]
    fn find(&self, text: &[u8]) -> Option<u32> {
        match self {
            SubstringSearch::OneByOne(finders) => finders
                .iter()
                .find(|(_, f)| f.find(text).is_some())
                .map(|(owner, _)| *owner),
            SubstringSearch::Automaton { owners, dfa, .. } => {
                dfa.find(text).map(|m| owners[m.pattern().as_usize()])
            }
        }
    }

    /// Bytes the structure occupies, for the diagnostics.
    fn memory(&self) -> usize {
        match self {
            SubstringSearch::OneByOne(finders) => finders.len() * size_of::<(u32, usize)>(),
            SubstringSearch::Automaton { dfa, owners, tail } => {
                dfa.memory_usage() + owners.len() * size_of::<u32>() + tail.memory()
            }
        }
    }

    /// How the structure is named in the diagnostics.
    fn describe(&self) -> &'static str {
        match self {
            SubstringSearch::OneByOne(_) => "one search per substring",
            SubstringSearch::Automaton { .. } => "substring automaton",
        }
    }
}

/// A validated, deduplicated collection of filters.
#[derive(Debug)]
pub struct FilterSet {
    /// Literal prefixes: the byte-comparison path, with the index and the
    /// sorted lookup built over them.
    filters: Vec<Filter>,
    index: Option<BitmapIndex>,
    /// Sorted lookup by filter length. Empty while the set is small enough that
    /// a scan is cheaper than the branching a binary search costs.
    buckets: Vec<LengthBucket>,
    /// Everything else: wildcards, classes, substrings, suffixes.
    ///
    /// Held separately so that a set without them costs exactly what it cost
    /// before these forms existed — one test against an empty vector.
    patterns: Vec<Filter>,
    /// How the literal substrings of this set are searched.
    text_substrings: Option<Box<SubstringSearch>>,
    /// Indices into `patterns` of the forms still checked one by one:
    /// everything the substring automaton cannot serve.
    walk_idx: Vec<u32>,
    /// Those of `walk_idx` that are anchored to the start and covered by
    /// `pattern_index`, and the rest.
    ///
    /// Split because only the anchored ones can be guarded by a bitmap: a
    /// suffix or a substring has no fixed position for the index to key on.
    anchored_idx: Vec<u32>,
    unguarded_idx: Vec<u32>,
    /// A bitmap over `anchored_idx`, when one discriminates.
    pattern_index: Option<BitmapIndex>,
    /// The same for suffixes, keyed on the end of the key-derived span.
    suffix_idx: Vec<u32>,
    suffix_index: Option<BitmapIndex>,
    /// Checksum-reaching forms the suffix bitmap does not answer for.
    checksum_probe_idx: Vec<u32>,
    /// Those of `checksum_idx` that the substring automaton does not already
    /// cover.
    ///
    /// In automaton mode the literal substrings are settled by the automaton
    /// over the key text plus the tail prefilter, so asking them again here
    /// would restore exactly the linear walk the automaton exists to remove.
    ///
    /// Holding these as a sorted prefix of `checksum_idx` plus a count was
    /// tried, to make the struct smaller. It measured 6% slower on
    /// prefix-only search, which is the opposite of what the idea predicted,
    /// so the plain vector stays.
    residual_checksum_idx: Vec<u32>,
    /// Indices into `patterns` of the forms whose placements reach past the
    /// key into the checksum symbols.
    ///
    /// Indices rather than a second vector because the two groups overlap: a
    /// substring is readable from the key at most of its offsets and reaches
    /// the checksum at the last few, so it belongs to both passes.
    checksum_idx: Vec<u32>,
    /// Regular expressions, held apart from every other form.
    ///
    /// A third vector rather than a third kind inside `patterns`, for the
    /// reason `patterns` is itself apart from `filters`: the walk over
    /// `patterns` is a hot loop, and a branch inside it would be paid by every
    /// set, including the ones that use no expression at all.
    regexes: Vec<Filter>,
    /// Those of `regexes` decidable from the 49 key-derived symbols, and those
    /// needing the whole address.
    ///
    /// Split at construction because the two are asked at different moments:
    /// the first before any hash, the second only for a candidate that already
    /// survived everything cheaper.
    regex_key_idx: Vec<u32>,
    regex_address_idx: Vec<u32>,
    /// A bitmap over each expression's obligatory prefixes, parallel to
    /// `regexes`, where the list is long enough to be worth one.
    ///
    /// Only some expressions get one. A single literal is four byte
    /// comparisons, and a bitmap would replace them with a load from a table
    /// that is not in a register; the list has to be long before a map is the
    /// cheaper answer. Measured on the two shapes that matter:
    /// `^a[2-7]{3}shop` has 216 literals that share the head `a`, and the
    /// shared head alone brings it to 17.8 ns, while `^[2-7]{3}shop` has 216
    /// that share nothing and costs 237.9 ns with no map.
    regex_index: Vec<Option<BitmapIndex>>,
}

impl FilterSet {
    /// Builds a set, dropping duplicates and filters absorbed by shorter ones.
    ///
    /// Absorption matters for dictionaries: a word list holding both short and
    /// long words with shared prefixes shrinks considerably, and the redundant
    /// entries would otherwise cost throughput for nothing.
    pub fn new(mut filters: Vec<Filter>) -> Self {
        // Sorting puts every filter after the shorter ones that could absorb
        // it, so one pass suffices.
        filters.sort();
        let mut kept: Vec<Filter> = Vec::with_capacity(filters.len());
        for f in filters {
            if kept.last().is_some_and(|prev| prev.absorbs(&f)) {
                continue;
            }
            kept.push(f);
        }
        // Zero asks for the width to be chosen by looking at the result.
        Self::with_index_bits(kept, 0)
    }

    /// Builds a set with an explicit index width. Used by measurements and
    /// tests; ordinary callers take the default.
    /// Builds a set with an explicit index width; `0` chooses one.
    pub fn with_index_bits(filters: Vec<Filter>, bits: u32) -> Self {
        // Expressions come out before anything else looks at the list: none of
        // the structures below is built over symbol sets an expression does
        // not have.
        let (regexes, filters): (Vec<Filter>, Vec<Filter>) =
            filters.into_iter().partition(|f| f.pattern.is_regex());
        let mut regex_key_idx = Vec::new();
        let mut regex_address_idx = Vec::new();
        let mut regex_index = Vec::with_capacity(regexes.len());
        for (i, f) in regexes.iter().enumerate() {
            match f.expression().map(Expression::haystack) {
                Some(Haystack::KeyText) => regex_key_idx.push(i as u32),
                _ => regex_address_idx.push(i as u32),
            }
            regex_index.push(f.expression().and_then(build_regex_index));
        }
        let (fast, rest): (Vec<Filter>, Vec<Filter>) =
            filters.into_iter().partition(Filter::is_fast);
        let patterns = rest;
        let mut literal_substrings = Vec::new();
        let mut walk_idx = Vec::new();
        for (i, f) in patterns.iter().enumerate() {
            if f.pattern.anchor == Anchor::Anywhere && f.pattern.is_literal() {
                literal_substrings.push((i as u32, f.pattern.canonical.as_bytes().to_vec()));
            } else {
                walk_idx.push(i as u32);
            }
        }
        let text_substrings = SubstringSearch::build(literal_substrings);
        let checksum_idx: Vec<u32> = patterns
            .iter()
            .enumerate()
            .filter(|(_, f)| f.needs_checksum())
            .map(|(i, _)| i as u32)
            .collect();
        // A literal substring is dropped from the probe list only when the
        // automaton is there to answer for it: that variant carries a tail
        // prefilter, which is what decides whether the address is worth
        // building. The one-search-per-substring variant carries none, so
        // dropping its filters here left nothing to ask, and every occurrence
        // that reached past the key-only span was lost without a word.
        let tail_answers = matches!(
            text_substrings.as_deref(),
            Some(SubstringSearch::Automaton { .. })
        );
        let residual_checksum_idx: Vec<u32> = checksum_idx
            .iter()
            .copied()
            .filter(|&i| {
                let f = &patterns[i as usize];
                !(tail_answers && f.pattern.anchor == Anchor::Anywhere && f.pattern.is_literal())
            })
            .collect();
        let auto = bits == 0;
        let index = if auto {
            narrowest_acceptable(TARGET_FALSE_HITS, |b| {
                BitmapIndex::build(&fast, b).map(|ix| (ix, ()))
            })
            .map(|(ix, ())| ix)
        } else {
            BitmapIndex::build(&fast, bits)
        };
        // Only forms anchored to the start can be keyed on a fixed position.
        let start_candidates: Vec<u32> = walk_idx
            .iter()
            .copied()
            .filter(|&i| patterns[i as usize].pattern.anchor == Anchor::Start)
            .collect();
        let built = if auto {
            narrowest_acceptable(target_for_walk(start_candidates.len()), |b| {
                BitmapIndex::build_over_patterns(&patterns, &start_candidates, b)
            })
        } else {
            BitmapIndex::build_over_patterns(&patterns, &start_candidates, bits)
        };
        let (pattern_index, anchored_idx) = match built {
            Some((ix, covered)) => (Some(ix), covered),
            None => (None, Vec::new()),
        };
        let suffix_candidates: Vec<u32> = walk_idx
            .iter()
            .copied()
            .filter(|&i| patterns[i as usize].pattern.anchor == Anchor::End)
            .collect();
        let built = if auto {
            narrowest_acceptable(target_for_walk(suffix_candidates.len()), |b| {
                BitmapIndex::build_over_suffixes(&patterns, &suffix_candidates, b)
            })
        } else {
            BitmapIndex::build_over_suffixes(&patterns, &suffix_candidates, bits)
        };
        let (suffix_index, suffix_idx) = match built {
            Some((ix, covered)) => (Some(ix), covered),
            None => (None, Vec::new()),
        };
        let unguarded_idx: Vec<u32> = walk_idx
            .iter()
            .copied()
            .filter(|i| !anchored_idx.contains(i) && !suffix_idx.contains(i))
            .collect();
        // What the checksum prefilter still has to ask one by one: everything
        // the suffix bitmap does not answer for.
        let checksum_probe_idx: Vec<u32> = checksum_idx
            .iter()
            .copied()
            .filter(|i| !suffix_idx.contains(i))
            .collect();
        let residual_checksum_idx: Vec<u32> = residual_checksum_idx
            .into_iter()
            .filter(|i| !suffix_idx.contains(i))
            .collect();
        let buckets = Self::build_buckets(&fast);
        FilterSet {
            filters: fast,
            index,
            buckets,
            patterns,
            text_substrings,
            walk_idx,
            anchored_idx,
            unguarded_idx,
            pattern_index,
            suffix_idx,
            suffix_index,
            residual_checksum_idx,
            checksum_probe_idx,
            checksum_idx,
            regexes,
            regex_key_idx,
            regex_address_idx,
            regex_index,
        }
    }

    /// The expressions answerable from the key text, asked after every cheaper
    /// form has already declined.
    ///
    /// Each expression is guarded by its own obligatory literals first, which
    /// is what makes the form affordable.
    ///
    /// `have` is the encoded key text when the caller already built one, `None`
    /// otherwise. An encoding costs forty nanoseconds and a literal read off
    /// the packed bits costs four, so the text is built only once an expression
    /// has passed its literals and has to be run. Encoding first and filtering
    /// afterwards costs 45.7 ns per candidate against the prefix path's 4.5.
    #[inline]
    fn match_regexes_key(
        &self,
        packed: &[u8; 32],
        have: Option<&[u8; KEY_ONLY_SYMBOLS]>,
    ) -> Option<&Filter> {
        let mut buf = [0u8; KEY_ONLY_SYMBOLS];
        let mut encoded = false;
        for &i in &self.regex_key_idx {
            let filter = &self.regexes[i as usize];
            let Some(expression) = filter.expression() else {
                continue;
            };
            if !self.literals_allow_at(i, expression, packed) {
                continue;
            }
            let text: &[u8] = match have {
                Some(text) => text,
                None => {
                    if !encoded {
                        crate::base32::encode_into(packed, &mut buf);
                        encoded = true;
                    }
                    &buf
                }
            };
            if substring_allows(expression, text) && expression.find(text).is_some() {
                return Some(filter);
            }
        }
        None
    }

    /// The expressions that need the whole address, asked last of all.
    fn match_regexes_address(
        &self,
        packed: &[u8; 32],
        text: &[u8; ADDRESS_LEN],
    ) -> Option<&Filter> {
        self.regex_address_idx.iter().find_map(|&i| {
            let filter = &self.regexes[i as usize];
            let expression = filter.expression()?;
            (self.literals_allow_at(i, expression, packed)
                && substring_allows(expression, text)
                && expression.find(text).is_some())
            .then_some(filter)
        })
    }

    /// Whether an expression's obligatory prefixes still allow this key,
    /// through whichever structure was built for it.
    #[inline]
    fn literals_allow_at(&self, i: u32, expression: &Expression, packed: &[u8; 32]) -> bool {
        match &self.regex_index[i as usize] {
            Some(index) => index.may_match(packed),
            None => literals_allow(expression, packed),
        }
    }

    /// Whether the set holds an expression that only the full address settles.
    #[inline]
    fn wants_address_for_regex(&self) -> bool {
        !self.regex_address_idx.is_empty()
    }

    /// Whether any address-reaching expression is still possible for this key.
    ///
    /// The counterpart of [`FilterSet::checksum_possible`] for expressions: a
    /// form that needs the checksum is not thereby beyond the key's reach,
    /// because its literals are anchored where the key can read them. Only
    /// candidates that survive this pay for a hash.
    #[inline]
    fn any_address_regex_possible(&self, packed: &[u8; 32]) -> bool {
        self.regex_address_idx.iter().any(|&i| {
            self.regexes[i as usize]
                .expression()
                .is_some_and(|e| self.literals_allow_at(i, e, packed))
        })
    }

    /// Every filter in the set, expressions included.
    fn all_filters(&self) -> impl Iterator<Item = &Filter> {
        self.filters
            .iter()
            .chain(self.patterns.iter())
            .chain(self.regexes.iter())
    }

    /// How many expressions the set holds.
    pub fn regex_count(&self) -> usize {
        self.regexes.len()
    }

    /// Whether any filter's share of the address space is not computable, so
    /// that an estimate built from the rest is a bound rather than a figure.
    pub fn has_unknown_odds(&self) -> bool {
        !self.regexes.is_empty()
    }

    /// Whether any filter needs the checksum symbols, which costs a SHA3-256
    /// per candidate.
    pub fn needs_checksum(&self) -> bool {
        !self.checksum_idx.is_empty() || self.wants_address_for_regex()
    }

    /// Whether this set is cheaper to match against the full address than
    /// against the key.
    ///
    /// It is, once the substrings are held in an automaton and any of them
    /// reaches the checksum. The alternative is the per-candidate prefilter,
    /// which asks every checksum-reaching filter whether it is still possible
    /// — linear in their number, and measured at some fifty times the cost of
    /// the hash it exists to avoid. Walking the whole address with the
    /// automaton costs one hash and one flat pass instead.
    pub fn prefers_address(&self) -> bool {
        // An expression needing the whole address has no cheap prefilter for
        // "could this still match": unlike a suffix, it has no fixed
        // placement to rule out. Building the address once and asking over it
        // is the only honest answer, so such a set takes this path whatever
        // its substrings look like.
        if self.wants_address_for_regex() {
            return true;
        }
        self.text_substrings
            .as_ref()
            .is_some_and(|s| s.is_automaton())
            && self.needs_checksum()
    }

    /// What the key alone settles, for a set that would otherwise have to hash
    /// every candidate.
    ///
    /// The automaton runs over the 51 key-derived symbols. A miss there leaves
    /// only the placements that reach into the checksum, and the tail
    /// prefilter answers whether any of those is still alive — cheaply, and
    /// without depending on how many filters there are.
    pub fn examine_key(&self, packed: &[u8; 32]) -> KeyVerdict<'_> {
        if let Some(found) = self.match_literals(packed) {
            return KeyVerdict::Matched(found);
        }
        // Encoded only for the forms that read text. A set that takes this
        // path because of an expression over the whole address has none: its
        // literals are read off the packed bits and its engine runs on the
        // address, so an encoding of the key here would be forty nanoseconds
        // spent on a buffer nothing looks at.
        let wants_text = self.text_substrings.is_some() || !self.regex_key_idx.is_empty();
        let mut text = [0u8; KEY_ONLY_SYMBOLS];
        if wants_text {
            crate::base32::encode_into(packed, &mut text);
            if let Some(owner) = self.text_substrings.as_ref().and_then(|s| s.find(&text)) {
                return KeyVerdict::Matched(&self.patterns[owner as usize]);
            }
        }
        if let Some(found) = self.match_walk(packed) {
            return KeyVerdict::Matched(found);
        }
        if let Some(found) = self.match_regexes_key(packed, wants_text.then_some(&text)) {
            return KeyVerdict::Matched(found);
        }
        // An expression over the whole address still has a say about the
        // key: its obligatory literals sit at the front, where the key
        // settles them, even though its match reaches the checksum. Asking
        // them here is what keeps `^shop.*` from hashing every candidate —
        // measured at 346.8 ns per candidate before this and a few after.
        if self.any_address_regex_possible(packed) {
            return KeyVerdict::NeedsAddress;
        }
        if let Some(SubstringSearch::Automaton { tail, .. }) = self.text_substrings.as_deref() {
            if tail.possible(&text) {
                return KeyVerdict::NeedsAddress;
            }
        }
        // Only the forms the automaton does not cover are asked one by one.
        // For a dictionary of substrings that list is empty, which is the
        // whole point.
        if self.checksum_alive(packed, &self.residual_checksum_idx) {
            KeyVerdict::NeedsAddress
        } else {
            KeyVerdict::No
        }
    }

    /// The matcher for a set that prefers the address: one pass over all 56
    /// symbols, so the checksum placements need no separate prefilter.
    pub fn match_whole_address(&self, packed: &[u8; 32], address: &[u8; 35]) -> Option<&Filter> {
        if let Some(found) = self.match_literals(packed) {
            return Some(found);
        }
        let mut text = [0u8; ADDRESS_LEN];
        crate::base32::encode_into(address, &mut text);
        if let Some(owner) = self.text_substrings.as_ref().and_then(|s| s.find(&text)) {
            return Some(&self.patterns[owner as usize]);
        }
        if let Some(found) = self
            .walk_idx
            .iter()
            .map(|&i| &self.patterns[i as usize])
            .find(|f| f.matches_symbols(address, true))
        {
            return Some(found);
        }
        self.match_regexes_address(packed, &text)
    }

    /// One line per matching structure: what was built, what it costs, or why
    /// it was not built.
    ///
    /// Which structure a set gets is decided from the forms and their number,
    /// and declining one is a normal outcome. Without this the user has no way
    /// to tell "no structure was built" from "a structure was built and does
    /// not help", and those call for opposite responses.
    pub fn describe_structures(&self) -> Vec<String> {
        let kib = |bytes: usize| format!("{} KiB", bytes.div_ceil(1024));
        let mut out = Vec::new();

        out.push(match (&self.index, self.filters.is_empty()) {
            (Some(ix), _) => format!(
                "prefix bitmap: {} over {} prefix(es)",
                kib(ix.memory()),
                self.filters.len()
            ),
            (None, true) => "prefix bitmap: not built, no literal prefixes".to_string(),
            (None, false) => {
                "prefix bitmap: not built, it would pass nearly everything through".to_string()
            }
        });

        let anchored_total = self.count_walk(Anchor::Start);
        if anchored_total > 0 {
            out.push(match &self.pattern_index {
                Some(ix) if self.anchored_idx.len() == anchored_total => format!(
                    "anchored-form bitmap: {} over {anchored_total} form(s)",
                    kib(ix.memory())
                ),
                Some(ix) => format!(
                    "anchored-form bitmap: {} over {} of {anchored_total} form(s); the rest are \
                     too broad to enumerate and are checked one by one",
                    kib(ix.memory()),
                    self.anchored_idx.len()
                ),
                None => "anchored-form bitmap: not built, the forms are too broad to enumerate \
                         or would fill the map"
                    .to_string(),
            });
        }

        let suffix_total = self.count_walk(Anchor::End);
        if suffix_total > 0 {
            out.push(match &self.suffix_index {
                Some(ix) if self.suffix_idx.len() == suffix_total => format!(
                    "suffix bitmap: {} over {suffix_total} suffix(es)",
                    kib(ix.memory())
                ),
                Some(ix) => format!(
                    "suffix bitmap: {} over {} of {suffix_total} suffix(es); the rest are short \
                     enough to fill the map and are checked one by one",
                    kib(ix.memory()),
                    self.suffix_idx.len()
                ),
                None => {
                    "suffix bitmap: not built, the suffixes are too short to key on".to_string()
                }
            });
        }

        match &self.text_substrings {
            Some(search) => out.push(format!(
                "substring search: {}, {}",
                search.describe(),
                kib(search.memory())
            )),
            None if self.count_walk(Anchor::Anywhere) > 0 => {}
            None => {}
        }

        // The one form no structure serves, named so its cost is not a
        // mystery: a substring holding a class or a wildcard has no literal to
        // give a searcher and no fixed position to key on.
        let open_substrings = self
            .patterns
            .iter()
            .filter(|f| f.pattern.anchor == Anchor::Anywhere && !f.pattern.is_literal())
            .count();
        if open_substrings > 0 {
            out.push(format!(
                "{open_substrings} substring(s) hold a class or a wildcard and are checked one by \
                 one at every offset"
            ));
        }

        // One line per expression. Which of the three paths it landed on does
        // not follow from reading it — adding a `$` moves it from the first to
        // the last, and the two differ by a hash per candidate — so the user
        // who wrote it has no way to know without being told.
        for (i, filter) in self.regexes.iter().enumerate() {
            let Some(expression) = filter.expression() else {
                continue;
            };
            let guard = match (expression.literals(), &self.regex_index[i]) {
                (Literals::Prefix(words, _), Some(ix)) => format!(
                    "a bitmap of {} over {} obligatory prefixes",
                    kib(ix.memory()),
                    words.len()
                ),
                (Literals::Prefix(words, _), None) if words.len() == 1 => format!(
                    "the obligatory prefix {:?}, compared on the key",
                    String::from_utf8_lossy(&words[0])
                ),
                (Literals::Prefix(words, common), None) if common.is_empty() => {
                    format!("{} obligatory prefixes, compared on the key", words.len())
                }
                (Literals::Prefix(words, common), None) => format!(
                    "{} obligatory prefixes sharing {:?}, compared on the key",
                    words.len(),
                    String::from_utf8_lossy(common)
                ),
                (Literals::Substring(words), _) if words.len() == 1 => format!(
                    "the obligatory substring {:?}, searched in the text",
                    String::from_utf8_lossy(&words[0])
                ),
                (Literals::Substring(words), _) => format!(
                    "{} obligatory substrings, searched in the text",
                    words.len()
                ),
                (Literals::None(why), _) => {
                    format!("nothing: {why}, so the engine sees every candidate")
                }
            };
            let reach = match expression.haystack() {
                Haystack::KeyText => {
                    format!("decided within the first {KEY_ONLY_SYMBOLS} symbols")
                }
                Haystack::FullAddress => {
                    "reaches the checksum, so a surviving candidate costs a SHA3-256".to_string()
                }
            };
            out.push(format!(
                "regex {}: guarded by {guard}; {reach}",
                expression.source()
            ));
        }

        // Said once, and said plainly, because the guarantee every other form
        // enjoys does not extend here and silence would read as though it did.
        if !self.regexes.is_empty() {
            out.push(format!(
                "{} regular expression(s): the limit on how much a growing filter count may cost \
                 does not cover them — they express a shape the other forms cannot, and are \
                 written one at a time rather than as dictionaries",
                self.regexes.len()
            ));
        }
        out
    }

    fn count_walk(&self, anchor: Anchor) -> usize {
        self.walk_idx
            .iter()
            .filter(|&&i| self.patterns[i as usize].pattern.anchor == anchor)
            .count()
    }

    /// The prefix bitmap as raw words, with the width it was built at.
    ///
    /// Exists for the device: the bitmap is the one structure here that has no
    /// pointers in it, so it is the one that can be copied into device memory
    /// and asked there. Everything that answers *which* filter matched holds
    /// references and stays on the host.
    pub fn index_words(&self) -> Option<(&[u64], u32)> {
        self.index.as_ref().map(|ix| (ix.words.as_slice(), ix.bits))
    }

    /// The structure built for the literal substrings, and what it costs in
    /// memory. `None` when the set holds none.
    pub fn substring_structure(&self) -> Option<(&'static str, usize)> {
        let search = self.text_substrings.as_ref()?;
        Some((search.describe(), search.memory()))
    }

    /// How many filters have placements past the key-derived span.
    ///
    /// Expressions count. Leaving them out let the diagnostics announce that
    /// the checksum was being computed and that nought filters needed it, in
    /// consecutive clauses of the same sentence.
    pub fn checksum_filter_count(&self) -> usize {
        self.checksum_idx.len() + self.regex_address_idx.len()
    }

    /// How many filters take the general path rather than the byte comparison.
    pub fn pattern_count(&self) -> usize {
        self.patterns.len()
    }

    /// Below this many filters a linear scan beats a binary search: the arrays
    /// fit in cache and the scan has no branch misprediction to pay for.
    const SCAN_LIMIT: usize = 64;

    fn build_buckets(filters: &[Filter]) -> Vec<LengthBucket> {
        if filters.len() <= Self::SCAN_LIMIT {
            return Vec::new();
        }
        let mut by_length: std::collections::BTreeMap<usize, Vec<(u64, u32)>> = Default::default();
        for (i, f) in filters.iter().enumerate() {
            let symbols = f.len().min(KEY_ONLY_SYMBOLS);
            let bits = (symbols * 5).min(64) as u32;
            let mask = if bits == 64 {
                u64::MAX
            } else {
                !0u64 << (64 - bits)
            };
            let mut padded = [0u8; 32];
            padded[..f.bytes.len()].copy_from_slice(&f.bytes);
            by_length
                .entry(symbols)
                .or_default()
                .push((LengthBucket::head(&padded) & mask, i as u32));
        }
        by_length
            .into_iter()
            .map(|(symbols, entries)| LengthBucket::build(symbols, entries))
            .collect()
    }

    /// Consumes the set, returning **every** filter. Used to rebuild with a
    /// different index width without re-parsing.
    ///
    /// Returning only the fast ones silently dropped every substring, wildcard
    /// and suffix on the way from parsing to the engine: the run reported
    /// "0 filters" and found nothing.
    pub fn into_filters(mut self) -> Vec<Filter> {
        self.filters.append(&mut self.patterns);
        self.filters.append(&mut self.regexes);
        self.filters
    }

    /// How much memory the prefix index occupies, or `None` when none is used.
    pub fn index_memory(&self) -> Option<usize> {
        self.index.as_ref().map(BitmapIndex::memory)
    }

    /// Parses a whole list, reporting the first offending entry.
    pub fn parse_all<I, S>(raw: I) -> Result<Self, (String, FilterError)>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut filters = Vec::new();
        for item in raw {
            let text = item.as_ref();
            match Filter::parse(text) {
                Ok(f) => filters.push(f),
                Err(e) => return Err((text.to_string(), e)),
            }
        }
        Ok(Self::new(filters))
    }

    /// Reads filters from a file, one per line.
    ///
    /// Blank lines are skipped, as are lines starting with `#` or `//`. Unlike
    /// the reference implementation there is no line-length cap: a long line is
    /// an error to report, not something to silently split into several
    /// filters.
    pub fn from_file(path: &Path) -> Result<Self, FilterFileError> {
        let lines = read_filter_lines(path)?;
        let filters = lines
            .iter()
            .map(|line| Filter::parse(line).expect("validated by read_filter_lines"))
            .collect();
        Ok(Self::new(filters))
    }

    pub fn len(&self) -> usize {
        self.filters.len() + self.pattern_count() + self.regexes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.filters.is_empty() && self.pattern_count() == 0 && self.regexes.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Filter> {
        self.all_filters()
    }

    /// The share of addresses at least one filter in the set matches.
    ///
    /// The filters are treated as independent. They are not exactly — two
    /// prefixes that share a first symbol are correlated — but each term is
    /// vanishingly small, so the product of the misses is right to many more
    /// digits than an estimate of waiting time can use.
    pub fn probability(&self) -> f64 {
        // Not `1 - prod(1 - p)`. Eleven base32 symbols is a chance of 2.8e-17,
        // and `1.0 - 2.8e-17` rounds to exactly 1.0 in a double, so that form
        // returns zero — which the rest of the program reads as "no address
        // can satisfy this" and says so. `ln_1p` and `exp_m1` keep the small
        // quantity small all the way through.
        let log_miss: f64 = self
            .iter()
            .map(|f| (-f.pattern().probability()).ln_1p())
            .sum();
        -log_miss.exp_m1()
    }

    /// How many candidates it takes to expect one hit, or `None` when the set
    /// matches nothing at all.
    pub fn expected_candidates(&self) -> Option<f64> {
        let p = self.probability();
        (p > 0.0).then(|| 1.0 / p)
    }

    /// The shortest filter in the set, which bounds how selective any prefilter
    /// built over this set can be.
    pub fn min_len(&self) -> usize {
        self.filters.iter().map(Filter::len).min().unwrap_or(0)
    }

    /// The reference matcher: returns the filter an address matches, if any.
    ///
    /// Deliberately the obvious implementation, and deliberately slow: it
    /// builds no index and takes the address as text. It is the oracle the fast
    /// path is tested against.
    pub fn match_address_text(&self, address: &str) -> Option<&Filter> {
        if let Some(found) = self.regexes.iter().find(|f| {
            f.expression()
                .is_some_and(|e| e.find(address.as_bytes()).is_some())
        }) {
            return Some(found);
        }
        self.filters
            .iter()
            .find(|f| address.starts_with(f.canonical()))
    }

    /// The hot-loop matcher: compares against the packed public key directly.
    ///
    /// Computing the address for every candidate would mean a SHA3-256 and a
    /// base32 encoding per candidate — measured at roughly a fourfold drop in
    /// throughput, which is why matching works on key bytes instead.
    ///
    /// A filter longer than [`KEY_ONLY_SYMBOLS`] is only prefiltered here; the
    /// caller confirms it against the full address, which happens so rarely
    /// that its cost does not matter.
    /// The general forms, with the bitmap standing in front of the anchored
    /// ones.
    ///
    /// The bitmap answers for every form at once; only those it cannot key on
    /// are walked one by one. Without it a thousand wildcards would be a
    /// thousand comparisons per candidate.
    #[inline]
    fn match_walk(&self, packed: &[u8; 32]) -> Option<&Filter> {
        if let Some(index) = &self.pattern_index {
            if index.may_match(packed) {
                if let Some(found) = self
                    .anchored_idx
                    .iter()
                    .map(|&i| &self.patterns[i as usize])
                    .find(|f| f.matches_symbols(packed, false))
                {
                    return Some(found);
                }
            }
        }
        // Suffixes are deliberately absent here: one always ends at symbol 55,
        // so its placement never fits inside the key and the key-side check
        // could only ever fail. They are settled by the checksum path instead,
        // where their bitmap stands in front of the walk.
        self.unguarded_idx
            .iter()
            .map(|&i| &self.patterns[i as usize])
            .find(|f| f.matches_symbols(packed, false))
    }

    /// Whether the set holds any form beyond a literal prefix.
    ///
    /// The caller decides on this once per batch. Asking per candidate meant
    /// reading a field of this struct that the literal path otherwise never
    /// touches, which cost about a percent on dictionary search — where the
    /// bitmap rejects almost every candidate and there is little else to pay.
    ///
    /// An expression counts. Leaving it out sent a set holding one down the
    /// loop written for literal prefixes, which asks only the byte comparison
    /// and so found nothing at all — a full run over three billion candidates
    /// reported zero hits where it should have reported some six hundred
    /// thousand.
    pub fn has_patterns(&self) -> bool {
        !self.patterns.is_empty() || !self.regexes.is_empty()
    }

    /// The matcher for a set of nothing but literal prefixes.
    #[inline]
    pub fn match_prefix(&self, packed: &[u8; 32]) -> Option<&Filter> {
        self.match_literals(packed)
    }

    #[inline]
    /// A test on the raw first limb, when the whole set is decided by the
    /// leading bits of the key.
    ///
    /// `None` means no such test exists — a substring or a suffix is not
    /// settled by where the key starts — and the caller must build the
    /// canonical bytes for every candidate as before.
    pub fn leading_probe(&self) -> Option<LimbProbe<'_>> {
        if !self.patterns.is_empty() || !self.regexes.is_empty() {
            return None;
        }
        let index = self.index.as_ref()?;
        index.within_first_limb().then_some(LimbProbe { index })
    }

    pub fn match_key(&self, packed: &[u8; 32]) -> Option<&Filter> {
        if let Some(found) = self.match_literals(packed) {
            return Some(found);
        }
        // One test against an empty vector for every set that uses no general
        // form, which is what keeps prefix-only searches at their old speed.
        // Expressions get the same treatment and the same second test: a load
        // of a length already in cache, against the alternative of a branch
        // inside the walk below.
        if self.patterns.is_empty() {
            return if self.regexes.is_empty() {
                None
            } else {
                self.match_key_regexes_only(packed)
            };
        }
        // No guard on `needs_checksum` below: a substring has dozens of
        // possible offsets and only the last few reach the checksum. Skipping
        // the whole filter would mean it found nothing at all, while
        // `matches_symbols` already ignores the offsets it cannot read.
        if self.text_substrings.is_none() && self.regex_key_idx.is_empty() {
            // `walk_idx` then names every pattern, and walking the vector
            // directly saves the indirection — worth 9% to a set of classes,
            // where there is little else per candidate to hide it behind.
            return self.match_walk(packed);
        }
        // One encoding for all the substrings. The buffer lives only in this
        // branch, so a set without them never pays for it.
        let mut text = [0u8; KEY_ONLY_SYMBOLS];
        crate::base32::encode_into(packed, &mut text);
        if let Some(owner) = self.text_substrings.as_ref().and_then(|s| s.find(&text)) {
            return Some(&self.patterns[owner as usize]);
        }
        if let Some(found) = self.match_walk(packed) {
            return Some(found);
        }
        // Last, after every form with a cheaper answer has declined. The
        // text is already built here, so it is handed over rather than made
        // again.
        self.match_regexes_key(packed, Some(&text))
    }

    /// The key path for a set whose only general form is an expression.
    ///
    /// Split out so the common case above keeps its shape: a set of prefixes
    /// plus one expression should not have to walk an empty `patterns` vector
    /// to find out there is nothing there.
    #[inline]
    fn match_key_regexes_only(&self, packed: &[u8; 32]) -> Option<&Filter> {
        self.match_regexes_key(packed, None)
    }

    /// The general matcher against a full address, for filters that reach the
    /// checksum symbols.
    pub fn match_address(&self, address: &[u8; 35]) -> Option<&Filter> {
        self.checksum_idx
            .iter()
            .map(|&i| &self.patterns[i as usize])
            .find(|f| f.matches_symbols(address, true))
    }

    /// Whether any checksum-reaching form is still possible for this key.
    ///
    /// A suffix long enough to start before symbol 51 has part of itself inside
    /// the key, and that part is decidable for free. Only candidates that
    /// survive this pay for a SHA3-256, which is what keeps a long suffix from
    /// costing what a three-symbol one does.
    #[inline]
    pub fn checksum_possible(&self, packed: &[u8; 32]) -> bool {
        self.checksum_alive(packed, &self.checksum_probe_idx)
    }

    /// Whether any checksum-reaching form is still alive, with the suffix
    /// bitmap standing in front of the suffixes.
    ///
    /// The bitmap answers for every suffix at once. Asking each in turn
    /// whether its one placement is still possible is what a dictionary of
    /// them would otherwise cost.
    #[inline]
    fn checksum_alive(&self, packed: &[u8; 32], probe: &[u32]) -> bool {
        if let Some(index) = &self.suffix_index {
            if index.may_match(packed)
                && self
                    .suffix_idx
                    .iter()
                    .any(|&i| self.patterns[i as usize].key_prefilter(packed))
            {
                return true;
            }
        }
        probe
            .iter()
            .any(|&i| self.patterns[i as usize].key_prefilter(packed))
    }

    #[inline]
    fn match_literals(&self, packed: &[u8; 32]) -> Option<&Filter> {
        if let Some(index) = &self.index {
            // One load and one bit test. Almost every candidate stops here,
            // and it costs the same whether the set holds one filter or a
            // million.
            if !index.may_match(packed) {
                return None;
            }
        }
        if self.buckets.is_empty() {
            return self.filters.iter().find(|f| f.matches_key(packed));
        }
        // Past the scan limit: one binary search per distinct filter length.
        let head = LengthBucket::head(packed);
        for bucket in &self.buckets {
            for &owner in bucket.lookup(head) {
                let f = &self.filters[owner as usize];
                if f.matches_key(packed) {
                    return Some(f);
                }
            }
        }
        None
    }
}

/// Default width of the bitmap index, in bits of the packed key.
///
/// `2^24` bits is 2 MB, which fits L2 on a typical machine. False-hit density
/// is `N · 32^-m` for `N` filters of `m` symbols, so at four symbols and a
/// thousand filters roughly one candidate in a thousand needs an exact check.
pub const DEFAULT_INDEX_BITS: u32 = 24;

/// The width to use when the configuration does not name one.
///
/// A fixed width is wrong at both ends: too narrow and the map saturates, so
/// every candidate falls through to the exact check; too wide and it stops
/// fitting in cache, which costs more than the false hits it prevents. The
/// balance moves with the number of filters (x86, matching in isolation):
///
/// | filters | 18 bits | 20 bits | 24 bits | 27 bits |
/// |---|---|---|---|---|
/// | 1 000 | **100.1 M/s** | 95.3 | 51.0 | 47.3 |
/// | 10 000 | 80.1 | **83.6** | 49.0 | 46.3 |
/// | 100 000 | — | 34.9 | **45.5** | 36.0 |
/// | 1 000 000 | — | — | **25.9** | 25.4 |
///
/// A filter longer than `bits / 5` symbols occupies exactly one slot, so the
/// occupancy of `n` filters is `n / 2^bits` and the rule below is simply "the
/// smallest width whose false-hit rate stays at or under one percent".
///
/// The ceiling of 24 bits is measured: at a million filters the 2 MiB map beats
/// the 16 MiB one although its false-hit rate is eight times worse. Reading the
/// machine's cache size would add nothing — the answer does not move.
pub fn choose_index_bits(filters: usize) -> u32 {
    if filters == 0 {
        return NARROWEST_INDEX_BITS;
    }
    let wanted = (filters as f64 / TARGET_FALSE_HITS).log2().ceil() as u32;
    wanted.clamp(NARROWEST_INDEX_BITS, WIDEST_INDEX_BITS)
}

/// Below this the map saturates faster than it saves.
const NARROWEST_INDEX_BITS: u32 = 16;
/// Above this the map stops paying for the cache it displaces.
const WIDEST_INDEX_BITS: u32 = 24;
/// The false-hit rate a width is chosen to hold, when what stands behind the
/// index is cheap.
///
/// Behind the prefix index is a binary search over length buckets, so a false
/// hit costs almost nothing and one in a hundred is fine. Behind the pattern
/// and suffix indexes is a walk over every form in their group, so a false hit
/// there costs a thousand times more when there are a thousand forms, and the
/// tolerable rate has to fall in the same proportion. Using this figure for
/// all three cost the general forms a factor of three at a thousand filters —
/// measured, before the rate was made proportional.
const TARGET_FALSE_HITS: f64 = 0.01;

/// The narrowest width whose false-hit rate is acceptable.
///
/// Counting filters is enough for literal prefixes, where each occupies one
/// slot. It is not enough for the other forms: one wildcard inside the indexed
/// span fills 32 slots and a class fills several, so the same count can mean
/// very different occupancy. Building and looking is exact, costs a few
/// milliseconds once, and covers both.
fn narrowest_acceptable<T>(
    target: f64,
    mut build: impl FnMut(u32) -> Option<(BitmapIndex, T)>,
) -> Option<(BitmapIndex, T)> {
    let mut widest = None;
    for bits in NARROWEST_INDEX_BITS..=WIDEST_INDEX_BITS {
        let Some((index, extra)) = build(bits) else {
            continue;
        };
        if index.occupancy() <= target {
            return Some((index, extra));
        }
        widest = Some((index, extra));
    }
    widest
}

/// The rate to aim for when a false hit leads to a walk over `behind` forms.
fn target_for_walk(behind: usize) -> f64 {
    TARGET_FALSE_HITS / (behind.max(1) as f64)
}

/// The occupancy above which the index is discarded.
///
/// What decides the index's worth is how much of it a filter set fills, not how
/// long the filters are: a prefix of `m` symbols covers `2^(k-5m)` entries, so
/// a thousand two-symbol filters saturate the map while a single one occupies a
/// thousandth of it. Occupancy is measured on the built map rather than
/// estimated, because filters overlap.
///
/// A quarter full means three candidates in four are still rejected by one bit
/// test, which beats a scan for any filter count; past that the exact check
/// runs often enough that the extra memory traffic stops paying for itself.
pub const MAX_INDEX_OCCUPANCY: f64 = 0.25;

/// A prefix index over a filter set.
///
/// One load and one bit test per candidate, independent of how many filters
/// there are. A prefix is a contiguous range of the index, so adding a filter
/// is a range fill — the flattening that explodes the reference's table under
/// `OMITMASK` costs nothing here.
#[derive(Debug)]
struct BitmapIndex {
    bits: u32,
    /// Where in the key the indexed window starts, in bits.
    ///
    /// Zero for the forms anchored to the start of the address. A suffix is
    /// anchored to the other end, so its window sits at the end of the
    /// key-derived span instead — same structure, different bits read.
    offset: u32,
    words: Vec<u64>,
}

impl BitmapIndex {
    /// Builds an index, or `None` when one would not discriminate.
    fn build(filters: &[Filter], bits: u32) -> Option<Self> {
        if filters.is_empty() || !(8..=32).contains(&bits) {
            return None;
        }

        let mut index = BitmapIndex {
            bits,
            offset: 0,
            words: vec![0u64; (1usize << bits).div_ceil(64)],
        };
        for f in filters {
            index.insert(f);
        }

        // A saturated map passes everything through to the scan and costs a
        // cache line per candidate for nothing.
        (index.occupancy() <= MAX_INDEX_OCCUPANCY).then_some(index)
    }

    /// Builds an index over forms anchored to the start of the address.
    ///
    /// A literal prefix fills a contiguous run, because everything below its
    /// last fixed bit varies freely. A form with free positions does not: the
    /// fixed bits are scattered, so the slots it can occupy are enumerated
    /// instead. One wildcard inside the indexed span is 32 slots, a class of
    /// four is four; two wildcards is a thousand, and past a point the
    /// enumeration is not worth doing at all.
    ///
    /// Returns the index and which of the given patterns it actually covers.
    /// A pattern too broad to enumerate is left out and must still be checked
    /// one by one — saying so is the difference between a prefilter and a
    /// wrong answer.
    fn build_over_patterns(filters: &[Filter], idx: &[u32], bits: u32) -> Option<(Self, Vec<u32>)> {
        if idx.is_empty() || !(8..=32).contains(&bits) {
            return None;
        }
        let mut index = BitmapIndex {
            bits,
            offset: 0,
            words: vec![0u64; (1usize << bits).div_ceil(64)],
        };
        let mut covered = Vec::with_capacity(idx.len());
        for &i in idx {
            // A pattern too broad to enumerate is simply not covered; the
            // others still get their index, and this one stays on the walk.
            let Some(slots) = index.pattern_slots(filters[i as usize].pattern()) else {
                continue;
            };
            for slot in slots {
                index.set(slot);
            }
            covered.push(i);
        }
        if covered.is_empty() || index.occupancy() > MAX_INDEX_OCCUPANCY {
            return None;
        }
        Some((index, covered))
    }

    /// Builds an index over suffixes, keyed on the end of the key-derived
    /// span rather than its beginning.
    ///
    /// A suffix is anchored too, just at the other end: symbols `56 - L` to
    /// `55`, of which everything below 51 is a function of the key alone. The
    /// window sits at the end of that span, so a long suffix pins it exactly
    /// and a short one leaves its upper bits free.
    ///
    /// Short suffixes therefore fill a great many slots and are left out by
    /// the budget: at 24 bits a suffix of ten symbols takes one slot, of eight
    /// takes 512, of six takes half a million. That is not a loss worth
    /// mourning — a six-symbol suffix carries 17 bits and is found in
    /// hundredths of a second.
    fn build_over_suffixes(filters: &[Filter], idx: &[u32], bits: u32) -> Option<(Self, Vec<u32>)> {
        // The whole key-derived span, not the part the hot loop can read
        // exactly: this is a prefilter, and [`suffix_slots`] widens the one
        // symbol the deferred sign bit touches rather than giving up on it.
        // Ending the window two symbols earlier instead would have cost every
        // suffix of eight and nine symbols its index.
        let span = (KEY_DERIVED_SYMBOLS * 5) as u32;
        if idx.is_empty() || !(8..=32).contains(&bits) || bits > span {
            return None;
        }
        let mut index = BitmapIndex {
            bits,
            offset: span - bits,
            words: vec![0u64; (1usize << bits).div_ceil(64)],
        };
        let mut covered = Vec::with_capacity(idx.len());
        for &i in idx {
            let Some(slots) = index.suffix_slots(filters[i as usize].pattern()) else {
                continue;
            };
            for slot in slots {
                index.set(slot);
            }
            covered.push(i);
        }
        if covered.is_empty() || index.occupancy() > MAX_INDEX_OCCUPANCY {
            return None;
        }
        Some((index, covered))
    }

    /// Every slot a suffix can occupy within the window at the end of the key.
    fn suffix_slots(&self, pattern: &Pattern) -> Option<Vec<usize>> {
        const BUDGET: usize = 4096;

        let start = ADDRESS_LEN - pattern.symbols.len();
        let window_lo = self.offset as usize;
        let window_hi = window_lo + self.bits as usize;
        let mut values: Vec<usize> = vec![0];

        for symbol in (window_lo / 5)..=((window_hi - 1) / 5) {
            let lo = (symbol * 5).max(window_lo);
            let hi = (symbol * 5 + 5).min(window_hi);
            let width = hi - lo;
            let shift = window_hi - hi;
            // The part of this symbol the window actually sees.
            let part = |v: u8| usize::from(v >> (5 - (hi - symbol * 5))) & ((1 << width) - 1);

            let mut set = if symbol >= start {
                pattern.symbols[symbol - start]
            } else {
                SymbolSet::ALL
            };
            if symbol == SIGN_SYMBOL {
                set = either_sign(set);
            }
            let mut parts: Vec<usize> = (0u8..32).filter(|v| set.contains(*v)).map(part).collect();
            parts.sort_unstable();
            parts.dedup();

            if values.len() * parts.len() > BUDGET {
                return None;
            }
            let mut next = Vec::with_capacity(values.len() * parts.len());
            for &base in &values {
                for &p in &parts {
                    next.push(base | (p << shift));
                }
            }
            values = next;
        }
        Some(values)
    }

    /// Every slot a start-anchored pattern can occupy, or `None` when there
    /// are more of them than indexing could be worth.
    fn pattern_slots(&self, pattern: &Pattern) -> Option<Vec<usize>> {
        /// Past this many slots for one filter the map fills with it alone.
        const BUDGET: usize = 4096;

        let full = (self.bits / 5) as usize;
        let rem = (self.bits % 5) as usize;
        let at = |i: usize| pattern.symbols.get(i).copied().unwrap_or(SymbolSet::ALL);

        let mut values: Vec<usize> = vec![0];
        for i in 0..full {
            let allowed: Vec<usize> = (0u8..32)
                .filter(|v| at(i).contains(*v))
                .map(usize::from)
                .collect();
            if values.len() * allowed.len() > BUDGET {
                return None;
            }
            let shift = self.bits as usize - 5 * (i + 1);
            let mut next = Vec::with_capacity(values.len() * allowed.len());
            for &base in &values {
                for &v in &allowed {
                    next.push(base | (v << shift));
                }
            }
            values = next;
        }
        if rem > 0 {
            // The last symbol is only partly inside the index: its top bits.
            let mut tops: Vec<usize> = (0u8..32)
                .filter(|v| at(full).contains(*v))
                .map(|v| usize::from(v >> (5 - rem)))
                .collect();
            tops.sort_unstable();
            tops.dedup();
            if values.len() * tops.len() > BUDGET {
                return None;
            }
            let mut next = Vec::with_capacity(values.len() * tops.len());
            for &base in &values {
                for &t in &tops {
                    next.push(base | t);
                }
            }
            values = next;
        }
        Some(values)
    }

    /// The fraction of slots set, measured rather than estimated.
    fn occupancy(&self) -> f64 {
        let set: u32 = self.words.iter().map(|w| w.count_ones()).sum();
        f64::from(set) / (1u64 << self.bits) as f64
    }

    /// The index of a packed key: its top `bits` bits, big-endian.
    #[inline(always)]
    fn slot(&self, packed: &[u8; 32]) -> usize {
        let byte = (self.offset / 8) as usize;
        let within = self.offset % 8;
        // Eight bytes from there cover any 32-bit window plus its alignment.
        // Past the end of the key the bits read as zero, which is right: the
        // window never reaches further than the key-derived span.
        let mut buf = [0u8; 8];
        let take = (packed.len() - byte).min(8);
        buf[..take].copy_from_slice(&packed[byte..byte + take]);
        let word = u64::from_be_bytes(buf);
        ((word << within) >> (64 - self.bits)) as usize
    }

    /// Marks every slot a filter can occupy.
    fn insert(&mut self, f: &Filter) {
        let symbols = f.len().min(KEY_ONLY_SYMBOLS);
        let prefix_bits = (symbols * 5) as u32;

        if prefix_bits >= self.bits {
            // More specific than the index: exactly one slot.
            let mut probe = [0u8; 32];
            probe[..f.bytes.len()].copy_from_slice(&f.bytes);
            let slot = self.slot(&probe);
            self.set(slot);
            return;
        }

        // Less specific: a contiguous run of slots, filled wholesale.
        let mut probe = [0u8; 32];
        probe[..f.bytes.len()].copy_from_slice(&f.bytes);
        let first = self.slot(&probe);
        let run = 1usize << (self.bits - prefix_bits);
        for slot in first..first + run {
            self.set(slot);
        }
    }

    #[inline(always)]
    fn set(&mut self, slot: usize) {
        self.words[slot >> 6] |= 1u64 << (slot & 63);
    }

    #[inline(always)]
    fn may_match(&self, packed: &[u8; 32]) -> bool {
        let slot = self.slot(packed);
        self.words[slot >> 6] & (1u64 << (slot & 63)) != 0
    }

    /// Whether the indexed window lies inside the first eight bytes, which is
    /// what lets the engine ask without building the canonical encoding.
    fn within_first_limb(&self) -> bool {
        self.offset + self.bits <= 64
    }

    /// The same test, from the first eight bytes read big-endian. Valid only
    /// when [`BitmapIndex::within_first_limb`] holds.
    #[inline(always)]
    fn may_match_head(&self, head: u64) -> bool {
        let slot = ((head << self.offset) >> (64 - self.bits)) as usize;
        self.words[slot >> 6] & (1u64 << (slot & 63)) != 0
    }

    /// Bytes occupied, for reporting.
    fn memory(&self) -> usize {
        self.words.len() * 8
    }
}

/// A test the engine can make on a candidate before it has been reduced.
///
/// The hot loop holds each candidate as four 64-bit limbs. Building the
/// canonical 32 bytes for every one of them, only for the index to decline
/// almost all of them, is work the index itself can avoid: the first eight
/// bytes of the canonical encoding are the first limb, and canonicalising
/// changes that limb by at most two subtractions of the modulus — which adds
/// 19 each time. Testing all three forms can therefore say "no" and never say
/// it wrongly.
pub struct LimbProbe<'a> {
    index: &'a BitmapIndex,
}

impl LimbProbe<'_> {
    /// Whether a candidate whose first limb is this could match anything.
    #[inline(always)]
    pub fn may_match(&self, limb0: u64) -> bool {
        let mut v = limb0;
        for _ in 0..3 {
            if self.index.may_match_head(v.swap_bytes()) {
                return true;
            }
            v = v.wrapping_add(19);
        }
        false
    }
}

/// Filters of one length, sorted for binary search.
///
/// The bitmap says a candidate *might* match; something then has to say which
/// filter, and scanning them all makes that step cost `O(N)`. At a thousand
/// filters that is invisible, at a million it dominates everything: measured at
/// 5 000 candidates/s against 32 million for the engine around it.
///
/// Filters are bucketed by length because the comparison mask depends on it.
/// The number of distinct lengths in a dictionary is small — usually one — so
/// the lookup costs one binary search per length.
#[derive(Debug)]
struct LengthBucket {
    /// Bits of the packed key this length covers, capped at 64.
    mask: u64,
    /// Masked 64-bit prefixes, sorted.
    keys: Vec<u64>,
    /// Index into `FilterSet::filters`, parallel to `keys`.
    owners: Vec<u32>,
}

impl LengthBucket {
    /// The leading 64 bits of a packed key, which every comparison starts from.
    #[inline(always)]
    fn head(packed: &[u8; 32]) -> u64 {
        u64::from_be_bytes([
            packed[0], packed[1], packed[2], packed[3], packed[4], packed[5], packed[6], packed[7],
        ])
    }

    fn build(symbols: usize, entries: Vec<(u64, u32)>) -> Self {
        let bits = (symbols * 5).min(64) as u32;
        let mask = if bits == 64 {
            u64::MAX
        } else {
            !0u64 << (64 - bits)
        };
        let mut entries = entries;
        entries.sort_unstable_by_key(|(k, _)| *k);
        LengthBucket {
            mask,
            keys: entries.iter().map(|(k, _)| *k).collect(),
            owners: entries.iter().map(|(_, o)| *o).collect(),
        }
    }

    /// Candidate filter indices whose leading bits match, as a slice of the
    /// sorted arrays. Usually empty; occasionally one entry.
    #[inline(always)]
    fn lookup(&self, head: u64) -> &[u32] {
        let probe = head & self.mask;
        match self.keys.binary_search(&probe) {
            Err(_) => &[],
            Ok(found) => {
                // Equal 64-bit prefixes can repeat only for filters longer than
                // 12 symbols, where the key does not hold the whole prefix.
                let mut start = found;
                while start > 0 && self.keys[start - 1] == probe {
                    start -= 1;
                }
                let mut end = found + 1;
                while end < self.keys.len() && self.keys[end] == probe {
                    end += 1;
                }
                &self.owners[start..end]
            }
        }
    }
}

/// A failure while reading a filter file.
#[derive(Debug)]
pub enum FilterFileError {
    Io(std::io::Error),
    Invalid {
        line: usize,
        text: String,
        source: FilterError,
    },
}

impl fmt::Display for FilterFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FilterFileError::Io(e) => write!(f, "could not read the filter file: {e}"),
            FilterFileError::Invalid { line, text, source } => {
                write!(f, "line {line}: {text:?}: {source}")
            }
        }
    }
}

impl std::error::Error for FilterFileError {}

#[cfg(test)]
mod tests {

    /// A long filter is rare, not impossible. The naive `1 - prod(1 - p)`
    /// underflows to zero from eleven symbols on, and everything downstream
    /// reads that as "no address can satisfy this".
    #[test]
    fn a_long_filter_keeps_a_chance_above_zero() {
        for len in 4..=16usize {
            let text: String = "abcdefghijklmnop".chars().take(len).collect();
            let set = FilterSet::parse_all([text.as_str()]).expect("a valid filter");
            let p = set.probability();
            assert!(
                p > 0.0,
                "{len} symbols came out as impossible: probability {p}"
            );
            let expected = 32f64.powi(-(len as i32));
            assert!(
                (p / expected - 1.0).abs() < 1e-6,
                "{len} symbols: got {p}, expected about {expected}"
            );
            assert!(set.expected_candidates().is_some(), "{len} symbols");
        }
    }

    /// Two filters are more likely than one, and the sum stays right where the
    /// terms are tiny.
    #[test]
    fn several_filters_add_up_without_losing_the_small_ones() {
        let one = FilterSet::parse_all(["abcdefghijkl"]).expect("valid");
        let two = FilterSet::parse_all(["abcdefghijkl", "mnopqrstuvwx"]).expect("valid");
        assert!(two.probability() > one.probability());
        assert!(
            (two.probability() / (2.0 * one.probability()) - 1.0).abs() < 1e-6,
            "two equally rare filters should be twice as likely"
        );
    }
    use super::*;

    /// `symbol_at` has to agree with the base32 encoding, or every pattern
    /// form would read the wrong positions.
    #[test]
    fn symbol_extraction_agrees_with_base32() {
        use crate::key;
        for seed in 0..64u32 {
            let mut b = [0u8; 32];
            b[..4].copy_from_slice(&seed.to_le_bytes());
            let pk = key::public_key(&b);
            let address = key::address(&pk);
            let full = key::address_bytes(&pk);
            for (m, want) in address.chars().enumerate() {
                assert_eq!(
                    symbol_at(&full, m),
                    base32::symbol_value(want as u8).unwrap(),
                    "seed {seed} position {m}"
                );
            }
        }
    }

    #[test]
    fn parses_every_form() {
        assert_eq!(Pattern::parse("abc").unwrap().anchor, Anchor::Start);
        assert_eq!(Pattern::parse("prefix:abc").unwrap().anchor, Anchor::Start);
        assert_eq!(Pattern::parse("^abc").unwrap().anchor, Anchor::Start);
        assert_eq!(
            Pattern::parse("contains:abc").unwrap().anchor,
            Anchor::Anywhere
        );
        assert_eq!(Pattern::parse("suffix:aqd").unwrap().anchor, Anchor::End);
        assert_eq!(
            Pattern::parse("  CONTAINS: abc  ").unwrap().anchor,
            Anchor::Anywhere,
            "form names and spacing must be forgiving"
        );
    }

    #[test]
    fn parses_wildcards_and_classes() {
        let p = Pattern::parse("a?c").unwrap();
        assert_eq!(p.len(), 3);
        assert!(!p.is_literal());
        assert!(p.symbols[1] == SymbolSet::ALL);

        let p = Pattern::parse("a[bc]d").unwrap();
        assert_eq!(p.len(), 3, "a class occupies one position");
        assert!(p.symbols[1].contains(base32::symbol_value(b'b').unwrap()));
        assert!(p.symbols[1].contains(base32::symbol_value(b'c').unwrap()));
        assert!(!p.symbols[1].contains(base32::symbol_value(b'd').unwrap()));

        assert!(Pattern::parse("abcd").unwrap().is_literal());
    }

    /// The tail of a v3 address is fixed by the version byte, so most suffixes
    /// can never occur. Accepting one means searching forever, which is exactly
    /// what the reference implementation does in regex mode.
    #[test]
    fn rejects_unreachable_suffixes() {
        // Must end in 'd'.
        match Pattern::parse("suffix:abc") {
            Err(FilterError::UnreachableTail {
                position_from_end, ..
            }) => assert_eq!(position_from_end, 1),
            other => panic!("expected a tail rejection, got {other:?}"),
        }
        // The penultimate symbol has only four possible values.
        match Pattern::parse("suffix:abd") {
            Err(FilterError::UnreachableTail {
                position_from_end, ..
            }) => assert_eq!(position_from_end, 2),
            other => panic!("expected a tail rejection, got {other:?}"),
        }
        // These are reachable.
        for good in [
            "suffix:d",
            "suffix:ad",
            "suffix:id",
            "suffix:qd",
            "suffix:yd",
            "suffix:zzad",
        ] {
            assert!(Pattern::parse(good).is_ok(), "{good} should be accepted");
        }
        // A wildcard covers the allowed values, so it is reachable.
        assert!(Pattern::parse("suffix:?d").is_ok());
        assert!(Pattern::parse("suffix:[aq]d").is_ok());
        assert!(
            Pattern::parse("suffix:[bc]d").is_err(),
            "a class missing every allowed value is still unreachable"
        );
    }

    #[test]
    fn rejects_malformed_patterns() {
        assert!(matches!(
            Pattern::parse("ab[cd"),
            Err(FilterError::UnbalancedClass { .. })
        ));
        assert!(matches!(
            Pattern::parse("ab]cd"),
            Err(FilterError::UnbalancedClass { .. })
        ));
        assert!(matches!(
            Pattern::parse("ab[]cd"),
            Err(FilterError::EmptyClass { .. })
        ));
        assert!(matches!(
            Pattern::parse("???"),
            Err(FilterError::NothingFixed)
        ));
        assert!(matches!(
            Pattern::parse("nonsense:abc"),
            Err(FilterError::UnknownForm { .. })
        ));
        assert!(matches!(
            Pattern::parse("a0c"),
            Err(FilterError::BadSymbol { position: 1, .. })
        ));
        assert!(matches!(Pattern::parse(""), Err(FilterError::Empty)));
    }

    /// Which forms need the checksum decides what they cost, so the
    /// classification is worth its own test.
    #[test]
    fn checksum_need_follows_the_positions_touched() {
        assert!(!Pattern::parse("abcdef").unwrap().needs_checksum());
        assert!(
            !Pattern::parse(&"a".repeat(KEY_ONLY_SYMBOLS))
                .unwrap()
                .needs_checksum(),
            "51 symbols are still a pure function of the key"
        );
        assert!(
            Pattern::parse(&format!("prefix:{}", "a".repeat(KEY_ONLY_SYMBOLS + 1)))
                .unwrap()
                .needs_checksum(),
            "52 symbols reach into the checksum"
        );
        assert!(
            Pattern::parse("suffix:zzad").unwrap().needs_checksum(),
            "a suffix always touches the tail"
        );
        assert!(
            Pattern::parse("contains:abcdef").unwrap().needs_checksum(),
            "contains may sit at the end, so it can reach the checksum"
        );
    }

    #[test]
    fn offsets_follow_the_anchor() {
        assert_eq!(Pattern::parse("abcd").unwrap().candidate_offsets(), vec![0]);
        assert_eq!(
            Pattern::parse("suffix:zzad").unwrap().candidate_offsets(),
            vec![ADDRESS_LEN - 4]
        );
        let any = Pattern::parse("contains:abcd").unwrap().candidate_offsets();
        assert_eq!(any.len(), ADDRESS_LEN - 4 + 1);
        assert_eq!(any[0], 0);
        assert_eq!(*any.last().unwrap(), ADDRESS_LEN - 4);
    }

    /// Naive check against the address text: what each form is supposed to
    /// mean, written the obvious way. The bit-level matcher is compared against
    /// this, never the other way round.
    fn naive_matches(pattern: &Pattern, address: &str) -> bool {
        let symbols: Vec<u8> = address
            .bytes()
            .map(|c| base32::symbol_value(c).unwrap())
            .collect();
        pattern.candidate_offsets().into_iter().any(|start| {
            start + pattern.symbols.len() <= symbols.len()
                && pattern
                    .symbols
                    .iter()
                    .enumerate()
                    .all(|(i, set)| set.contains(symbols[start + i]))
        })
    }

    /// Every form has to agree with the naive check on real addresses.
    #[test]
    fn forms_agree_with_the_naive_check() {
        use crate::key;

        let forms = [
            "contains:ab",
            "contains:q",
            "a?c",
            "[abc]",
            "?[xyz]",
            "a?[bc]d",
            "prefix:z",
        ];
        let patterns: Vec<Pattern> = forms.iter().map(|f| Pattern::parse(f).unwrap()).collect();
        let filters: Vec<Filter> = forms.iter().map(|f| Filter::parse(f).unwrap()).collect();

        let mut matched = 0usize;
        for seed in 0..3000u32 {
            let mut b = [0u8; 32];
            b[..4].copy_from_slice(&seed.to_le_bytes());
            let pk = key::public_key(&b);
            let address = key::address(&pk);
            let full = key::address_bytes(&pk);

            for (pattern, filter) in patterns.iter().zip(filters.iter()) {
                let want = naive_matches(pattern, &address);
                let got = filter.matches_symbols(&full, true);
                assert_eq!(got, want, "form {:?} disagreed on {address}", pattern.text);
                if want {
                    matched += 1;
                }
            }
        }
        assert!(matched > 0, "the sample produced no matches at all");
    }

    /// Matching against the packed key must equal matching against the full
    /// address for every form that stays inside the key-only span.
    #[test]
    fn key_only_forms_match_the_same_on_key_and_address() {
        use crate::key;

        let forms = ["contains:ab", "a?c", "[abc]d", "prefix:zz"];
        let filters: Vec<Filter> = forms.iter().map(|f| Filter::parse(f).unwrap()).collect();

        for seed in 0..2000u32 {
            let mut b = [0u8; 32];
            b[..4].copy_from_slice(&seed.to_le_bytes());
            let pk = key::public_key(&b);
            let full = key::address_bytes(&pk);

            for filter in &filters {
                let from_key = filter.matches_symbols(&pk, false);
                // Restricting the address check to the same span makes the two
                // comparable: a substring may sit past symbol 51.
                let from_address = filter.matches_symbols(&full, false);
                assert_eq!(
                    from_key,
                    from_address,
                    "form {:?} seed {seed}",
                    filter.source_text()
                );
            }
        }
    }

    /// A substring filter has to find matches from the packed key alone. The
    /// first version gated the whole filter on "needs checksum" and it found
    /// nothing at all, which no test then in place would have caught.
    #[test]
    fn a_substring_finds_matches_without_the_checksum() {
        use crate::key;

        let set = FilterSet::parse_all(["contains:ab"]).unwrap();
        let mut found = 0usize;
        let mut checked_against_text = 0usize;

        for seed in 0..4000u32 {
            let mut b = [0u8; 32];
            b[..4].copy_from_slice(&seed.to_le_bytes());
            let pk = key::public_key(&b);
            if set.match_key(&pk).is_some() {
                found += 1;
                // Where it claims a match, the address really contains it —
                // within the span readable from the key.
                let address = key::address(&pk);
                assert!(
                    address[..KEY_ONLY_SYMBOLS].contains("ab"),
                    "claimed a match on {address}"
                );
                checked_against_text += 1;
            }
        }
        assert!(
            found > 0,
            "a two-symbol substring must occur in 4000 addresses"
        );
        assert_eq!(found, checked_against_text);
    }

    /// A range is only shorthand: it must accept exactly what spelling the
    /// symbols out accepts, and nothing beyond the alphabet's end.
    #[test]
    fn a_class_range_spans_the_alphabet_between_its_ends() {
        let spelled = Pattern::parse("a[bcde]d").unwrap();
        let ranged = Pattern::parse("a[b-e]d").unwrap();
        assert_eq!(spelled.symbols[1], ranged.symbols[1]);

        // The alphabet is a-z then 2-7, so digits below 2 are not symbols at
        // all and a range cannot conjure them.
        assert!(matches!(
            Pattern::parse("a[0-9]d"),
            Err(FilterError::BadSymbol { .. })
        ));
        assert!(matches!(
            Pattern::parse("a[e-b]d"),
            Err(FilterError::EmptyRange { .. })
        ));

        // A range and single symbols mix freely within one class.
        let mixed = Pattern::parse("a[b-eq]d").unwrap();
        for c in *b"bcdeq" {
            assert!(mixed.symbols[1].contains(base32::symbol_value(c).unwrap()));
        }
        assert!(!mixed.symbols[1].contains(base32::symbol_value(b'f').unwrap()));
    }

    /// Filters read from a file must keep their form. They did not: the set
    /// was parsed and then handed on as its filters' canonical text, which
    /// turned `a[b-e]zz` into `abzz` and `contains:qqq` into a prefix.
    #[test]
    fn a_filter_file_keeps_the_form_of_what_it_holds() {
        let path = std::env::temp_dir().join("onion-gen-filter-forms.txt");
        std::fs::write(&path, "contains:qqq\n# a comment\na[b-e]zz\nsuffix:zzad\n").unwrap();

        let lines = read_filter_lines(&path).unwrap();
        assert_eq!(lines, ["contains:qqq", "a[b-e]zz", "suffix:zzad"]);

        // The route the binary takes: text out of the file, parsed once.
        let set = FilterSet::parse_all(&lines).unwrap();
        assert_eq!(set.len(), 3);
        assert_eq!(set.pattern_count(), 3, "none of these is a literal prefix");
        assert_eq!(
            set.checksum_filter_count(),
            2,
            "the substring and the suffix"
        );

        let anchors: Vec<Anchor> = set.iter().map(|f| f.pattern().anchor).collect();
        assert!(anchors.contains(&Anchor::Anywhere));
        assert!(anchors.contains(&Anchor::End));
        assert!(anchors.contains(&Anchor::Start));

        std::fs::remove_file(&path).ok();
    }

    /// The automaton path must find exactly what a naive search of the whole
    /// address finds — including matches that start inside the key and run
    /// into the checksum, which is what the tail prefilter decides.
    ///
    /// A false negative there is the dangerous kind of bug: the search would
    /// simply never report those addresses, and nothing would look wrong.
    #[test]
    fn the_automaton_path_agrees_with_a_naive_search_of_the_address() {
        // Twelve, so the set is over the crossover and uses the automaton.
        let bodies = [
            "zzz", "qqq", "xxx", "jjj", "vvv", "www", "kkk", "ppp", "yyy", "mmm", "bbb", "ccc",
        ];
        let specs: Vec<String> = bodies.iter().map(|b| format!("contains:{b}")).collect();
        let set = FilterSet::parse_all(&specs).unwrap();
        assert!(
            set.prefers_address(),
            "twelve literal substrings must take the automaton path"
        );

        let mut tail_matches = 0usize;
        let mut state = 0x243f_6a88_85a3_08d3u64;
        for seed in 0u32..40000 {
            let mut key = [0u8; 32];
            for chunk in key.chunks_mut(8) {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                chunk.copy_from_slice(&state.to_le_bytes()[..chunk.len()]);
            }
            key[31] &= 0x7f;

            let address = crate::key::address_bytes(&key);
            let text = crate::base32::encode(&address);
            let naive = bodies.iter().any(|b| text.contains(b));

            let ours = match set.examine_key(&key) {
                KeyVerdict::Matched(_) => true,
                KeyVerdict::No => false,
                KeyVerdict::NeedsAddress => set.match_whole_address(&key, &address).is_some(),
            };
            assert_eq!(naive, ours, "seed {seed}, address {text}");

            // Only a match that the key alone cannot see exercises the
            // prefilter, so the test is worth little unless some occur.
            if naive && !bodies.iter().any(|b| text[..KEY_ONLY_SYMBOLS].contains(b)) {
                tail_matches += 1;
            }
        }
        assert!(
            tail_matches > 0,
            "no match landed past the key, so the prefilter was never tested"
        );
    }

    /// The bitmap over anchored forms is a prefilter, and a prefilter that
    /// rejects a real match is worse than no prefilter at all: the search
    /// would simply never report those addresses.
    #[test]
    fn the_pattern_bitmap_never_rejects_a_real_match() {
        let mut specs = Vec::new();
        let alphabet = base32::ALPHABET;
        for i in 0..60usize {
            let a = alphabet[i % 32] as char;
            let b = alphabet[(i * 7 + 3) % 32] as char;
            let c = alphabet[(i * 11 + 5) % 32] as char;
            // A mix of wildcards and classes, all anchored to the start.
            specs.push(match i % 3 {
                0 => format!("{a}?{b}{c}"),
                1 => format!("{a}[{b}{c}]{b}"),
                _ => format!("{a}{b}?{c}"),
            });
        }
        let indexed = FilterSet::parse_all(&specs).unwrap();
        assert!(
            indexed.pattern_index.is_some(),
            "sixty anchored forms should be worth indexing"
        );
        assert_eq!(
            indexed.anchored_idx.len() + indexed.unguarded_idx.len(),
            indexed.walk_idx.len(),
            "every general form must be in exactly one of the two groups"
        );

        let mut state = 0x853c_49e6_748f_ea9bu64;
        let mut matches = 0usize;
        for _ in 0..200_000 {
            let mut packed = [0u8; 32];
            for chunk in packed.chunks_mut(8) {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                chunk.copy_from_slice(&state.to_le_bytes()[..chunk.len()]);
            }
            // The oracle: every form, no index, no grouping.
            let naive = indexed
                .patterns
                .iter()
                .find(|f| f.matches_symbols(&packed, false))
                .map(Filter::canonical);
            let ours = indexed.match_key(&packed).map(Filter::canonical);
            assert_eq!(naive.is_some(), ours.is_some(), "packed {packed:?}");
            if naive.is_some() {
                matches += 1;
            }
        }
        assert!(
            matches > 100,
            "only {matches} matches, too few to have tested anything"
        );
    }

    /// The suffix bitmap guards the checksum prefilter, and a prefilter that
    /// wrongly says "impossible" loses addresses silently.
    #[test]
    fn the_suffix_bitmap_never_rules_out_a_reachable_suffix() {
        // Eight symbols: three of them fall inside the key, which is few
        // enough that survivors actually occur in a sample this size.
        let alphabet = base32::ALPHABET;
        let specs: Vec<String> = (0..40usize)
            .map(|i| {
                let body: String = (0..6)
                    .map(|j| alphabet[(i * 5 + j * 7) % 32] as char)
                    .collect();
                format!("suffix:{body}ad")
            })
            .collect();
        let set = FilterSet::parse_all(&specs).unwrap();
        assert!(
            set.suffix_index.is_some(),
            "forty eight-symbol suffixes should be worth indexing"
        );
        assert_eq!(set.suffix_idx.len(), set.len(), "all of them covered");

        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut alive = 0usize;
        for _ in 0..200_000 {
            let mut packed = [0u8; 32];
            for chunk in packed.chunks_mut(8) {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                chunk.copy_from_slice(&state.to_le_bytes()[..chunk.len()]);
            }
            // The oracle: ask every suffix directly, no bitmap.
            let naive = set.patterns.iter().any(|f| f.key_prefilter(&packed));
            let ours = set.checksum_possible(&packed);
            assert!(
                !naive || ours,
                "the bitmap ruled out a candidate the walk keeps alive"
            );
            if naive {
                alive += 1;
            }
        }
        assert!(alive > 0, "no candidate survived, so nothing was tested");
    }

    /// Declining a structure is a normal outcome, and each reason has to be
    /// distinguishable: they call for different responses from the user.
    #[test]
    fn the_diagnostics_name_the_structure_or_the_reason_there_is_none() {
        let joined = |specs: &[&str]| {
            FilterSet::parse_all(specs)
                .unwrap()
                .describe_structures()
                .join(" | ")
        };

        // The width is chosen from what the set actually fills, so the size
        // is not fixed; what must hold is that a structure is built and says
        // what it costs.
        let one = joined(&["abcdefghij"]);
        assert!(
            one.contains("prefix bitmap: ") && one.contains("KiB"),
            "got {one}"
        );
        let wild = joined(&["a?cdefghij"]);
        assert!(
            wild.contains("anchored-form bitmap: ") && wild.contains("KiB"),
            "got {wild}"
        );

        // A narrow index covers fewer symbols, so what counts as "too broad"
        // moves with the width the set chooses. Four free positions at the
        // front are past the budget at every width.
        assert!(joined(&["??abcde"]).contains("anchored-form bitmap: "));
        let broad = joined(&["????abcd"]);
        assert!(
            broad.contains("anchored-form bitmap: not built"),
            "expected a refusal, got {broad}"
        );
        let mixed = joined(&["a?bcde", "????abcd"]);
        assert!(mixed.contains("1 of 2 form(s)"), "got {mixed}");

        // A suffix short enough to leave most of the window free cannot be
        // keyed on; a long one can.
        assert!(joined(&["suffix:zad"]).contains("suffix bitmap: not built"));
        assert!(joined(&["suffix:abcdefghzad"]).contains("suffix bitmap: "));

        // The one form no structure serves.
        let open = joined(&["contains:a[bc]d"]);
        assert!(
            open.contains("hold a class or a wildcard"),
            "the form nothing indexes must be named, got {open}"
        );
        assert!(!joined(&["contains:abc"]).contains("hold a class"));

        // Which path an expression landed on, for each of the three.
        let bits = joined(&["regex:^shop"]);
        assert!(
            bits.contains("the obligatory prefix \"shop\", compared on the key")
                && bits.contains("within the first 49"),
            "got {bits}"
        );
        let mapped = joined(&["regex:^a[2-7]{3}shop"]);
        assert!(
            mapped.contains("a bitmap of") && mapped.contains("216 obligatory prefixes"),
            "a long literal list must say it was indexed, got {mapped}"
        );
        let open_regex = joined(&["regex:^[bcdfghjklmnp]{4}"]);
        assert!(
            open_regex.contains("the engine sees every candidate"),
            "an expression with no literal must say so, got {open_regex}"
        );
        let tail = joined(&["regex:^zz.*qd$"]);
        assert!(
            tail.contains("reaches the checksum"),
            "an expression over the address must say it costs a hash, got {tail}"
        );

        // The threshold that does not cover them, said once and only when
        // there is something for it to be said about.
        assert!(joined(&["regex:^shop"]).contains("does not cover them"));
        assert!(!joined(&["abcd"]).contains("does not cover them"));
    }

    /// A suffix cannot be settled from the key, and the set must say so: it is
    /// the difference between hashing every candidate and hashing none.
    #[test]
    fn a_suffix_is_kept_apart_from_the_key_readable_forms() {
        let set = FilterSet::parse_all(["abc", "contains:xy", "suffix:zad"]).unwrap();
        assert_eq!(set.len(), 3);
        assert!(set.needs_checksum(), "a suffix reaches past the key");

        // A substring reaches the checksum too, at its last few offsets only,
        // which is why the two groups overlap rather than partition.
        assert!(FilterSet::parse_all(["contains:xy"])
            .unwrap()
            .needs_checksum());
        let only_key = FilterSet::parse_all(["abc", "ab?de"]).unwrap();
        assert!(
            !only_key.needs_checksum(),
            "an anchored form inside the key must not pay for a hash"
        );
    }

    /// The prefilter is what keeps a long suffix cheap: the part of it that
    /// lands inside the key decides most candidates without a hash.
    #[test]
    fn the_key_decides_the_part_of_a_suffix_that_reaches_it() {
        // 10 symbols: placed at 46..55, so 46..50 are readable from the key.
        let long = Filter::parse("suffix:aaaaaaaazad").unwrap();
        // Nothing reaches the key, so every candidate stays possible.
        let short = Filter::parse("suffix:zad").unwrap();

        let mut hits = 0;
        for n in 0u8..64 {
            let mut packed = [0u8; 32];
            packed[20] = n;
            packed[28] = n.wrapping_mul(7);
            assert!(short.key_prefilter(&packed), "nothing to decide on");
            if long.key_prefilter(&packed) {
                hits += 1;
            }
        }
        assert!(hits < 64, "the readable symbols must reject something");
    }

    /// Rebuilding a set must not lose the general forms. It did, and the run
    /// reported "0 filters" while looking perfectly healthy otherwise.
    #[test]
    fn rebuilding_a_set_keeps_every_form() {
        let original = FilterSet::parse_all(["abc", "contains:xy", "de?g", "suffix:zzad"]).unwrap();
        assert_eq!(original.len(), 4);

        let rebuilt = FilterSet::with_index_bits(original.into_filters(), DEFAULT_INDEX_BITS);
        assert_eq!(
            rebuilt.len(),
            4,
            "a rebuild must not drop the general forms"
        );
        assert_eq!(rebuilt.pattern_count(), 3);
        assert!(rebuilt.needs_checksum());
    }

    /// A literal prefix must keep taking the byte path, and everything else
    /// must not.
    #[test]
    fn only_literal_prefixes_take_the_fast_path() {
        assert!(Filter::parse("abcdef").unwrap().is_fast());
        assert!(Filter::parse("prefix:abcdef").unwrap().is_fast());
        assert!(!Filter::parse("contains:abcdef").unwrap().is_fast());
        assert!(!Filter::parse("suffix:zzad").unwrap().is_fast());
        assert!(!Filter::parse("ab?d").unwrap().is_fast());
        assert!(!Filter::parse("a[bc]d").unwrap().is_fast());

        let set = FilterSet::parse_all(["abc", "contains:xy", "de?g"]).unwrap();
        assert_eq!(set.len(), 3);
        assert_eq!(set.pattern_count(), 2, "two forms take the general path");
    }

    /// A set of plain prefixes must not acquire the general path at all — the
    /// requirement that these forms cost nothing to those who do not use them.
    #[test]
    fn a_prefix_only_set_has_no_general_path() {
        let set = FilterSet::parse_all(["abc", "xyz", "qrst"]).unwrap();
        assert_eq!(set.pattern_count(), 0);
        assert!(!set.needs_checksum());
        assert!(set.index_memory().is_some(), "the index still applies");
    }

    #[test]
    fn checksum_need_is_reported_for_the_whole_set() {
        assert!(!FilterSet::parse_all(["abc", "a?c"])
            .unwrap()
            .needs_checksum());
        assert!(FilterSet::parse_all(["abc", "suffix:zzad"])
            .unwrap()
            .needs_checksum());
    }

    #[test]
    fn accepts_plain_prefixes() {
        assert_eq!(Filter::parse("abcd").unwrap().canonical(), "abcd");
        assert_eq!(Filter::parse("ABCD").unwrap().canonical(), "abcd");
        assert_eq!(Filter::parse("^abcd").unwrap().canonical(), "abcd");
        assert_eq!(Filter::parse("  abcd  ").unwrap().canonical(), "abcd");
        assert_eq!(Filter::parse("a2b7").unwrap().canonical(), "a2b7");
    }

    /// Digits absent from the base32 alphabet make a filter unreachable. The
    /// reference accepts them silently in regex mode, leaving the user to
    /// search forever.
    #[test]
    fn rejects_unreachable_filters() {
        for (raw, position) in [("a0b", 1), ("a1b", 1), ("ab8", 2), ("9ab", 0)] {
            match Filter::parse(raw) {
                Err(FilterError::BadSymbol { position: p, .. }) => assert_eq!(p, position, "{raw}"),
                other => panic!("{raw} should have been rejected, got {other:?}"),
            }
        }
        assert_eq!(Filter::parse(""), Err(FilterError::Empty));
        assert_eq!(Filter::parse("^"), Err(FilterError::Empty));

        let long = "a".repeat(ADDRESS_LEN + 1);
        assert_eq!(
            Filter::parse(&long),
            Err(FilterError::TooLong {
                length: ADDRESS_LEN + 1
            })
        );
        assert!(Filter::parse(&"a".repeat(ADDRESS_LEN)).is_ok());
    }

    #[test]
    fn duplicates_and_absorbed_filters_are_dropped() {
        let set = FilterSet::parse_all(["ab", "abc", "ab", "abcd", "xy"]).unwrap();
        let kept: Vec<&str> = set.iter().map(Filter::canonical).collect();
        assert_eq!(kept, vec!["ab", "xy"]);
    }

    #[test]
    fn matching_finds_the_prefix() {
        let set = FilterSet::parse_all(["abc", "xyz"]).unwrap();
        assert_eq!(
            set.match_address_text("abcdef…").map(Filter::canonical),
            Some("abc")
        );
        assert_eq!(
            set.match_address_text("xyzabc…").map(Filter::canonical),
            Some("xyz")
        );
        assert!(set.match_address_text("qqqqq…").is_none());
    }

    #[test]
    fn a_file_skips_comments_and_blanks() {
        let path = std::env::temp_dir().join(format!("onion-gen-filters-{}", std::process::id()));
        std::fs::write(&path, "# a comment\n\n  abc  \n// another\nxyz\n\n\tqrs\n").unwrap();
        let set = FilterSet::from_file(&path).unwrap();
        let kept: Vec<&str> = set.iter().map(Filter::canonical).collect();
        assert_eq!(kept, vec!["abc", "qrs", "xyz"]);
        std::fs::remove_file(&path).unwrap();
    }

    /// The fast path must agree with the reference matcher on real keys. A
    /// divergence here would mean silently missed or invented hits.
    #[test]
    fn key_matching_agrees_with_address_matching() {
        use crate::key;

        let set = FilterSet::parse_all(["ab", "q", "zz7", "mnop"]).unwrap();
        let mut seed = [0u8; 32];
        let mut checked = 0usize;

        for n in 0..4000u32 {
            seed[..4].copy_from_slice(&n.to_le_bytes());
            let pk = key::public_key(&seed);
            let address = key::address(&pk);

            let by_key = set.match_key(&pk).map(Filter::canonical);
            let by_address = set.match_address_text(&address).map(Filter::canonical);
            assert_eq!(by_key, by_address, "disagreement on {address}");
            if by_key.is_some() {
                checked += 1;
            }
        }
        assert!(checked > 0, "the sample produced no hits at all");
    }

    /// Builds a packed key whose leading bits equal a filter's, with the rest
    /// filled from `noise`. Searching for such a key by brute force would take
    /// a million candidates per four symbols, so it is constructed instead.
    fn key_matching(f: &Filter, noise: u64) -> [u8; 32] {
        let mut packed = [0u8; 32];
        let mut x = noise | 1;
        for chunk in packed.chunks_mut(8) {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            chunk.copy_from_slice(&x.to_le_bytes());
        }
        let n = f.bytes.len();
        packed[..n - 1].copy_from_slice(&f.bytes[..n - 1]);
        packed[n - 1] = (packed[n - 1] & !f.mask) | f.bytes[n - 1];
        packed
    }

    fn random_key(noise: u64) -> [u8; 32] {
        let mut packed = [0u8; 32];
        let mut x = noise.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        for chunk in packed.chunks_mut(8) {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            chunk.copy_from_slice(&x.to_le_bytes());
        }
        packed
    }

    /// The index must never hide a real match. This is the property that
    /// matters: a prefilter that drops hits is worse than no prefilter, and the
    /// failure is silent.
    #[test]
    fn the_index_never_loses_a_match() {
        let raw = [
            "abcd",
            "abcdef",
            "zzzz",
            "qrst",
            "mnopqr",
            "a".repeat(20).leak() as &str,
        ];
        let parsed: Vec<Filter> = raw.iter().map(|r| Filter::parse(r).unwrap()).collect();

        let indexed = FilterSet::new(parsed.clone());
        assert!(
            indexed.index_memory().is_some(),
            "an index was expected here"
        );
        // The same filters with the index suppressed, as the oracle.
        let plain = FilterSet::with_index_bits(parsed.clone(), 7);
        assert!(plain.index_memory().is_none());

        let mut matches = 0usize;
        for n in 0..20000u64 {
            // Half the sample is constructed to match, so the "no false
            // negatives" property is actually exercised.
            let packed = if n % 2 == 0 {
                key_matching(&indexed.filters[(n as usize / 2) % indexed.len()], n)
            } else {
                random_key(n)
            };
            let with = indexed.match_key(&packed).map(Filter::canonical);
            let without = plain.match_key(&packed).map(Filter::canonical);
            assert_eq!(with, without, "index disagreed on sample {n}");
            if with.is_some() {
                matches += 1;
            }
        }
        assert!(
            matches >= 9000,
            "expected the constructed half to match, got {matches}"
        );
    }

    /// The index is kept or dropped by how full it is, not by filter length.
    /// A single short filter still discriminates; a thousand of them do not.
    #[test]
    fn a_saturated_index_is_discarded() {
        // One two-symbol filter fills 1/1024 of the map: worth keeping.
        let one_short = FilterSet::parse_all(["ab"]).unwrap();
        assert!(one_short.index_memory().is_some());

        // Every two-symbol prefix fills all of it: worthless.
        let alphabet = "abcdefghijklmnopqrstuvwxyz234567";
        let all_pairs: Vec<String> = alphabet
            .chars()
            .flat_map(|a| alphabet.chars().map(move |b| format!("{a}{b}")))
            .collect();
        let saturated = FilterSet::parse_all(&all_pairs).unwrap();
        assert_eq!(saturated.len(), 1024);
        assert!(
            saturated.index_memory().is_none(),
            "a full map must be discarded"
        );

        // A thousand three-symbol filters fill about 3%: still worth keeping.
        let three: Vec<String> = (0..1000)
            .map(|i: usize| {
                let c = |n: usize| alphabet.as_bytes()[n % 32] as char;
                format!("{}{}{}", c(i), c(i / 32), c(i / 1024 + 7))
            })
            .collect();
        let sparse = FilterSet::parse_all(&three).unwrap();
        assert!(
            sparse.index_memory().is_some(),
            "a sparse map must be kept whatever the filter length"
        );
    }

    /// A prefix shorter than the index width occupies a contiguous run of
    /// slots, and every key sharing that prefix must land inside it.
    #[test]
    fn a_short_prefix_fills_a_contiguous_run() {
        // Four symbols is 20 bits against a 24-bit index: a run of 16 slots.
        let set = FilterSet::parse_all(["abcd"]).unwrap();
        let f = Filter::parse("abcd").unwrap();
        for n in 0..5000u64 {
            let packed = key_matching(&f, n);
            assert!(
                set.match_key(&packed).is_some(),
                "a real match fell outside the index at sample {n}"
            );
        }
    }

    /// A filter more specific than the index occupies exactly one slot, and a
    /// key differing inside the indexed span must not reach the exact check.
    #[test]
    fn a_long_prefix_occupies_one_slot() {
        let set = FilterSet::parse_all(["abcdefgh"]).unwrap();
        let f = Filter::parse("abcdefgh").unwrap();
        for n in 0..2000u64 {
            assert!(set.match_key(&key_matching(&f, n)).is_some(), "sample {n}");
        }
        let mut miss = 0usize;
        for n in 0..2000u64 {
            if set.match_key(&random_key(n)).is_none() {
                miss += 1;
            }
        }
        assert_eq!(miss, 2000, "random keys must not match an 8-symbol filter");
    }

    /// Past the scan limit the exact check switches to a sorted lookup. It has
    /// to agree with the scan on every candidate, including filters of mixed
    /// lengths and filters longer than a 64-bit key can hold.
    #[test]
    fn sorted_lookup_agrees_with_the_scan() {
        let alphabet = "abcdefghijklmnopqrstuvwxyz234567";
        let mut raw: Vec<String> = Vec::new();
        let mut x: u64 = 0xDEAD_BEEF_1234_5678;
        // Several lengths at once, so more than one bucket is in play. Each
        // length gets its own leading symbol, otherwise a short filter absorbs
        // the longer ones and the set collapses to one bucket.
        for (group, length) in [4usize, 6, 9, 14].into_iter().enumerate() {
            for _ in 0..200usize {
                let mut text = String::with_capacity(length);
                text.push(alphabet.as_bytes()[group] as char);
                for _ in 1..length {
                    x ^= x << 13;
                    x ^= x >> 7;
                    x ^= x << 17;
                    text.push(alphabet.as_bytes()[(x % 32) as usize] as char);
                }
                raw.push(text);
            }
        }
        let parsed: Vec<Filter> = raw.iter().map(|r| Filter::parse(r).unwrap()).collect();

        let bucketed = FilterSet::new(parsed.clone());
        assert!(
            bucketed.len() > FilterSet::SCAN_LIMIT,
            "the set must be large enough to use buckets"
        );
        assert!(!bucketed.buckets.is_empty(), "buckets were expected");

        // A set below the limit uses the scan; build one holding the same
        // filters by calling the matcher on the filters directly.
        let scan = |packed: &[u8; 32]| -> Option<&Filter> {
            bucketed.filters.iter().find(|f| f.matches_key(packed))
        };

        let mut matched = 0usize;
        for n in 0..40000u64 {
            // Half constructed to match a real filter, half random.
            let packed = if n % 2 == 0 {
                key_matching(&bucketed.filters[(n as usize / 2) % bucketed.len()], n)
            } else {
                random_key(n)
            };
            let fast = bucketed.match_key(&packed).map(Filter::canonical);
            let slow = scan(&packed).map(Filter::canonical);
            assert_eq!(fast, slow, "lookup disagreed with the scan on sample {n}");
            if fast.is_some() {
                matched += 1;
            }
        }
        assert!(
            matched >= 18000,
            "expected the constructed half to match, got {matched}"
        );
    }

    #[test]
    fn index_size_follows_its_width() {
        let filters: Vec<Filter> = ["abcdef"]
            .iter()
            .map(|r| Filter::parse(r).unwrap())
            .collect();
        assert_eq!(
            FilterSet::with_index_bits(filters.clone(), 16).index_memory(),
            Some(8 * 1024)
        );
        assert_eq!(
            FilterSet::with_index_bits(filters, DEFAULT_INDEX_BITS).index_memory(),
            Some(2 * 1024 * 1024)
        );
    }

    /// A filter longer than the key-only span must be flagged for confirmation
    /// rather than silently accepted on a partial match.
    #[test]
    fn long_filters_require_a_full_check() {
        let short = Filter::parse("abcdef").unwrap();
        assert!(!short.needs_full_check());

        let exactly = Filter::parse(&"a".repeat(KEY_ONLY_SYMBOLS)).unwrap();
        assert!(!exactly.needs_full_check());

        let long = Filter::parse(&"a".repeat(KEY_ONLY_SYMBOLS + 2)).unwrap();
        assert!(long.needs_full_check());
    }

    #[test]
    fn a_bad_line_names_its_number() {
        let path = std::env::temp_dir().join(format!("onion-gen-bad-{}", std::process::id()));
        std::fs::write(&path, "abc\nxyz\na0b\n").unwrap();
        match FilterSet::from_file(&path) {
            Err(FilterFileError::Invalid { line, text, .. }) => {
                assert_eq!(line, 3);
                assert_eq!(text, "a0b");
            }
            other => panic!("expected a line-numbered error, got {other:?}"),
        }
        std::fs::remove_file(&path).unwrap();
    }
}

#[cfg(test)]
mod probability_tests {
    use super::*;

    fn p(raw: &str) -> f64 {
        Pattern::parse(raw).expect("a valid filter").probability()
    }

    #[test]
    fn a_prefix_costs_thirty_two_per_symbol() {
        for n in 1..=8usize {
            let filter = "a".repeat(n);
            let want = 32f64.powi(-(n as i32));
            let got = p(&filter);
            assert!(
                (got - want).abs() < want * 1e-9,
                "{n} symbols: {got} against {want}"
            );
        }
    }

    #[test]
    fn the_tail_is_cheaper_because_the_protocol_pays_for_it() {
        // The last symbol is fixed and the one before it takes four values, so
        // a three-symbol suffix costs 128 candidates and not 32768: one free
        // position, one of four, one of one.
        assert!(
            (p("suffix:qad") - 1.0 / 128.0).abs() < 1e-12,
            "{}",
            p("suffix:qad")
        );
        // One symbol further left is a free position again.
        assert!((p("suffix:aqad") - 1.0 / 4096.0).abs() < 1e-15);
        // A prefix of the same length pays for all three positions.
        assert!((p("qad") - 32f64.powi(-3)).abs() < 1e-18);
    }

    #[test]
    fn a_class_is_its_size_over_the_alphabet() {
        assert!((p("[ab]") - 2.0 / 32.0).abs() < 1e-12);
        assert!((p("[a-p]") - 16.0 / 32.0).abs() < 1e-12);
        // A wildcard is the whole alphabet, so it costs nothing.
        assert!((p("a?") - 1.0 / 32.0).abs() < 1e-12);
    }

    #[test]
    fn a_substring_is_worth_its_placements() {
        // 56 - 3 + 1 placements, each 32^-3, minus the overlap the sum ignores.
        let one = 32f64.powi(-3);
        let got = p("contains:abc");
        assert!(got > one * 50.0 && got <= one * 54.0, "{got}");
    }

    #[test]
    fn a_set_is_worth_the_sum_of_its_filters() {
        let set = FilterSet::parse_all(["aaaa", "bbbb", "cccc"]).expect("valid");
        let one = 32f64.powi(-4);
        let got = set.probability();

        // Exactly one minus the chance of missing all three.
        let exact = 1.0 - (1.0 - one).powi(3);
        assert!((got - exact).abs() < exact * 1e-12, "{got} against {exact}");

        // Which is the sum of the three, short by the second-order term. That
        // is the whole difference between "add them up" and the truth, and at
        // this scale it is one part in a million.
        let sum = 3.0 * one;
        let shortfall = (sum - got) / sum;
        assert!(
            shortfall > 0.0 && shortfall < 2.0 * one,
            "the overlap is {shortfall}, not of order {one}"
        );

        let expected = set.expected_candidates().expect("reachable");
        assert!((expected - 1.0 / got).abs() < 1e-6);
    }

    /// The point of the whole calculation: two symbols of filter are three
    /// orders of magnitude of waiting.
    #[test]
    fn two_more_symbols_is_a_thousand_times_the_wait() {
        let eight = FilterSet::parse_all(["aaaaaaaa"]).expect("valid");
        let ten = FilterSet::parse_all(["aaaaaaaaaa"]).expect("valid");
        let ratio = ten.expected_candidates().expect("reachable")
            / eight.expected_candidates().expect("reachable");
        assert!((ratio - 1024.0).abs() < 1.0, "{ratio}");
    }
}
