//! How good a find is, expressed as how rare it is.
//!
//! A broad filter produces finds in the thousands and they are not equally
//! good: `contains:shop` is one address in thirty-odd thousand. "Better" here
//! means "rarer", measured over five million random addresses rather than
//! asserted. The tables below are that measurement, and a test re-derives them
//! on a fresh sample.
//!
//! The total is a sort key and not a probability. Feature families overlap — a
//! run of three identical symbols *is* a palindrome of three — so adding their
//! rarities counts the same event twice, and the search is for whichever
//! feature happens to be there, which is the multiple-comparisons mistake in
//! its usual form. What the total means comes from [`share_at_least`], which is
//! measured over whole addresses and so has the overlap already in it.

use crate::key::ADDRESS_LEN;

/// Where a filter matched, and how many places it could have matched.
///
/// Passed in rather than derived from the filter here, because the offset is
/// read from the packed address and the rest of this module reads the printed
/// one. Mixing the two is not a type error — both are `&[u8]` — and the first
/// attempt did exactly that, asking a symbol-offset matcher to walk ASCII.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    /// The symbol the match starts at, counted from zero.
    pub at: usize,
    /// How many offsets the form could have taken.
    pub of: usize,
}

/// The symbols an address can end with are fixed by the protocol, so the last
/// two carry no information and are left out of every feature.
const FREE: usize = ADDRESS_LEN - 2;

/// The families, fixed in advance rather than chosen after looking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// The longest run of one repeated symbol.
    Run,
    /// The longest stretch of a two-symbol unit repeated, as in `abab`.
    Tiling,
    /// The longest palindrome.
    Palindrome,
    /// How few digits the address has. Digits are `2`–`7`; a random address
    /// carries about ten of them, and having far fewer reads as a word.
    FewDigits,
    /// Where the filter matched, for a form that could have matched elsewhere.
    Placement,
}

impl Family {
    pub fn name(self) -> &'static str {
        match self {
            Family::Run => "run",
            Family::Tiling => "tiling",
            Family::Palindrome => "palindrome",
            Family::FewDigits => "few digits",
            Family::Placement => "placement",
        }
    }
}

/// The longest run of one repeated symbol, over the free part of the address.
pub fn longest_run(address: &[u8]) -> usize {
    let text = &address[..FREE.min(address.len())];
    let mut best = 1usize;
    let mut cur = 1usize;
    for pair in text.windows(2) {
        cur = if pair[0] == pair[1] { cur + 1 } else { 1 };
        best = best.max(cur);
    }
    best
}

/// How many times a two-symbol unit repeats consecutively, at best.
///
/// `abab` counts two, `ababab` three. A unit of one repeated symbol is left to
/// [`longest_run`], which reports it more precisely.
pub fn longest_tiling(address: &[u8]) -> usize {
    let text = &address[..FREE.min(address.len())];
    let mut best = 1usize;
    for start in 0..text.len().saturating_sub(3) {
        let (a, b) = (text[start], text[start + 1]);
        if a == b {
            continue;
        }
        let mut count = 1usize;
        let mut at = start + 2;
        while at + 1 < text.len() && text[at] == a && text[at + 1] == b {
            count += 1;
            at += 2;
        }
        best = best.max(count);
    }
    best
}

/// The longest palindrome, by expansion around each centre.
pub fn longest_palindrome(address: &[u8]) -> usize {
    let text = &address[..FREE.min(address.len())];
    let mut best = 1usize;
    for centre in 0..text.len() {
        for (mut l, mut r) in [
            (centre as isize, centre as isize),
            (centre as isize, centre as isize + 1),
        ] {
            while l >= 0 && (r as usize) < text.len() && text[l as usize] == text[r as usize] {
                l -= 1;
                r += 1;
            }
            best = best.max((r - l - 1) as usize);
        }
    }
    best
}

/// How many digits the address carries. Fewer is rarer.
pub fn digit_count(address: &[u8]) -> usize {
    address[..FREE.min(address.len())]
        .iter()
        .filter(|b| b.is_ascii_digit())
        .count()
}

/// Every family's level for one address.
///
/// A struct rather than a list, because this runs once per find and a find can
/// arrive thousands of times a second: building a vector here measured 34 ns of
/// the 284 the whole scoring cost, spent on an allocation that is freed one
/// line later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Levels {
    pub run: usize,
    pub tiling: usize,
    pub palindrome: usize,
    /// Symbols that are *not* digits, so that a higher level is the rarer one
    /// here as in every other family. Without the inversion the tables would
    /// each need a direction, and one of them would eventually get it wrong.
    pub few_digits: usize,
    /// How near the front the filter matched, for a form that could have
    /// matched elsewhere. `None` for a form anchored to the start, where the
    /// position carries nothing.
    pub placement: Option<usize>,
}

