//! Who is searching which part of the space, and what comes back when they
//! stop.
//!
//! Leases rather than assignments: a pod can be evicted at any moment, and a
//! range assigned forever would sit unsearched in silence. The cost is that
//! whatever a departing worker covered without reporting gets covered again.
//!
//! Leases are sized by time, not by blocks. The spread between the fastest card
//! and a processor is 14.3x, so any fixed block count is wrong for one of them:
//! too small and the card spends its time asking, too large and an eviction
//! throws away half a range on average.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

/// A half-open range of block indices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    pub first: u64,
    pub blocks: u64,
}

impl Range {
    pub fn contains(&self, block: u64) -> bool {
        block >= self.first && block < self.first + self.blocks
    }
}

/// A range currently held by a worker.
#[derive(Debug, Clone)]
struct Held {
    range: Range,
    worker: u64,
    expires: Instant,
}

/// How big a lease is when the worker has not yet measured itself.
///
/// Deliberately small: the first lease exists to produce a measurement, and
/// being wrong about it briefly costs one extra round trip. Being wrong about
/// it generously costs a slow worker holding a range it cannot finish.
const FIRST_LEASE_BLOCKS: u64 = 256;

/// Bounds on a lease, whatever the arithmetic says.
///
/// The floor keeps a fast card from spending its time asking rather than
/// searching. The ceiling bounds what an eviction throws away.
const MIN_LEASE_BLOCKS: u64 = 64;
const MAX_LEASE_BLOCKS: u64 = 1 << 24;

pub struct Queue {
    /// The lowest block never yet handed to anybody.
    frontier: u64,
    /// Ranges that came back unfinished. Handed out again before any new
    /// ground, so that a gap left by a departing worker closes quickly rather
    /// than waiting behind everything else.
    returned: VecDeque<Range>,
    held: HashMap<u64, Held>,
    next_lease: u64,
    lease_for: Duration,
    work_seconds: f64,
}

impl Queue {
    pub fn new(first_block: u64, lease_for: Duration, work_seconds: u64) -> Self {
        Queue {
            frontier: first_block,
            returned: VecDeque::new(),
            held: HashMap::new(),
            next_lease: 1,
            lease_for,
            work_seconds: work_seconds as f64,
        }
    }

    /// How many blocks a worker of this rate should take.
    pub fn size_for(&self, blocks_per_second: Option<f64>) -> u64 {
        let Some(rate) = blocks_per_second.filter(|r| r.is_finite() && *r > 0.0) else {
            return FIRST_LEASE_BLOCKS;
        };
        let wanted = rate * self.work_seconds;
        // `as u64` saturates at the top and floors, which is what is wanted
        // either side; the clamp then puts it inside the bounds.
        (wanted as u64).clamp(MIN_LEASE_BLOCKS, MAX_LEASE_BLOCKS)
    }

    /// Hands out a range, taking one that came back before breaking new ground.
    pub fn lease(
        &mut self,
        worker: u64,
        blocks_per_second: Option<f64>,
        now: Instant,
    ) -> (u64, Range) {
        self.reclaim(now);
        let wanted = self.size_for(blocks_per_second);
        let range = match self.returned.pop_front() {
            Some(back) if back.blocks <= wanted => back,
            // A returned range bigger than this worker wants is split, and the
            // remainder goes back at the front: handing over the whole thing
            // would give a slow worker a range it cannot finish, and the range
            // would then expire and be split anyway, a lease later.
            Some(back) => {
                self.returned.push_front(Range {
                    first: back.first + wanted,
                    blocks: back.blocks - wanted,
                });
                Range {
                    first: back.first,
                    blocks: wanted,
                }
            }
            None => {
                let range = Range {
                    first: self.frontier,
                    blocks: wanted,
                };
                self.frontier += wanted;
                range
            }
        };
        let lease = self.next_lease;
        self.next_lease += 1;
        self.held.insert(
            lease,
            Held {
                range,
                worker,
                expires: now + self.lease_for,
            },
        );
        (lease, range)
    }

    /// Pushes a lease's expiry back. `false` when the range has already gone to
    /// somebody else, which tells the worker to stop and ask again.
    pub fn renew(&mut self, lease: u64, now: Instant) -> bool {
        self.reclaim(now);
        match self.held.get_mut(&lease) {
            Some(held) => {
                held.expires = now + self.lease_for;
                true
            }
            None => false,
        }
    }

    /// Gives a range back before its time, when a worker has finished it.
    pub fn done(&mut self, lease: u64) {
        self.held.remove(&lease);
    }

    /// The range a lease covers, if it is still held.
    pub fn range_of(&self, lease: u64) -> Option<Range> {
        self.held.get(&lease).map(|h| h.range)
    }

    /// Whoever holds this lease.
    pub fn holder(&self, lease: u64) -> Option<u64> {
        self.held.get(&lease).map(|h| h.worker)
    }

