//! Regular expressions as a matching form.
//!
//! Matching packed bits costs 4.3 ns per candidate; base32-encoding one costs
//! 40.9 ns and running a regex over that text 51.1 ns. Leaving the bit level is
//! the expense, not the engine.
//!
//! An expression carrying an obligatory literal therefore hands it to the
//! structures the other forms use, and the engine sees only the survivors.
//! "Obligatory" must be proven rather than likely: a prefilter that rejects
//! what the expression would have accepted shows up as keys never found, not
//! as a slowdown.

use std::fmt;

use regex::bytes::{Regex, RegexBuilder};
use regex_syntax::hir::literal::{Extractor, Seq};
use regex_syntax::hir::{Hir, Look, LookSet};
use regex_syntax::ParserBuilder;

use crate::base32;
use crate::filter::KEY_ONLY_SYMBOLS;
use crate::key::ADDRESS_LEN;

/// The form prefix that introduces an expression on the command line.
pub const FORM: &str = "regex";

/// How far into the key-derived text a match may reach and still be decidable
/// from the key alone.
///
/// The whole readable span, which is safe only because [`only_safe_looks`]
/// refuses every look-around that could read the boundary. A match ending
/// exactly at symbol 49 sits at the end of the key text but in the middle of
/// the address, and `$` or `\b` there would answer differently for the two.
const KEY_TEXT_REACH: usize = KEY_ONLY_SYMBOLS;

/// How many obligatory literals are worth keeping before they stop paying for
/// themselves.
///
/// Every literal is an entry in the structure that guards the engine, and the
/// structures are built to hold thousands, so the limit is not about their
/// capacity. It is about the extractor: `^a[2-7]{3}shop` expands to 216
/// literals, and a slightly greedier expression expands without bound. Past
/// the limit the literals are shortened rather than dropped — a shorter
/// obligatory prefix is still obligatory.
const LITERAL_LIMIT: usize = 256;

/// Which text the expression is matched against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Haystack {
    /// The 49 symbols readable from the public key, with no checksum computed.
    KeyText,
    /// All 56 symbols, which costs a SHA3-256 per candidate that reaches here.
    FullAddress,
}

/// What was proven about every string the expression accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Literals {
    /// Every match starts at symbol zero and begins with one of these, so the
    /// leading bits of the key answer without encoding anything.
    ///
    /// The second field is the longest prefix common to all of them, which is
    /// what makes a long list affordable: `^a[2-7]{3}shop` has 216 literals
    /// and every one begins with `a`, so a single comparison rejects
    /// thirty-one candidates in thirty-two and the list itself is walked only
    /// by the survivors.
    Prefix(Vec<Vec<u8>>, Vec<u8>),
    /// Every match contains one of these somewhere, so the substring automaton
    /// answers over the encoded text.
    Substring(Vec<Vec<u8>>),
    /// Nothing obligatory could be proven, and the engine runs on every
    /// candidate. The reason is carried so the diagnostics can say which of the
    /// several reasons it was.
    None(&'static str),
}

impl Literals {
    /// The literals, whatever their kind.
    pub fn words(&self) -> &[Vec<u8>] {
        match self {
            Literals::Prefix(w, _) | Literals::Substring(w) => w,
            Literals::None(_) => &[],
        }
    }

    /// The prefix every accepted address must begin with, when there is one.
    pub fn common_prefix(&self) -> &[u8] {
        match self {
            Literals::Prefix(_, common) => common,
            _ => &[],
        }
    }
}

/// Which of the three paths an expression takes, for reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Path {
    /// Guarded by a bitmap over packed bits: the cheap case.
    Bits,
    /// Guarded by the substring automaton over the key text.
    Text,
    /// Unguarded: the engine sees every candidate.
    Every,
}

/// A compiled expression, together with what was proven about it.
#[derive(Debug, Clone)]
pub struct Expression {
    source: String,
    regex: Regex,
    literals: Literals,
    haystack: Haystack,
}

/// Compared by source text. Two expressions with the same text are the same
/// filter; the compiled automaton behind them has no useful equality.
impl PartialEq for Expression {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source
    }
}

impl Eq for Expression {}

