//! How long a search will take, worked out before it starts.
//!
//! Two questions, one answer behind them. "How long for a name like this" needs
//! the odds of the filter; "how long for any name of N symbols" needs the odds
//! of a prefix of that length. Both divide by what this machine actually does,
//! which is measured here rather than assumed, because a figure from someone
//! else's processor is worse than no figure.

use crate::curve::{self, Point};
use crate::filter::FilterSet;
use crate::key;
use crate::pairs::PairEngine;
use std::time::{Duration, Instant};

/// Shortest and longest prefix the survey covers.
pub const SHORTEST: usize = 4;
pub const LONGEST: usize = 12;

/// What one search is expected to cost.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Estimate {
    /// Candidates that have to be examined for an even chance of a find.
    pub candidates: f64,
    /// Seconds to that even chance, at the measured rate.
    pub median_seconds: f64,
    /// Seconds to a nine-in-ten chance. The number worth planning against:
    /// half the time a search takes longer than its median, by definition.
    pub ninety_seconds: f64,
}

impl Estimate {
    /// From the chance that one candidate is a hit.
    ///
    /// The waiting time is exponential, so the median is `ln 2 / p` candidates
    /// and the ninetieth percentile `ln 10 / p` — a factor of 3.3 between them,
    /// which is why quoting the median alone misleads.
    pub fn from_chance(chance: f64, rate: f64) -> Estimate {
        if chance <= 0.0 || !chance.is_finite() {
            return Estimate {
                candidates: f64::INFINITY,
                median_seconds: f64::INFINITY,
                ninety_seconds: f64::INFINITY,
            };
        }
        let candidates = std::f64::consts::LN_2 / chance;
        let per_second = rate.max(1.0);
        Estimate {
            candidates,
            median_seconds: candidates / per_second,
            ninety_seconds: (10f64.ln() / chance) / per_second,
        }
    }
}

/// The chance that one candidate starts with a given number of chosen symbols.
///
/// Base32, so each symbol is one in 32.
pub fn chance_of_prefix(symbols: usize) -> f64 {
    32f64.powi(-(symbols as i32))
}

/// What a search for each prefix length from [`SHORTEST`] to [`LONGEST`] costs.
pub fn by_length(rate: f64) -> Vec<(usize, Estimate)> {
    (SHORTEST..=LONGEST)
        .map(|n| (n, Estimate::from_chance(chance_of_prefix(n), rate)))
        .collect()
}

/// What this filter set costs, when its odds are known.
///
/// `None` when the set contains something whose odds cannot be counted — a
/// regular expression, where the honest answer is that nobody knows.
pub fn for_filters(filters: &FilterSet, rate: f64) -> Option<Estimate> {
    if filters.has_unknown_odds() {
        return None;
    }
    let chance = filters.probability();
    (chance > 0.0).then(|| Estimate::from_chance(chance, rate))
}

/// Measures this machine on the engine that will do the searching.
///
/// Deliberately the same `PairEngine` a run uses, rather than a synthetic loop:
/// an estimate built on a different code path would be an estimate of something
/// nobody is going to run.
pub fn measure_rate(threads: usize, batch_size: usize, at_least: Duration) -> f64 {
    let threads = threads.max(1);
    let counted: Vec<u64> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|t| {
                scope.spawn(move || {
                    let mut engine = PairEngine::new(batch_size);
                    let per_round = engine.batch_size() as u64;
                    let seed = [t as u8; 32];
                    let scalar = key::secret_scalar(&seed);
                    let mut acc =
                        curve::scalar_base_mult(&scalar, &Point::basepoint(), &curve::two_d());
                    engine.begin(&mut acc);

                    // One untimed round first: the first touches every table
                    // entry and would otherwise be charged to the rate.
                    engine.run(&mut acc, None);

                    let start = Instant::now();
                    let mut done = 0u64;
                    while start.elapsed() < at_least {
                        for _ in 0..8 {
                            engine.run(&mut acc, None);
                            done += per_round;
                        }
                    }
                    (done as f64 / start.elapsed().as_secs_f64()) as u64
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap_or(0)).collect()
    });
    counted.iter().sum::<u64>() as f64
}

/// Batch sizes the automatic choice tries, smallest first.
///
/// A batch is how many candidates share one modular inversion, so a larger one
/// spreads that cost further — until the two planes of field elements stop
/// fitting in cache and every candidate starts paying for a miss instead. Where
/// that turns over is a property of the machine, not of the program, which is
/// why it is measured rather than chosen here.
pub const BATCH_CHOICES: &[usize] = &[4096, 8192, 16384, 32768, 65536];