    /// Moves everything overdue back into the queue.
    pub fn reclaim(&mut self, now: Instant) -> usize {
        let gone: Vec<u64> = self
            .held
            .iter()
            .filter(|(_, h)| h.expires <= now)
            .map(|(id, _)| *id)
            .collect();
        for id in &gone {
            if let Some(held) = self.held.remove(id) {
                self.returned.push_back(held.range);
            }
        }
        gone.len()
    }

    pub fn out(&self) -> usize {
        self.held.len()
    }

    pub fn waiting(&self) -> usize {
        self.returned.len()
    }

    pub fn frontier(&self) -> u64 {
        self.frontier
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queue() -> Queue {
        Queue::new(0, Duration::from_secs(60), 60)
    }

    #[test]
    fn two_workers_get_different_ground() {
        let mut q = queue();
        let now = Instant::now();
        let (_, a) = q.lease(1, Some(10.0), now);
        let (_, b) = q.lease(2, Some(10.0), now);
        assert!(
            a.first + a.blocks <= b.first || b.first + b.blocks <= a.first,
            "ranges overlap: {a:?} and {b:?}"
        );
    }

    #[test]
    fn a_faster_worker_gets_more() {
        let q = queue();
        let slow = q.size_for(Some(511.0));
        let fast = q.size_for(Some(7327.0));
        assert!(fast > slow, "{fast} should exceed {slow}");
        // Sized by time: both should take about the work period.
        assert_eq!(slow, 511 * 60);
        assert_eq!(fast, 7327 * 60);
    }

    #[test]
    fn the_first_lease_is_small_and_the_second_is_measured() {
        let mut q = queue();
        let now = Instant::now();
        let (_, first) = q.lease(1, None, now);
        assert_eq!(first.blocks, FIRST_LEASE_BLOCKS);
        let (_, second) = q.lease(1, Some(1000.0), now);
        assert_eq!(second.blocks, 60_000, "the measurement should decide it");
    }

    #[test]
    fn a_size_is_bounded_both_ways() {
        let q = queue();
        assert_eq!(q.size_for(Some(0.001)), MIN_LEASE_BLOCKS);
        assert_eq!(q.size_for(Some(1e18)), MAX_LEASE_BLOCKS);
        // Nonsense from a worker must not produce a nonsense lease.
        assert_eq!(q.size_for(Some(f64::NAN)), FIRST_LEASE_BLOCKS);
        assert_eq!(q.size_for(Some(-5.0)), FIRST_LEASE_BLOCKS);
        assert_eq!(q.size_for(Some(f64::INFINITY)), FIRST_LEASE_BLOCKS);
    }

    #[test]
    fn a_silent_worker_gives_its_range_back() {
        let mut q = queue();
        let start = Instant::now();
        let (lease, taken) = q.lease(1, Some(10.0), start);
        assert_eq!(q.out(), 1);

        let later = start + Duration::from_secs(61);
        assert_eq!(q.reclaim(later), 1, "the lease should have expired");
        assert_eq!(q.out(), 0);
        assert_eq!(q.waiting(), 1);

        // And the next worker gets exactly that ground, not fresh ground.
        let (_, again) = q.lease(2, Some(10.0), later);
        assert_eq!(again, taken, "the returned range should go out first");
        assert!(!q.renew(lease, later), "the old lease is no longer held");
    }

    #[test]
    fn a_worker_that_keeps_talking_keeps_its_range() {
        let mut q = queue();
        let start = Instant::now();
        let (lease, mine) = q.lease(1, Some(10.0), start);
        for step in 1..=5 {
            let now = start + Duration::from_secs(30 * step);
            assert!(q.renew(lease, now), "renewing in time must hold the range");
            let (_, other) = q.lease(2, Some(10.0), now);
            assert_ne!(other, mine, "a renewed range must not be handed out again");
            q.done(other.first);
        }
    }

    #[test]
    fn a_returned_range_too_big_for_the_next_worker_is_split() {
        let mut q = queue();
        let start = Instant::now();
        let (_, big) = q.lease(1, Some(10_000.0), start);
        let later = start + Duration::from_secs(61);
        q.reclaim(later);

        let (_, small) = q.lease(2, Some(100.0), later);
        assert_eq!(small.first, big.first);
        assert_eq!(small.blocks, 6_000);
        assert_eq!(q.waiting(), 1, "the remainder must still be waiting");

        // And the remainder is the rest of it, with nothing lost or repeated.
        let (_, rest) = q.lease(3, Some(10_000.0), later);
        assert_eq!(rest.first, big.first + small.blocks);
        assert_eq!(rest.blocks, big.blocks - small.blocks);
    }

    #[test]
    fn a_lease_knows_what_it_covers() {
        let mut q = queue();
        let now = Instant::now();
        let (lease, range) = q.lease(1, Some(10.0), now);
        assert_eq!(q.range_of(lease), Some(range));
        assert_eq!(q.holder(lease), Some(1));
        assert!(range.contains(range.first));
        assert!(range.contains(range.first + range.blocks - 1));
        assert!(!range.contains(range.first + range.blocks));
        q.done(lease);
        assert_eq!(q.range_of(lease), None);
    }
}