impl Levels {
    /// The level this address reached in one family.
    pub fn of(&self, family: Family) -> Option<usize> {
        match family {
            Family::Run => Some(self.run),
            Family::Tiling => Some(self.tiling),
            Family::Palindrome => Some(self.palindrome),
            Family::FewDigits => Some(self.few_digits),
            Family::Placement => self.placement,
        }
    }
}

/// Measures every family for one address.
///
/// `address` is the printed address. `placement` may be `None` where there is
/// nothing to be relative to, which is how the calibration measures: it runs
/// over addresses nobody asked for.
pub fn levels(address: &[u8], placement: Option<Placement>) -> Levels {
    Levels {
        run: longest_run(address),
        tiling: longest_tiling(address),
        palindrome: longest_palindrome(address),
        few_digits: FREE - digit_count(address),
        placement: placement.map(|p| FREE - p.at.min(FREE)),
    }
}

/// What a level in each family is worth, in bits of rarity.
///
/// Measured over five million random addresses. A level is listed only where
/// fewer than one address in three reaches it: a feature most addresses have
/// distinguishes nothing, and a `run` of two is present in 81%.
///
/// Levels past the measured range are extrapolated only where the slope is a
/// fact rather than a fit. One more symbol in a run is 32 times rarer, so five
/// more bits — the measured steps are +5.0 and +4.9. A tiling unit is two
/// symbols, hence ten. Palindromes have no such slope, since odd and even
/// lengths behave differently, so that family saturates at its last measured
/// level.
mod bits {
    /// `(level, bits)`, ascending. The last entry is the last measured one.
    pub const RUN: &[(usize, f64)] = &[(3, 4.4), (4, 9.4), (5, 14.3)];
    pub const RUN_PER_LEVEL: f64 = 5.0;

    pub const TILING: &[(usize, f64)] = &[(2, 4.4), (3, 14.5)];
    pub const TILING_PER_LEVEL: f64 = 10.0;

    pub const PALINDROME: &[(usize, f64)] =
        &[(4, 3.4), (5, 4.3), (6, 8.4), (7, 9.4), (8, 13.5), (9, 14.4)];

    /// Non-digit symbols out of the 54 free ones. 54 is every one of them, so
    /// this family has a ceiling and needs no extrapolation.
    pub const FEW_DIGITS: &[(usize, f64)] = &[
        (46, 1.8),
        (47, 2.5),
        (48, 3.3),
        (49, 4.5),
        (50, 5.8),
        (51, 7.5),
        (52, 9.7),
        (53, 12.3),
        (54, 15.9),
    ];
}

/// Looks a level up in a measured table, extrapolating past its end only when
/// a per-level slope was given.
fn bits_for(table: &[(usize, f64)], per_level: Option<f64>, level: usize) -> f64 {
    let Some(&(last_level, last_bits)) = table.last() else {
        return 0.0;
    };
    if level > last_level {
        return match per_level {
            Some(step) => last_bits + step * (level - last_level) as f64,
            // Saturated: better than the best measured level, and no honest
            // way to say by how much.
            None => last_bits,
        };
    }
    table
        .iter()
        .rev()
        .find(|(l, _)| *l <= level)
        .map_or(0.0, |(_, b)| *b)
}

/// What one address scored, and where the score came from.
#[derive(Debug, Clone, PartialEq)]
pub struct Score {
    pub total: f64,
    pub parts: Vec<(Family, f64)>,
    pub levels: Levels,
}

/// Scores a find, working out its placement from the filter that matched it.
///
/// Both the single-machine path and the master go through here, so that a key
/// carries the same score whichever found it.
pub fn of_find(public_key: &[u8; 32], filter: &crate::filter::Filter) -> Score {
    // Two forms of the same thing: the filter matches against the raw bytes an
    // address encodes, the score reads the symbols it encodes to.
    let raw = crate::key::address_bytes(public_key);
    let symbols = crate::key::address(public_key);
    let placement = (!filter.is_anchored_at_start())
        .then(|| {
            filter.match_offset(&raw, true).map(|at| Placement {
                at,
                of: filter.pattern().candidate_offsets().len(),
            })
        })
        .flatten();
    score(symbols.as_bytes(), placement)
}