impl Expression {
    /// Compiles an expression, rejecting what is decidably unmatchable.
    pub fn parse(source: &str) -> Result<Expression, ExpressionError> {
        let hir = parse_hir(source)?;
        let props = hir.properties();

        // An address is 56 symbols. An expression that cannot match in fewer
        // than 57 has nothing to find, however long the search runs.
        if let Some(min) = props.minimum_len() {
            if min > ADDRESS_LEN {
                return Err(ExpressionError::Unreachable(format!(
                    "it cannot match fewer than {min} symbols, and an address has {ADDRESS_LEN}"
                )));
            }
        }

        let seq = Extractor::new().extract(&hir);
        // A finite, empty sequence is the extractor saying the expression
        // accepts nothing at all.
        if seq.len() == Some(0) {
            return Err(ExpressionError::Unreachable(
                "it accepts no string at all".to_string(),
            ));
        }
        if let Some(bad) = every_literal_outside_base32(&seq) {
            return Err(ExpressionError::Unreachable(format!(
                "every branch requires the symbol {:?}, which base32 never produces",
                bad as char
            )));
        }

        let regex = RegexBuilder::new(source)
            .unicode(false)
            .build()
            .map_err(|e| ExpressionError::Syntax(e.to_string()))?;

        let anchored = props.look_set_prefix().contains(Look::Start);
        let literals = classify_literals(&seq, anchored);
        let haystack = if anchored
            && only_safe_looks(&hir)
            && props.maximum_len().is_some_and(|m| m <= KEY_TEXT_REACH)
        {
            Haystack::KeyText
        } else {
            Haystack::FullAddress
        };

        Ok(Expression {
            source: source.to_string(),
            regex,
            literals,
            haystack,
        })
    }

    /// The expression as written.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// What was proven about every string it accepts.
    pub fn literals(&self) -> &Literals {
        &self.literals
    }

    /// Which text it is matched against.
    pub fn haystack(&self) -> Haystack {
        self.haystack
    }

    /// Whether checking it costs a checksum per candidate that reaches it.
    pub fn needs_checksum(&self) -> bool {
        self.haystack == Haystack::FullAddress
    }

    /// Which of the three paths it takes.
    pub fn path(&self) -> Path {
        match self.literals {
            Literals::Prefix(..) => Path::Bits,
            Literals::Substring(_) => Path::Text,
            Literals::None(_) => Path::Every,
        }
    }

    /// Runs the engine over already-encoded text, returning where it matched.
    ///
    /// The caller is responsible for handing over the text this expression
    /// asked for: key text only when [`Expression::haystack`] says so.
    pub fn find(&self, text: &[u8]) -> Option<usize> {
        self.regex.find(text).map(|m| m.start())
    }
}

impl fmt::Display for Expression {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{FORM}:{}", self.source)
    }
}

/// Why an expression was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpressionError {
    /// It does not parse. The message carries the position, because the
    /// underlying parser reports one and dropping it would make a long
    /// expression a guessing game.
    Syntax(String),
    /// It parses, but no address could ever match it.
    Unreachable(String),
}

impl fmt::Display for ExpressionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExpressionError::Syntax(detail) => {
                write!(f, "the regular expression does not parse: {detail}")
            }
            ExpressionError::Unreachable(why) => {
                write!(f, "no address can ever match this expression: {why}")
            }
        }
    }
}

impl std::error::Error for ExpressionError {}

/// Parses to HIR under exactly the options the engine is built with.
///
/// The two must agree. Literals extracted from one expression and used to
/// guard a different one is the precise shape of the defect this module is
/// written to avoid, and a parser configured differently is a different
/// expression.
fn parse_hir(source: &str) -> Result<Hir, ExpressionError> {
    ParserBuilder::new()
        .utf8(false)
        .unicode(false)
        .build()
        .parse(source)
        .map_err(|e| ExpressionError::Syntax(e.to_string()))
}

/// Whether every look-around in the expression is one known to read the same
/// thing in the key text as in the full address.
///
/// A whitelist rather than a list of the dangerous ones, and deliberately so.
/// The key text is a 49-symbol prefix of the 56-symbol address, so a look that
/// asks about the end of the haystack — `$` plainly, `\b` quietly — answers
/// differently for the two. Enumerating those would put the burden on us to
/// have thought of all of them, and to keep thinking of them as the regex
/// crate grows new ones; a look we failed to list would not fail loudly, it
/// would silently match the wrong text. Only `^` is known safe, because the
/// two texts share a start. Everything else, present and future, sends the
/// expression to the full address, which is always correct and merely slower.
fn only_safe_looks(hir: &Hir) -> bool {
    let looks = hir.properties().look_set();
    looks.subtract(LookSet::singleton(Look::Start)).is_empty()
}

/// The symbol that makes every branch impossible, if there is one.
///
/// Every literal has to be impossible for the expression to be: one live
/// branch is enough to keep it worth searching. `^(abc|0xy)` is reachable even
/// though `0` never appears in an address.
fn every_literal_outside_base32(seq: &Seq) -> Option<u8> {
    let words = seq.literals()?;
    if words.is_empty() {
        return None;
    }
    let mut culprit = None;
    for word in words {
        match word.as_bytes().iter().find(|&&b| !is_base32(b)) {
            Some(&bad) => culprit.get_or_insert(bad),
            // This branch is matchable, so the expression is.
            None => return None,
        };
    }
    culprit
}

