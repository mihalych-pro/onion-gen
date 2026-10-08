//! What master and worker say to each other.
//!
//! The worker reports *where* it found something rather than *what*: the master
//! handed out the seed, so it derives the key itself with the same
//! `expanded_secret_at_offset`. Two numbers cross the wire instead of
//! sixty-four bytes, and the master checks the find rather than believing it.
//!
//! Work is counted in blocks per second, not candidates per second. A block is
//! 64 batches from its own seed; how many candidates that is depends on the
//! worker's batch size, so the master would otherwise have to know how each
//! worker is configured to size its lease.

use serde::{Deserialize, Serialize};

/// A worker announcing itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Register {
    /// What to call it in the fleet view. An operator recognises a host name.
    pub name: String,
}

/// What a worker gets for announcing itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Registered {
    pub worker: u64,
    /// The set to search for, as written. Delivered from here because a
    /// dictionary of ten thousand words fits in no command line, and copying
    /// it to every node by hand is the work this mode exists to remove.
    pub filters: Vec<String>,
}

/// A worker asking for something to do.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AskWork {
    pub worker: u64,
    /// The lease just finished, so the master can drop it rather than wait for
    /// it to expire. Without this a completed range goes back into the queue
    /// and is searched a second time by somebody else.
    #[serde(default)]
    pub finished: Option<u64>,
    /// Blocks per second, as measured by the worker on its own last lease.
    /// `None` the first time, when there is nothing to measure yet.
    pub blocks_per_second: Option<f64>,
}

/// A range of the search space, held for a while.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lease {
    pub lease: u64,
    /// The root seed, hex. Everything the worker searches comes from it.
    pub seed: String,
    pub first_block: u64,
    pub blocks: u64,
    /// Seconds until the master gives this range to somebody else.
    pub expires_in: u64,
}

/// A worker saying it is still there.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Heartbeat {
    pub lease: u64,
    pub blocks_done: u64,
    pub candidates: u64,
}

/// Whether the worker should carry on with this lease.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Renewed {
    /// False when the master has already given the range to somebody else,
    /// which tells the worker to stop and ask for fresh work.
    pub held: bool,
    pub expires_in: u64,
}

/// Where a find is, which is all the master needs to reconstruct it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Found {
    pub lease: u64,
    pub block: u64,
    /// The offset within the block's chain.
    pub offset: u64,
}

/// What the master made of a reported find.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Accepted {
    pub accepted: bool,
    /// The address the master derived, so the worker can log what it handed
    /// over, and a rejection can be understood without reading the master.
    pub address: Option<String>,
    pub why: Option<String>,
}

/// One worker, as the fleet view shows it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerView {
    pub worker: u64,
    pub name: String,
    pub blocks_per_second: f64,
    pub candidates: u64,
    pub finds: u64,
    /// Seconds since it last said anything. An operator needs this to tell a
    /// slow worker from an absent one: the two look alike in a total.
    pub silent_for: u64,
    pub present: bool,
}

/// The whole fleet at a glance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetView {
    pub workers: Vec<WorkerView>,
    pub blocks_per_second: f64,
    pub candidates: u64,
    pub finds: u64,
    pub rejected: u64,
    /// Ranges handed out and not yet finished or expired.
    pub leases_out: usize,
    /// Ranges that came back unfinished and are waiting to be given again.
    pub leases_returned: usize,
    /// What the store is doing. The one part of a fleet that is somebody
    /// else's service, so "is it still there" is a question worth answering
    /// from the metrics rather than from a log.
    pub store: StoreView,
}

/// The store, as the metrics report it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreView {
    /// Which kind it is, for the label: `directory`, `sqlite`, `postgres`,
    /// `mysql`.
    #[serde(skip)]
    pub kind: &'static str,
    pub connections: u32,
    pub idle: u32,
    pub rows: u64,
    pub bytes: u64,
    pub batches: u64,
    pub failures: u64,
}

/// A name that is safe to put in a metric label.
///
/// A worker names itself, so its name arrives here unfiltered. The Prometheus
/// client does not escape label values — checked against the library, not
/// assumed — so a quote would end the label early and hand the scraper a line
/// it cannot parse, taking the rest of the exposition down with it. Stopping it
/// at the boundary is simpler than escaping at every use, and leaves one place
/// to be right.
pub fn safe_label(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '"' | '\\' | '\n' | '\r' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .take(128)
        .collect();
    if cleaned.trim().is_empty() {
        "unnamed".to_string()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_that_could_break_a_label_is_made_safe() {
        assert_eq!(safe_label("node-1"), "node-1");
        assert_eq!(safe_label("ev\"il"), "ev_il");
        assert_eq!(safe_label("back\\slash"), "back_slash");
        assert_eq!(safe_label("two\nlines"), "two_lines");
    }

    #[test]
    fn an_empty_or_blank_name_still_names_something() {
        assert_eq!(safe_label(""), "unnamed");
        assert_eq!(safe_label("   "), "unnamed");
    }

    #[test]
    fn a_very_long_name_is_cut_rather_than_passed_on() {
        let long = "a".repeat(500);
        assert_eq!(safe_label(&long).len(), 128);
    }
}