/// Scores one address.
///
/// Runs once per find and never per candidate: 254 ns against the 6 ms a find
/// already spends on a directory and three files, which is the number worth
/// comparing against rather than zero.
pub fn score(address: &[u8], placement: Option<Placement>) -> Score {
    let levels = levels(address, placement);
    let mut parts = Vec::with_capacity(5);
    let mut push = |family: Family, bits: f64| {
        if bits > 0.0 {
            parts.push((family, bits));
        }
    };
    push(
        Family::Run,
        bits_for(bits::RUN, Some(bits::RUN_PER_LEVEL), levels.run),
    );
    push(
        Family::Tiling,
        bits_for(bits::TILING, Some(bits::TILING_PER_LEVEL), levels.tiling),
    );
    push(
        Family::Palindrome,
        bits_for(bits::PALINDROME, None, levels.palindrome),
    );
    push(
        Family::FewDigits,
        bits_for(bits::FEW_DIGITS, None, levels.few_digits),
    );
    if let Some(bits) = placement_bits(placement) {
        push(Family::Placement, bits);
    }
    let total = parts.iter().map(|(_, b)| b).sum();
    Score {
        total,
        parts,
        levels,
    }
}

/// What it is worth that the filter matched where it did.
///
/// Derived rather than measured, because the geometry gives it exactly: a form
/// with `p` possible placements lands at or before offset `k` in about
/// `(k + 1) / p` of the addresses that match it at all, so landing at the very
/// front is worth `log2(p)` bits. This is the only part of the score that
/// knows what was asked for; a form anchored to the start has one placement
/// and so contributes nothing, which is correct rather than a special case.
fn placement_bits(placement: Option<Placement>) -> Option<f64> {
    let p = placement?;
    if p.of <= 1 {
        return None;
    }
    Some((p.of as f64 / (p.at + 1) as f64).log2().max(0.0))
}

/// What a total score means, measured over whole addresses.
///
/// `(score, share of addresses scoring at least that)`, from five million
/// random addresses and confirmed on five million more the table was not built
/// from: every row agreed within three standard errors.
///
/// This table is the reason the total is allowed to be a sum at all. Read as
/// bits, a score of 20 would mean one address in a million; measured, it is one
/// in eleven thousand. The sum overstates rarity some ninety-fold, because the
/// families overlap — a run of three identical symbols is also a palindrome of
/// three — and because the search is for whichever feature happens to be
/// present. Both errors are inside this measurement already, which is why it is
/// this and not the arithmetic that gets shown to anyone.
const CALIBRATION: &[(f64, f64)] = &[
    (0.0, 1.0),
    (2.0, 0.322_294),
    (4.0, 0.182_860),
    (6.0, 0.060_111),
    (8.0, 0.021_377),
    (10.0, 0.009_001),
    (12.0, 0.003_969),
    (14.0, 0.001_362),
    (16.0, 0.000_587),
    (18.0, 0.000_230),
    (20.0, 0.000_087_4),
    (24.0, 0.000_009_4),
];

/// The share of random addresses scoring at least `total`.
///
/// Interpolated geometrically between measured points, because the share falls
/// by a roughly constant factor per step and a straight line in the share
/// itself would run negative. Past the last measured point the answer
/// saturates: the sample held ten addresses above it, which is enough to say
/// "rarer than this" and not enough to say how much rarer.
pub fn share_at_least(total: f64) -> f64 {
    let first = CALIBRATION[0];
    if total <= first.0 {
        return first.1;
    }
    for pair in CALIBRATION.windows(2) {
        let ((x0, y0), (x1, y1)) = (pair[0], pair[1]);
        if total <= x1 {
            let t = (total - x0) / (x1 - x0);
            return y0 * (y1 / y0).powf(t);
        }
    }
    CALIBRATION[CALIBRATION.len() - 1].1
}