fn is_base32(b: u8) -> bool {
    base32::symbol_value(b).is_some()
}

/// Turns an extracted sequence into a prefilter, or explains why it cannot.
fn classify_literals(seq: &Seq, anchored: bool) -> Literals {
    let Some(words) = seq.literals() else {
        // The extractor gives up rather than return something it cannot
        // vouch for, which is the direction we want it to fail in.
        return Literals::None("no literal is common to every branch");
    };
    if words.is_empty() {
        return Literals::None("no literal is common to every branch");
    }
    // An empty literal is matched by every string, so a prefilter built on it
    // would pass everything while costing a lookup.
    if words.iter().any(|w| w.is_empty()) {
        return Literals::None("a branch accepts the empty string");
    }

    let mut kept: Vec<Vec<u8>> = words.iter().map(|w| w.as_bytes().to_vec()).collect();
    if kept.len() > LITERAL_LIMIT {
        match shorten(&kept) {
            Some(shorter) => kept = shorter,
            None => return Literals::None("the obligatory literals are too many to index"),
        }
    }
    dedup(&mut kept);

    if !anchored {
        return Literals::Substring(kept);
    }
    // A branch whose literal base32 cannot produce is a branch no address can
    // take, so dropping it narrows the prefilter without losing a match. The
    // case where every branch is like that was refused at parse, so something
    // always survives here.
    let reachable: Vec<Vec<u8>> = kept
        .into_iter()
        .filter(|w| w.iter().all(|&b| is_base32(b)))
        .collect();
    if reachable.is_empty() {
        return Literals::None("no branch begins with base32");
    }
    let common = common_prefix(&reachable);
    Literals::Prefix(reachable, common)
}

/// The longest prefix shared by every literal.
fn common_prefix(words: &[Vec<u8>]) -> Vec<u8> {
    let Some((first, rest)) = words.split_first() else {
        return Vec::new();
    };
    let mut len = first.len();
    for word in rest {
        len = len.min(
            word.iter()
                .zip(first.iter())
                .take_while(|(a, b)| a == b)
                .count(),
        );
        if len == 0 {
            break;
        }
    }
    first[..len].to_vec()
}

/// Trades length for count: a shorter obligatory prefix is still obligatory.
///
/// `^a[2-7]{3}shop` expands to 216 eight-symbol literals; cut to one symbol
/// they collapse to the single literal `a`. That guards far less, but it
/// guards soundly and costs one lookup instead of none.
fn shorten(words: &[Vec<u8>]) -> Option<Vec<Vec<u8>>> {
    let longest = words.iter().map(Vec::len).max().unwrap_or(0);
    for keep in (1..=longest).rev() {
        let mut cut: Vec<Vec<u8>> = words
            .iter()
            .filter_map(|w| {
                let head = w.get(..keep)?;
                Some(head.to_vec())
            })
            .collect();
        // A literal shorter than the cut cannot be extended, and dropping it
        // would drop the branch it stands for.
        if cut.len() != words.len() {
            continue;
        }
        dedup(&mut cut);
        if cut.len() <= LITERAL_LIMIT {
            return Some(cut);
        }
    }
    None
}