/// Picks the batch size by running the engine at each of them.
///
/// Returns the fastest, and the rate it reached. The window is short because
/// the differences are large: on an M1 Pro the best was measured at more than
/// twice the worst, which no amount of sampling noise hides.
pub fn tune_batch(threads: usize, each: Duration) -> (usize, f64) {
    let mut best = (crate::batch::DEFAULT_BATCH_SIZE, 0.0);
    for &size in BATCH_CHOICES {
        let rate = measure_rate(threads, size, each);
        if rate > best.1 {
            best = (size, rate);
        }
    }
    best
}

/// A duration a person can read, from seconds.
///
/// Rounded hard on purpose: an estimate good to three significant figures
/// would be claiming a precision the exponential distribution does not have.
pub fn human_time(seconds: f64) -> String {
    if !seconds.is_finite() {
        return "never".to_string();
    }
    const MINUTE: f64 = 60.0;
    const HOUR: f64 = 60.0 * MINUTE;
    const DAY: f64 = 24.0 * HOUR;
    const YEAR: f64 = 365.25 * DAY;
    match seconds {
        s if s < 1.0 => "under a second".to_string(),
        s if s < 1.5 => "1 second".to_string(),
        s if s < MINUTE => format!("{s:.0} seconds"),
        s if s < HOUR => format!("{:.0} minutes", s / MINUTE),
        s if s < DAY => format!("{:.1} hours", s / HOUR),
        s if s < YEAR => format!("{:.1} days", s / DAY),
        s if s < YEAR * 1000.0 => format!("{:.1} years", s / YEAR),
        s => {
            let years = s / YEAR;
            // Past a thousand years the only useful information is the scale.
            format!("{:.0e} years", years)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_longer_prefix_costs_thirty_two_times_more() {
        let four = Estimate::from_chance(chance_of_prefix(4), 1e6);
        let five = Estimate::from_chance(chance_of_prefix(5), 1e6);
        let ratio = five.median_seconds / four.median_seconds;
        assert!((ratio - 32.0).abs() < 1e-6, "ratio was {ratio}");
    }

    /// Half of all searches take longer than the median, which is why the
    /// survey quotes a second number.
    #[test]
    fn the_ninetieth_percentile_is_the_longer_wait() {
        let e = Estimate::from_chance(chance_of_prefix(6), 1e8);
        let ratio = e.ninety_seconds / e.median_seconds;
        assert!(
            (ratio - 10f64.ln() / std::f64::consts::LN_2).abs() < 1e-9,
            "ratio was {ratio}"
        );
        assert!(e.ninety_seconds > e.median_seconds);
    }

    #[test]
    fn a_faster_machine_waits_proportionally_less() {
        let slow = Estimate::from_chance(chance_of_prefix(5), 1e6);
        let fast = Estimate::from_chance(chance_of_prefix(5), 2e6);
        assert!((slow.median_seconds / fast.median_seconds - 2.0).abs() < 1e-9);
        assert_eq!(slow.candidates, fast.candidates, "the work is the same");
    }

    #[test]
    fn an_impossible_search_is_said_to_be_impossible() {
        let e = Estimate::from_chance(0.0, 1e9);
        assert!(!e.median_seconds.is_finite());
        assert_eq!(human_time(e.median_seconds), "never");
    }

    #[test]
    fn durations_read_as_a_person_would_say_them() {
        assert_eq!(human_time(0.4), "under a second");
        assert_eq!(human_time(1.2), "1 second");
        assert_eq!(human_time(30.0), "30 seconds");
        assert_eq!(human_time(600.0), "10 minutes");
        assert_eq!(human_time(7200.0), "2.0 hours");
        assert_eq!(human_time(86400.0 * 3.0), "3.0 days");
        assert_eq!(human_time(86400.0 * 400.0), "1.1 years");
        assert!(human_time(1e15).ends_with("years"));
    }

    /// The survey has to cover what the program will actually be asked for.
    #[test]
    fn the_survey_spans_four_to_twelve_symbols() {
        let rows = by_length(1e8);
        assert_eq!(rows.len(), LONGEST - SHORTEST + 1);
        assert_eq!(rows[0].0, 4);
        assert_eq!(rows[rows.len() - 1].0, 12);
        for pair in rows.windows(2) {
            assert!(
                pair[1].1.median_seconds > pair[0].1.median_seconds,
                "each symbol must cost more than the one before"
            );
        }
    }

    /// The tuner has to come back with something from the list, and with a
    /// rate that matches what that size actually does.
    #[test]
    fn tuning_picks_a_size_it_measured() {
        let (size, rate) = tune_batch(2, Duration::from_millis(60));
        assert!(BATCH_CHOICES.contains(&size), "picked {size}");
        assert!(rate > 1e5, "rate was {rate}");
    }

    #[test]
    fn the_measured_rate_is_a_real_rate() {
        let rate = measure_rate(2, 2048, Duration::from_millis(120));
        assert!(
            rate > 1e5,
            "a modern machine does far more than this: {rate}"
        );
    }
}