/// "one in N", the form the share is worth showing in.
pub fn one_in(total: f64) -> f64 {
    let share = share_at_least(total);
    if share <= 0.0 {
        f64::INFINITY
    } else {
        1.0 / share
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 54 free symbols carrying none of the features: no run of three, no
    /// two-symbol tiling, no palindrome of five, and twelve digits, which is
    /// about what a random address has. Everything below is this string with
    /// one feature put into it, so a difference in score is that feature and
    /// nothing else.
    const PLAIN: &str = "454ml6lg4tjfcz4kaedcmpb5u4mos7af5r2fquosbegzgsyebannd6";

    /// A full address: 54 free symbols plus the tail the protocol fixes.
    fn addr(free: &str) -> Vec<u8> {
        assert_eq!(free.len(), ADDRESS_LEN - 2, "the free part is 54 symbols");
        let mut out = free.as_bytes().to_vec();
        out.extend_from_slice(b"qd");
        out
    }

    /// `PLAIN` with `feature` written over its first symbols.
    fn with(feature: &str) -> Vec<u8> {
        let mut free = feature.to_string();
        free.push_str(&PLAIN[feature.len()..]);
        addr(&free)
    }

    #[test]
    fn the_plain_address_really_is_plain() {
        let l = levels(&addr(PLAIN), None);
        assert_eq!((l.run, l.tiling), (2, 1), "levels were {l:?}");
        assert!(l.palindrome < 5, "levels were {l:?}");
        assert_eq!(score(&addr(PLAIN), None).total, 0.0);
    }

    #[test]
    fn a_run_is_found() {
        assert_eq!(longest_run(b"abaaaab"), 4);
        assert_eq!(longest_run(b"abcabc"), 1);
    }

    #[test]
    fn a_tiling_needs_two_different_symbols() {
        assert_eq!(longest_tiling(b"xababaz"), 2);
        assert_eq!(longest_tiling(b"xabababz"), 3);
        // `aaaa` is a run, and calling it a tiling as well would say the same
        // thing twice under two names.
        assert_eq!(longest_tiling(b"aaaa"), 1);
    }

    #[test]
    fn a_palindrome_is_measured_both_ways_round() {
        assert_eq!(longest_palindrome(b"yabcbax"), 5);
        assert_eq!(longest_palindrome(b"yabccbax"), 6);
    }

    #[test]
    fn the_fixed_tail_is_left_out_of_every_feature() {
        // The protocol pins the last symbol at `d`, so an address ending in
        // `ddqd` would otherwise be credited with a run nobody achieved.
        let free = format!("{}ddd", &PLAIN[..51]);
        let l = levels(&addr(&free), None);
        assert_eq!(l.run, 3, "the run must stop at the free symbols: {l:?}");
        assert_eq!(digit_count(&addr(PLAIN)), 12);
    }

    #[test]
    fn a_rarer_feature_scores_higher_than_a_commoner_one() {
        let three = score(&with("aaa"), None).total;
        let four = score(&with("aaaa"), None).total;
        let five = score(&with("aaaaa"), None).total;
        assert!(
            0.0 < three && three < four && four < five,
            "runs should order: {three} {four} {five}"
        );
    }

    #[test]
    fn a_feature_most_addresses_have_scores_nothing() {
        // A run of two is present in 81% of addresses, so it is not in the
        // table and must contribute zero rather than a small number.
        assert_eq!(score(&addr(PLAIN), None).total, 0.0);
    }

    #[test]
    fn placement_counts_only_where_the_form_could_have_matched_elsewhere() {
        // A prefix: one placement, so nothing to say about where it landed.
        assert!(placement_bits(Some(Placement { at: 0, of: 1 })).is_none());
        // A substring of three symbols over the address: 52 placements.
        let front = placement_bits(Some(Placement { at: 0, of: 52 })).unwrap();
        let middle = placement_bits(Some(Placement { at: 25, of: 52 })).unwrap();
        let back = placement_bits(Some(Placement { at: 51, of: 52 })).unwrap();
        assert!(
            (front - 52f64.log2()).abs() < 1e-9,
            "the very front is worth log2 of the placements, got {front}"
        );
        assert!(front > middle && middle > back, "{front} {middle} {back}");
        assert!(back < 0.1, "the very back is worth nothing, got {back}");
    }

    #[test]
    fn the_calibration_is_monotonic_and_never_exceeds_one() {
        let mut previous = f64::INFINITY;
        for step in 0..60 {
            let share = share_at_least(step as f64);
            assert!(share <= 1.0 && share > 0.0, "share at {step} is {share}");
            assert!(share <= previous, "share rose at {step}");
            previous = share;
        }
    }

    #[test]
    fn a_score_past_the_measured_range_saturates_rather_than_extrapolating() {
        let last = share_at_least(24.0);
        assert_eq!(share_at_least(40.0), last);
        assert_eq!(share_at_least(400.0), last);
    }

    #[test]
    fn the_measured_points_are_reproduced_exactly() {
        for &(total, share) in CALIBRATION {
            let got = share_at_least(total);
            assert!(
                (got - share).abs() < 1e-9,
                "at {total}: table says {share}, lookup says {got}"
            );
        }
    }
}