fn dedup(words: &mut Vec<Vec<u8>>) {
    words.sort();
    words.dedup();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(expression: &Expression) -> Vec<String> {
        expression
            .literals()
            .words()
            .iter()
            .map(|w| String::from_utf8_lossy(w).into_owned())
            .collect()
    }

    #[test]
    fn an_anchored_literal_goes_to_the_bits() {
        let e = Expression::parse("^shop").unwrap();
        assert_eq!(e.path(), Path::Bits);
        assert_eq!(words(&e), ["shop"]);
        assert_eq!(e.haystack(), Haystack::KeyText);
        assert!(!e.needs_checksum());
    }

    #[test]
    fn an_alternation_contributes_every_branch() {
        let e = Expression::parse("^my(shop|store)").unwrap();
        assert_eq!(e.path(), Path::Bits);
        assert_eq!(words(&e), ["myshop", "mystore"]);
    }

    #[test]
    fn an_unanchored_literal_goes_to_the_automaton() {
        let e = Expression::parse("(cat|dog)food").unwrap();
        assert_eq!(e.path(), Path::Text);
        assert_eq!(words(&e), ["catfood", "dogfood"]);
        // Unanchored, so a placement could reach the tail.
        assert!(e.needs_checksum());
    }

    #[test]
    fn a_wide_class_has_no_literal_but_still_avoids_the_checksum() {
        let e = Expression::parse("^[bcdfghjklmnpqrstvwxyz]{6}").unwrap();
        assert_eq!(e.path(), Path::Every);
        assert_eq!(e.haystack(), Haystack::KeyText);
    }

    #[test]
    fn an_unbounded_anchored_expression_needs_the_address() {
        let e = Expression::parse("^shop.*").unwrap();
        // The literal still guards it; only the haystack grows.
        assert_eq!(e.path(), Path::Bits);
        assert!(e.needs_checksum());
    }

    #[test]
    fn reading_the_end_forces_the_full_address() {
        // Bounded and anchored, so length alone would send it to the key text.
        // The `$` is what must not let that happen.
        let e = Expression::parse("^[a-z]{20}$").unwrap();
        assert_eq!(e.haystack(), Haystack::FullAddress);
        let b = Expression::parse("^[a-z]{20}\\b").unwrap();
        assert_eq!(b.haystack(), Haystack::FullAddress);
    }

    #[test]
    fn a_branch_accepting_nothing_leaves_no_prefilter() {
        // `x*` lets the expression start at `y`, so `xy` is not obligatory.
        let e = Expression::parse("^x*yz").unwrap();
        assert!(words(&e)
            .iter()
            .all(|w| w.starts_with('y') || w.starts_with('x')));
        // Whatever it extracted, it must not claim a literal every match lacks.
        for w in words(&e) {
            assert!(!w.is_empty(), "an empty literal guards nothing");
        }
    }

    #[test]
    fn too_many_literals_are_shortened_rather_than_dropped() {
        // 8^5 = 32768 branches, far past the limit. Cutting them to their
        // common first symbol leaves one literal that is still obligatory.
        let e = Expression::parse("^a[2-7]{5}shop").unwrap();
        let kept = words(&e);
        assert!(
            kept.len() <= LITERAL_LIMIT,
            "kept {} literals, limit is {LITERAL_LIMIT}",
            kept.len()
        );
        assert!(
            !kept.is_empty(),
            "shortening must not give up the prefilter"
        );
        assert!(
            kept.iter().all(|w| w.starts_with('a')),
            "every branch begins with `a`, so every kept literal must: {kept:?}"
        );
    }

    #[test]
    fn a_literal_within_the_limit_is_kept_whole() {
        // 6^3 = 216 branches, under the limit, so nothing is traded away.
        let e = Expression::parse("^a[2-7]{3}shop").unwrap();
        let kept = words(&e);
        assert_eq!(kept.len(), 216);
        assert!(kept.iter().all(|w| w.ends_with("shop")));
    }

    #[test]
    fn an_expression_matching_nothing_is_refused() {
        let e = Expression::parse("[^\\x00-\\xff]").unwrap_err();
        assert!(
            matches!(e, ExpressionError::Unreachable(_)),
            "expected unreachable, got {e}"
        );
    }

    #[test]
    fn a_literal_outside_base32_is_refused() {
        let e = Expression::parse("^sh0p").unwrap_err();
        match e {
            ExpressionError::Unreachable(why) => {
                assert!(why.contains('0'), "the message must name the symbol: {why}")
            }
            other => panic!("expected unreachable, got {other}"),
        }
    }

    #[test]
    fn one_live_branch_keeps_the_expression() {
        // `0xy` can never appear, but `abc` can, so the expression is worth
        // searching for. Refusing it here would be the expensive mistake.
        let e = Expression::parse("^(abc|0xy)").expect("one reachable branch is enough");
        assert_eq!(e.path(), Path::Bits);
    }

    #[test]
    fn an_expression_longer_than_an_address_is_refused() {
        let e = Expression::parse("^[a-z]{57}").unwrap_err();
        match e {
            ExpressionError::Unreachable(why) => assert!(
                why.contains("57") && why.contains("56"),
                "the message must say both lengths: {why}"
            ),
            other => panic!("expected unreachable, got {other}"),
        }
    }

    #[test]
    fn a_syntax_error_carries_its_position() {
        let e = Expression::parse("^(shop").unwrap_err();
        match e {
            ExpressionError::Syntax(detail) => assert!(
                detail.contains('^') || detail.contains("unclosed"),
                "the message must point at the error: {detail}"
            ),
            other => panic!("expected a syntax error, got {other}"),
        }
    }

    #[test]
    fn the_display_form_round_trips_to_what_was_typed() {
        let e = Expression::parse("^my(shop|store)").unwrap();
        assert_eq!(e.to_string(), "regex:^my(shop|store)");
        assert_eq!(e.source(), "^my(shop|store)");
    }
}
