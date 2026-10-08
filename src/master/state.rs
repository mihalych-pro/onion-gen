//! The master's state machine, with no sockets in it.
//!
//! Kept apart from the HTTP layer on purpose: what goes wrong here — a range
//! handed out twice, a find credited to the wrong worker, a silent worker
//! counted as present — is invisible from outside and awkward to provoke
//! through a socket. Every rule below is exercised directly.

use std::collections::HashMap;
use std::io;

use std::time::{Duration, Instant};

use crate::filter::FilterSet;
use crate::master::{examine, Queue, Verdict};
use crate::proto::*;
use crate::score;
use crate::store::{Find, Store};

/// One worker, as the master remembers it.
#[derive(Debug, Clone)]
struct Known {
    name: String,
    blocks_per_second: f64,
    candidates: u64,
    finds: u64,
    last_seen: Instant,
}

pub struct Master {
    root: [u8; 32],
    queue: Queue,
    filters: FilterSet,
    /// The filter set as written, because that is what a worker needs: a
    /// parsed filter cannot be turned back into its text without losing the
    /// form it was written in.
    filter_text: Vec<String>,
    store: Store,
    workers: HashMap<u64, Known>,
    next_worker: u64,
    lease_for: Duration,
    finds: u64,
    rejected: u64,
}

impl Master {
    pub fn new(
        root: [u8; 32],
        first_block: u64,
        filters: FilterSet,
        filter_text: Vec<String>,
        store: Store,
        lease_for: Duration,
        work_seconds: u64,
    ) -> Self {
        Master {
            root,
            queue: Queue::new(first_block, lease_for, work_seconds),
            filters,
            filter_text,
            store,
            workers: HashMap::new(),
            next_worker: 1,
            lease_for,
            finds: 0,
            rejected: 0,
        }
    }

    pub fn register(&mut self, ask: &Register, now: Instant) -> Registered {
        let worker = self.next_worker;
        self.next_worker += 1;
        self.workers.insert(
            worker,
            Known {
                name: ask.name.clone(),
                blocks_per_second: 0.0,
                candidates: 0,
                finds: 0,
                last_seen: now,
            },
        );
        Registered {
            worker,
            filters: self.filter_text.clone(),
        }
    }

    pub fn work(&mut self, ask: &AskWork, now: Instant) -> Lease {
        if let Some(known) = self.workers.get_mut(&ask.worker) {
            known.last_seen = now;
            if let Some(rate) = ask.blocks_per_second.filter(|r| r.is_finite() && *r > 0.0) {
                known.blocks_per_second = rate;
            }
        }
        if let Some(finished) = ask.finished {
            // Released rather than left to expire: an expired lease goes back
            // into the queue, and a range that was searched to the end would
            // then be searched again by the next worker along.
            self.queue.done(finished);
        }
        let (lease, range) = self.queue.lease(ask.worker, ask.blocks_per_second, now);
        Lease {
            lease,
            seed: hex(&self.root),
            first_block: range.first,
            blocks: range.blocks,
            expires_in: self.lease_for.as_secs(),
        }
    }

    pub fn heartbeat(&mut self, beat: &Heartbeat, now: Instant) -> Renewed {
        if let Some(worker) = self.queue.holder(beat.lease) {
            if let Some(known) = self.workers.get_mut(&worker) {
                known.last_seen = now;
                known.candidates = known.candidates.max(beat.candidates);
            }
        }
        let held = self.queue.renew(beat.lease, now);
        Renewed {
            held,
            expires_in: self.lease_for.as_secs(),
        }
    }

    /// Takes a reported position, works out what it is, and keeps it if it is
    /// what was asked for.
    pub fn found(&mut self, report: &Found, now: Instant) -> io::Result<Accepted> {
        // A lease the master no longer knows about is not a reason to refuse.
        // It means the lease expired, or this master was restarted and lost
        // what it had handed out — and in both cases the worker is handing
        // over exactly what it was told to keep. The find is still checked,
        // by deriving it and matching the address.
        let range = self.queue.range_of(report.lease);
        if let Some(worker) = self.queue.holder(report.lease) {
            if let Some(known) = self.workers.get_mut(&worker) {
                known.last_seen = now;
            }
        }
        match examine(
            &self.root,
            range,
            report.block,
            report.offset,
            &self.filters,
        ) {
            Verdict::Good {
                address,
                public_key,
                secret,
                filter,
            } => {
                // A range can be handed out twice after an expiry, so the same
                // address can arrive twice. The second one is not an error and
                // must not displace the first, nor be counted again.
                let hostname = crate::key::hostname(&public_key);
                if !self.store.holds(&hostname)? {
                    let score = self
                        .filters
                        .match_address_text(&address)
                        .map_or(0.0, |f| score::of_find(&public_key, f).total);
                    self.store.put(Find {
                        address: hostname,
                        public_key,
                        secret: *secret,
                        filter,
                        score,
                        block: report.block,
                        offset: report.offset,
                        source: "a worker".to_string(),
                    })?;
                    self.finds += 1;
                    if let Some(worker) = self.queue.holder(report.lease) {
                        if let Some(known) = self.workers.get_mut(&worker) {
                            known.finds += 1;
                        }
                    }
                }
                Ok(Accepted {
                    accepted: true,
                    address: Some(address),
                    why: None,
                })
            }
            Verdict::OutsideLease => {
                self.rejected += 1;
                Ok(refused("that position is outside the lease"))
            }
            Verdict::NoMatch { address } => {
                self.rejected += 1;
                Ok(Accepted {
                    accepted: false,
                    address: Some(address),
                    why: Some("that address satisfies no filter".into()),
                })
            }
            Verdict::NotAKey => {
                self.rejected += 1;
                Ok(refused("that position yields no usable key"))
            }
        }
    }

