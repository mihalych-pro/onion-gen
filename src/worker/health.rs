//! What the worker says about itself, and what its own metrics are built from.
//!
//! The two probes answer different questions.
//!
//! **Liveness asks only whether the search is moving**, never whether the
//! master is reachable. A worker that has lost its master still finishes the
//! range it holds and keeps its finds; restarting it throws both away, and
//! every worker loses the master at the same moment.
//!
//! **Readiness means "has work in hand".** A pod with nowhere to get work is
//! not a worker and a rollout should stop on it. A worker whose master has gone
//! but whose range is unfinished is ready.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// How long the search may make no progress before the worker is wedged.
///
/// Generous on purpose: a lease boundary, a slow first block on a cold device
/// and a long `SHA3` pass over a checksum-reaching filter are all ordinary
/// pauses, and a probe that fires on those restarts a healthy worker.
pub const STILL_FOR: Duration = Duration::from_secs(120);

/// Shared between the search, the loop and the probe endpoint.
#[derive(Debug)]
pub struct Health {
    started: Instant,
    /// Blocks finished since the process started, across every lease.
    blocks: AtomicU64,
    /// Milliseconds since `started` when `blocks` last changed.
    moved_at: AtomicU64,
    holds_lease: AtomicBool,
    registered: AtomicBool,
    buffered: AtomicU64,
    /// The last measured rate, as `f64` bits.
    rate: AtomicU64,
}

impl Default for Health {
    fn default() -> Self {
        Health {
            started: Instant::now(),
            blocks: AtomicU64::new(0),
            moved_at: AtomicU64::new(0),
            holds_lease: AtomicBool::new(false),
            registered: AtomicBool::new(false),
            buffered: AtomicU64::new(0),
            rate: AtomicU64::new(0),
        }
    }
}

impl Health {
    /// Records that the search has got somewhere.
    pub fn advanced(&self, blocks_total: u64) {
        self.blocks.store(blocks_total, Ordering::Relaxed);
        self.moved_at
            .store(self.started.elapsed().as_millis() as u64, Ordering::Relaxed);
    }

    pub fn registered(&self, yes: bool) {
        self.registered.store(yes, Ordering::Relaxed);
    }

    pub fn holding(&self, yes: bool) {
        self.holds_lease.store(yes, Ordering::Relaxed);
        if yes {
            // Taking a lease is progress in itself: the first block of a fresh
            // range can take a while, and the clock should not already be
            // running against it.
            self.moved_at
                .store(self.started.elapsed().as_millis() as u64, Ordering::Relaxed);
        }
    }

    pub fn buffered(&self, count: u64) {
        self.buffered.store(count, Ordering::Relaxed);
    }

    pub fn rate(&self, blocks_per_second: f64) {
        self.rate
            .store(blocks_per_second.to_bits(), Ordering::Relaxed);
    }

    /// Is the search moving? Nothing about the master enters this.
    pub fn alive(&self) -> bool {
        let since = self
            .started
            .elapsed()
            .saturating_sub(Duration::from_millis(self.moved_at.load(Ordering::Relaxed)));
        since < STILL_FOR
    }

    /// Does this worker have work in hand?
    ///
    /// Not "can it reach the master": one that holds a range and is searching
    /// it is working, whatever the master is doing. Readiness drops when the
    /// range runs out and no new one can be had, which is the moment a pod
    /// stops being a worker.
    pub fn ready(&self) -> bool {
        self.registered.load(Ordering::Relaxed) && self.holds_lease.load(Ordering::Relaxed)
    }

    pub fn blocks(&self) -> u64 {
        self.blocks.load(Ordering::Relaxed)
    }

    pub fn buffered_count(&self) -> u64 {
        self.buffered.load(Ordering::Relaxed)
    }

    pub fn rate_now(&self) -> f64 {
        f64::from_bits(self.rate.load(Ordering::Relaxed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_worker_is_alive_but_not_ready() {
        let h = Health::default();
        assert!(h.alive(), "nothing has had time to go wrong yet");
        assert!(!h.ready(), "it has not registered or taken work");
    }

    #[test]
    fn readiness_needs_both_a_registration_and_a_lease() {
        let h = Health::default();
        h.registered(true);
        assert!(!h.ready(), "registered but holding nothing");
        h.holding(true);
        assert!(h.ready());
        h.holding(false);
        assert!(!h.ready(), "between leases it is not ready");
    }

    /// The rule the module exists for: losing the master must not make a
    /// worker look dead. Observed on a live run — a worker kept searching
    /// through a master restart and buffered 464 finds, every one of which
    /// reached the store afterwards.
    #[test]
    fn losing_the_master_does_not_make_the_worker_dead() {
        let h = Health::default();
        h.registered(true);
        h.holding(true);
        h.advanced(10);

        // The master goes away: no lease can be renewed or taken.
        h.holding(false);
        assert!(!h.ready(), "it cannot be given work");
        assert!(
            h.alive(),
            "it is finishing the range it has; restarting it would throw that \
             away, and every worker at once"
        );
    }

    #[test]
    fn progress_is_what_liveness_watches() {
        let h = Health::default();
        h.advanced(1);
        assert!(h.alive());
        assert_eq!(h.blocks(), 1);
        h.advanced(5);
        assert_eq!(h.blocks(), 5);
    }

    #[test]
    fn what_the_worker_knows_about_itself_is_readable() {
        let h = Health::default();
        h.rate(439.7);
        h.buffered(3);
        assert!((h.rate_now() - 439.7).abs() < 1e-9);
        assert_eq!(h.buffered_count(), 3);
    }
}
