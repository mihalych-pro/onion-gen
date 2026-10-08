//! The fleet, as Prometheus reads it.
//!
//! Built from a [`FleetView`] on every scrape rather than kept beside the
//! state: a second copy updated in parallel would be a second thing to keep
//! right, and a few dozen series cost nothing at scrape intervals.
//!
//! Only the master's own knowledge is here — how many workers it has, how many
//! ranges are out, what it accepted and refused. A worker serves its own
//! metrics; nothing is forwarded through here.

use prometheus_client::encoding::{text::encode, EncodeLabelSet};
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::registry::Registry;

use crate::proto::{safe_label, FleetView};

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct WorkerLabel {
    worker: String,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct PresenceLabel {
    present: String,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct LeaseLabel {
    state: String,
}

/// Which kind of store. A fixed word from the backend, never anything out of
/// a connection string: a host name in a label is a host name in whatever
/// scrapes it, and a password would be worse.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct StoreLabel {
    kind: String,
}

/// The exposition text for one scrape.
pub fn render(view: &FleetView) -> String {
    let mut registry = Registry::default();

    let workers = Family::<PresenceLabel, Gauge>::default();
    registry.register(
        "onion_gen_workers",
        "Workers the master knows about",
        workers.clone(),
    );
    let present = view.workers.iter().filter(|w| w.present).count();
    workers
        .get_or_create(&PresenceLabel {
            present: "true".into(),
        })
        .set(present as i64);
    workers
        .get_or_create(&PresenceLabel {
            present: "false".into(),
        })
        .set((view.workers.len() - present) as i64);

    let rate = Gauge::<f64, std::sync::atomic::AtomicU64>::default();
    registry.register(
        "onion_gen_blocks_per_second",
        "Search rate across present workers",
        rate.clone(),
    );
    rate.set(view.blocks_per_second);

    let finds = Counter::<u64>::default();
    registry.register(
        "onion_gen_finds",
        "Keys accepted and written",
        finds.clone(),
    );
    finds.inc_by(view.finds);

    let rejected = Counter::<u64>::default();
    registry.register(
        "onion_gen_rejected",
        "Reported positions the master refused. A worker failing all of them is \
         a fault to look at rather than bad luck",
        rejected.clone(),
    );
    rejected.inc_by(view.rejected);

    let leases = Family::<LeaseLabel, Gauge>::default();
    registry.register(
        "onion_gen_leases",
        "Ranges handed out, and ranges that came back unfinished",
        leases.clone(),
    );
    leases
        .get_or_create(&LeaseLabel {
            state: "out".into(),
        })
        .set(view.leases_out as i64);
    leases
        .get_or_create(&LeaseLabel {
            state: "returned".into(),
        })
        .set(view.leases_returned as i64);

    // Per worker from here. Without these, a fleet that lost half its nodes and
    // a fleet where half the nodes degraded look alike in the total and call
    // for opposite responses.
    let worker_rate = Family::<WorkerLabel, Gauge<f64, std::sync::atomic::AtomicU64>>::default();
    registry.register(
        "onion_gen_worker_blocks_per_second",
        "Search rate of one worker, as the master last heard it",
        worker_rate.clone(),
    );
    let worker_finds = Family::<WorkerLabel, Gauge>::default();
    registry.register(
        "onion_gen_worker_finds",
        "Keys the master accepted from one worker",
        worker_finds.clone(),
    );
    let silent = Family::<WorkerLabel, Gauge>::default();
    registry.register(
        "onion_gen_worker_silent_seconds",
        "How long since a worker last said anything. Past the lease period it \
         is absent rather than slow",
        silent.clone(),
    );
    for w in &view.workers {
        let label = WorkerLabel {
            worker: safe_label(&w.name),
        };
        worker_rate.get_or_create(&label).set(w.blocks_per_second);
        worker_finds.get_or_create(&label).set(w.finds as i64);
        silent.get_or_create(&label).set(w.silent_for as i64);
    }

    // The store. A fleet's one dependency on somebody else's service, so
    // "is it connected" and "is it falling over" are worth answering here
    // rather than in a log nobody is reading at three in the morning.
    let label = StoreLabel {
        kind: safe_label(view.store.kind),
    };

    let connections = Family::<StoreLabel, Gauge>::default();
    registry.register(
        "onion_gen_store_connections",
        "Connections the store holds open. A file counts as one",
        connections.clone(),
    );
    connections
        .get_or_create(&label)
        .set(i64::from(view.store.connections));

    let idle = Family::<StoreLabel, Gauge>::default();
    registry.register(
        "onion_gen_store_connections_idle",
        "Of those, how many are not in use",
        idle.clone(),
    );
    idle.get_or_create(&label).set(i64::from(view.store.idle));

    let rows = Family::<StoreLabel, Counter>::default();
    registry.register(
        "onion_gen_store_rows",
        "Keys written to the store. A repeat of an address is not counted",
        rows.clone(),
    );
    rows.get_or_create(&label).inc_by(view.store.rows);

    let bytes = Family::<StoreLabel, Counter>::default();
    registry.register(
        "onion_gen_store_bytes",
        "Key material written, before encoding: 160 bytes a key",
        bytes.clone(),
    );
    bytes.get_or_create(&label).inc_by(view.store.bytes);

    let batches = Family::<StoreLabel, Counter>::default();
    registry.register(
        "onion_gen_store_batches",
        "Batches committed. Finds go down a thousand at a time or once a second",
        batches.clone(),
    );
    batches.get_or_create(&label).inc_by(view.store.batches);

    let failures = Family::<StoreLabel, Counter>::default();
    registry.register(
        "onion_gen_store_failures",
        "Batches the store refused. Anything above zero is a fleet losing time \
         and a store worth looking at",
        failures.clone(),
    );
    failures.get_or_create(&label).inc_by(view.store.failures);

    let mut out = String::new();
    encode(&mut out, &registry).expect("writing to a String cannot fail");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::WorkerView;

    fn worker(name: &str, rate: f64, present: bool) -> WorkerView {
        WorkerView {
            worker: 1,
            name: name.into(),
            blocks_per_second: rate,
            candidates: 1_000,
            finds: 2,
            silent_for: if present { 3 } else { 900 },
            present,
        }
    }

    fn view(workers: Vec<WorkerView>) -> FleetView {
        FleetView {
            blocks_per_second: workers
                .iter()
                .filter(|w| w.present)
                .map(|w| w.blocks_per_second)
                .sum(),
            candidates: workers.iter().map(|w| w.candidates).sum(),
            finds: 5,
            rejected: 1,
            leases_out: 2,
            leases_returned: 0,
            workers,
            store: crate::proto::StoreView {
                kind: "postgres",
                connections: 2,
                idle: 1,
                rows: 7,
                bytes: 1120,
                batches: 3,
                failures: 0,
            },
        }
    }

    #[test]
    fn a_present_and_an_absent_worker_are_counted_apart() {
        let text = render(&view(vec![
            worker("here", 10.0, true),
            worker("gone", 90.0, false),
        ]));
        assert!(
            text.contains("onion_gen_workers{present=\"true\"} 1"),
            "{text}"
        );
        assert!(
            text.contains("onion_gen_workers{present=\"false\"} 1"),
            "{text}"
        );
        assert!(
            text.contains("onion_gen_blocks_per_second 10"),
            "an absent worker must not be in the fleet rate: {text}"
        );
    }

    #[test]
    fn each_worker_keeps_its_own_series() {
        let text = render(&view(vec![
            worker("alpha", 10.0, true),
            worker("beta", 20.0, true),
        ]));
        assert!(text.contains("onion_gen_worker_blocks_per_second{worker=\"alpha\"} 10"));
        assert!(text.contains("onion_gen_worker_blocks_per_second{worker=\"beta\"} 20"));
        assert!(text.contains("onion_gen_worker_silent_seconds{worker=\"alpha\"} 3"));
    }

    /// The library passes label values through untouched, so a worker could
    /// otherwise end a label early and take the rest of the scrape with it.
    #[test]
    fn a_hostile_worker_name_cannot_break_the_exposition() {
        let text = render(&view(vec![worker("ev\"il\\node", 1.0, true)]));
        assert!(
            text.contains("worker=\"ev_il_node\""),
            "the name must be made safe: {text}"
        );
        for line in text
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
        {
            assert_eq!(
                line.matches('"').count() % 2,
                0,
                "unbalanced quotes would end the scrape here: {line}"
            );
        }
    }

    #[test]
    fn counters_and_gauges_are_declared_as_such() {
        let text = render(&view(vec![worker("n", 1.0, true)]));
        assert!(text.contains("# TYPE onion_gen_finds counter"), "{text}");
        assert!(text.contains("# TYPE onion_gen_leases gauge"), "{text}");
        assert!(text.contains("onion_gen_finds_total 5"), "{text}");
    }

    /// The store is the one part of a fleet that is somebody else's service,
    /// so its numbers have to reach the scrape.
    #[test]
    fn the_store_is_reported_with_its_kind() {
        let text = render(&view(vec![worker("n", 1.0, true)]));
        assert!(
            text.contains("onion_gen_store_connections{kind=\"postgres\"} 2"),
            "{text}"
        );
        assert!(
            text.contains("onion_gen_store_rows_total{kind=\"postgres\"} 7"),
            "{text}"
        );
        assert!(
            text.contains("onion_gen_store_bytes_total{kind=\"postgres\"} 1120"),
            "{text}"
        );
        assert!(
            text.contains("onion_gen_store_failures_total{kind=\"postgres\"} 0"),
            "{text}"
        );
        assert!(
            text.contains("# TYPE onion_gen_store_rows counter"),
            "{text}"
        );
        assert!(
            text.contains("# TYPE onion_gen_store_connections gauge"),
            "{text}"
        );
    }

    #[test]
    fn a_fleet_with_no_workers_still_renders() {
        let text = render(&view(vec![]));
        assert!(text.contains("onion_gen_workers{present=\"true\"} 0"));
        assert!(text.contains("onion_gen_finds_total 5"));
    }
}