    /// What the fleet looks like now.
    pub fn view(&mut self, now: Instant) -> FleetView {
        self.queue.reclaim(now);
        let mut workers: Vec<WorkerView> = self
            .workers
            .iter()
            .map(|(id, k)| {
                let silent = now.saturating_duration_since(k.last_seen);
                WorkerView {
                    worker: *id,
                    name: k.name.clone(),
                    blocks_per_second: k.blocks_per_second,
                    candidates: k.candidates,
                    finds: k.finds,
                    silent_for: silent.as_secs(),
                    // A worker quiet for longer than a lease is not slow, it is
                    // gone; the two look alike in a total and call for opposite
                    // responses.
                    present: silent <= self.lease_for,
                }
            })
            .collect();
        workers.sort_by_key(|w| w.worker);
        FleetView {
            blocks_per_second: workers
                .iter()
                .filter(|w| w.present)
                .map(|w| w.blocks_per_second)
                .sum(),
            candidates: workers.iter().map(|w| w.candidates).sum(),
            finds: self.finds,
            rejected: self.rejected,
            leases_out: self.queue.out(),
            leases_returned: self.queue.waiting(),
            workers,
            store: {
                let s = self.store.stats();
                crate::proto::StoreView {
                    kind: self.store.kind(),
                    connections: s.connections,
                    idle: s.idle,
                    rows: s.rows,
                    bytes: s.bytes,
                    batches: s.batches,
                    failures: s.failures,
                }
            },
        }
    }

    /// Writes down whatever the store is holding, if it has waited long
    /// enough. Called on a timer by the server, because a fleet that goes
    /// quiet would otherwise leave its last finds in memory.
    pub fn flush_if_due(&mut self) -> io::Result<()> {
        if self.store.overdue() {
            self.store.flush()?;
        }
        Ok(())
    }

    /// Writes down everything held, whether it has waited or not.
    pub fn flush(&mut self) -> io::Result<()> {
        self.store.flush()
    }
}

fn refused(why: &str) -> Accepted {
    Accepted {
        accepted: false,
        address: None,
        why: Some(why.into()),
    }
}

fn hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::batch::{BatchEngine, CHAIN_STEP};
    use crate::curve::{self, Point};
    use crate::key;
    use crate::run::block_seed;
    use std::path::{Path, PathBuf};

    fn master(dir: &Path) -> Master {
        Master::new(
            [0x2fu8; 32],
            0,
            FilterSet::parse_all(["ab"]).expect("a valid filter"),
            vec!["ab".to_string()],
            Store::directory(dir.to_path_buf()),
            Duration::from_secs(60),
            60,
        )
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "onion-gen-master-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    /// Searches a block the way a worker does and returns where it found one.
    fn find_in_block(root: &[u8; 32], block: u64, prefix: &str) -> (u64, String) {
        let seed = block_seed(root, block);
        let scalar = key::secret_scalar(&seed);
        let mut acc = curve::scalar_base_mult(&scalar, &Point::basepoint(), &curve::two_d());
        let mut engine = BatchEngine::new(1024);
        let mut base = 0u64;
        for _ in 0..512 {
            engine.run(&mut acc);
            for i in 0..engine.batch_size() {
                let packed = engine.packed_with_sign(i);
                let address = key::address(&packed);
                if address.starts_with(prefix) {
                    return (base + CHAIN_STEP * i as u64, address);
                }
            }
            base += CHAIN_STEP * engine.batch_size() as u64;
        }
        panic!("no hit for {prefix} in block {block}");
    }

    #[test]
    fn a_worker_gets_the_filter_set_it_never_had() {
        let dir = temp("filters");
        let mut m = master(&dir);
        let got = m.register(
            &Register {
                name: "node-1".into(),
            },
            Instant::now(),
        );
        assert_eq!(got.filters, vec!["ab".to_string()]);
        assert_eq!(
            got.worker, 1,
            "workers are named by the master, not themselves"
        );
    }

    #[test]
    fn a_find_travels_as_two_numbers_and_lands_as_a_key() {
        let dir = temp("find");
        let mut m = master(&dir);
        let now = Instant::now();
        let w = m
            .register(
                &Register {
                    name: "node-1".into(),
                },
                now,
            )
            .worker;
        let lease = m.work(
            &AskWork {
                worker: w,
                blocks_per_second: Some(10.0),
                finished: None,
            },
            now,
        );
        let block = lease.first_block;
        let (offset, expected) = find_in_block(&[0x2fu8; 32], block, "ab");

        let got = m
            .found(
                &Found {
                    lease: lease.lease,
                    block,
                    offset,
                },
                now,
            )
            .expect("the write succeeds");
        assert!(got.accepted, "{got:?}");
        assert_eq!(got.address.as_deref(), Some(expected.as_str()));

        // And it is on disk in the layout tor takes.
        m.flush().expect("the write succeeds");
        let written = dir.join(format!("{expected}.onion"));
        assert!(
            written.join("hs_ed25519_secret_key").exists(),
            "{written:?}"
        );
        assert!(written.join("hostname").exists());
        assert_eq!(m.view(now).finds, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_same_find_arriving_twice_is_not_counted_twice() {
        let dir = temp("twice");
        let mut m = master(&dir);
        let now = Instant::now();
        let w = m.register(&Register { name: "n".into() }, now).worker;
        let lease = m.work(
            &AskWork {
                worker: w,
                blocks_per_second: Some(10.0),
                finished: None,
            },
            now,
        );
        let (offset, _) = find_in_block(&[0x2fu8; 32], lease.first_block, "ab");
        let report = Found {
            lease: lease.lease,
            block: lease.first_block,
            offset,
        };

        assert!(m.found(&report, now).expect("write").accepted);
        assert!(m.found(&report, now).expect("write").accepted);
        assert_eq!(
            m.view(now).finds,
            1,
            "a range re-issued after an expiry can produce the same address twice"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_position_outside_the_lease_is_refused_and_counted() {
        let dir = temp("outside");
        let mut m = master(&dir);
        let now = Instant::now();
        let w = m.register(&Register { name: "n".into() }, now).worker;
        let lease = m.work(
            &AskWork {
                worker: w,
                blocks_per_second: Some(10.0),
                finished: None,
            },
            now,
        );
        let far = lease.first_block + lease.blocks + 1;

        let got = m
            .found(
                &Found {
                    lease: lease.lease,
                    block: far,
                    offset: 0,
                },
                now,
            )
            .expect("no write");
        assert!(!got.accepted);
        assert_eq!(m.view(now).rejected, 1);
        assert_eq!(m.view(now).finds, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_silent_worker_is_shown_absent_rather_than_slow() {
        let dir = temp("silent");
        let mut m = master(&dir);
        let start = Instant::now();
        let a = m
            .register(
                &Register {
                    name: "chatty".into(),
                },
                start,
            )
            .worker;
        let b = m
            .register(
                &Register {
                    name: "gone".into(),
                },
                start,
            )
            .worker;
        m.work(
            &AskWork {
                worker: a,
                blocks_per_second: Some(10.0),
                finished: None,
            },
            start,
        );
        m.work(
            &AskWork {
                worker: b,
                blocks_per_second: Some(90.0),
                finished: None,
            },
            start,
        );

        let later = start + Duration::from_secs(90);
        m.work(
            &AskWork {
                worker: a,
                blocks_per_second: Some(10.0),
                finished: None,
            },
            later,
        );

        let view = m.view(later);
        let chatty = view.workers.iter().find(|w| w.name == "chatty").expect("a");
        let gone = view.workers.iter().find(|w| w.name == "gone").expect("b");
        assert!(chatty.present);
        assert!(!gone.present, "silent for 90 s with a 60 s lease");
        assert!(gone.silent_for >= 90);
        assert_eq!(
            view.blocks_per_second, 10.0,
            "an absent worker must not be counted into the fleet's rate"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_lost_lease_tells_the_worker_to_stop() {
        let dir = temp("lost");
        let mut m = master(&dir);
        let start = Instant::now();
        let w = m.register(&Register { name: "n".into() }, start).worker;
        let lease = m.work(
            &AskWork {
                worker: w,
                blocks_per_second: Some(10.0),
                finished: None,
            },
            start,
        );

        let late = start + Duration::from_secs(61);
        let renewed = m.heartbeat(
            &Heartbeat {
                lease: lease.lease,
                blocks_done: 1,
                candidates: 5,
            },
            late,
        );
        assert!(!renewed.held, "an expired lease must not renew");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
