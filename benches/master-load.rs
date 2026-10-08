//! Where the master stops keeping up.
//!
//! Two ceilings, and they are reached by different things, so they are measured
//! apart.
//!
//! The first is lease traffic: workers asking for work and renewing what they
//! hold. Cheap per request — a mutex and some arithmetic — and it scales with
//! the number of workers.
//!
//! The second is finds. Each one makes the master derive a key, build an
//! address and write three files, and it does that under the same mutex. It
//! scales with how common the filter is rather than with how many workers there
//! are, and it is the one that bites first.
//!
//! Run with: cargo run --release --example master-load -- <master-url>

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use onion_gen::proto::*;
use onion_gen::worker::Client;

#[tokio::main(flavor = "multi_thread", worker_threads = 8)]
async fn main() {
    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "http://127.0.0.1:18090".into());
    let client = Client::new(&url).expect("a master address");

    println!("== 1. Lease traffic: how many workers before the master lags ==\n");
    println!(
        "  {:>8}  {:>10}  {:>10}  {:>10}",
        "workers", "req/s", "p50", "p99"
    );
    for workers in [1usize, 10, 50, 100, 200, 400] {
        let (rate, p50, p99) = lease_round(&client, workers).await;
        println!(
            "  {workers:>8}  {rate:>10.0}  {:>8.2}ms  {:>8.2}ms",
            p50.as_secs_f64() * 1e3,
            p99.as_secs_f64() * 1e3
        );
    }

    println!("\n== 2. Finds: how many the master can take ==\n");
    find_round(&client).await;
}

/// One round of what a fleet actually sends: a work request and the heartbeats
/// that follow it, from `workers` at once.
async fn lease_round(client: &Client, workers: usize) -> (f64, Duration, Duration) {
    let mut ids = Vec::with_capacity(workers);
    for i in 0..workers {
        let me = client
            .register(&format!("load-{i}"))
            .await
            .expect("registration");
        ids.push(me.worker);
    }

    let latencies = Arc::new(std::sync::Mutex::new(Vec::with_capacity(workers * 4)));
    let started = Instant::now();
    let mut tasks = Vec::with_capacity(workers);
    for id in ids {
        let client = client.clone();
        let latencies = Arc::clone(&latencies);
        tasks.push(tokio::spawn(async move {
            for _ in 0..4 {
                let at = Instant::now();
                // Deliberately asking for tiny leases: the point is the request
                // rate, not the work handed out.
                let lease = client.work(id, Some(1.0), None).await.expect("work");
                let took = at.elapsed();
                latencies.lock().expect("lock").push(took);

                let at = Instant::now();
                let _ = client
                    .heartbeat(&Heartbeat {
                        lease: lease.lease,
                        blocks_done: 1,
                        candidates: 1,
                    })
                    .await
                    .expect("heartbeat");
                latencies.lock().expect("lock").push(at.elapsed());
            }
        }));
    }
    for t in tasks {
        t.await.expect("a task");
    }
    let elapsed = started.elapsed();

    let mut taken = latencies.lock().expect("lock").clone();
    taken.sort();
    let requests = taken.len() as f64;
    let p = |q: f64| taken[((taken.len() as f64 - 1.0) * q) as usize];
    (requests / elapsed.as_secs_f64(), p(0.50), p(0.99))
}

/// How fast finds can be handed over, which is the ceiling that bites first.
async fn find_round(client: &Client) {
    let me = client.register("load-finds").await.expect("registration");
    let lease = client
        .work(me.worker, Some(1000.0), None)
        .await
        .expect("work");

    let accepted = Arc::new(AtomicU64::new(0));
    let refused = Arc::new(AtomicU64::new(0));
    let started = Instant::now();
    let mut sent = 0u64;

    // Positions that will not be keys most of the time, so this measures the
    // master's path rather than the disk. The disk number is measured in
    // `score-calibration` and is 185 keys a second.
    while started.elapsed() < Duration::from_secs(5) {
        let mut tasks = Vec::with_capacity(32);
        for i in 0..32u64 {
            let client = client.clone();
            let accepted = Arc::clone(&accepted);
            let refused = Arc::clone(&refused);
            let find = Found {
                lease: lease.lease,
                block: lease.first_block + (sent + i) % lease.blocks,
                offset: (sent + i) * 8,
            };
            tasks.push(tokio::spawn(async move {
                match client.found(&find).await {
                    Ok(answer) if answer.accepted => accepted.fetch_add(1, Ordering::Relaxed),
                    Ok(_) => refused.fetch_add(1, Ordering::Relaxed),
                    Err(_) => refused.fetch_add(1, Ordering::Relaxed),
                };
            }));
        }
        for t in tasks {
            let _ = t.await;
        }
        sent += 32;
    }
    let elapsed = started.elapsed().as_secs_f64();
    println!(
        "  {:.0} reports/s handled ({} accepted, {} refused)",
        sent as f64 / elapsed,
        accepted.load(Ordering::Relaxed),
        refused.load(Ordering::Relaxed)
    );
    println!(
        "\n  A four-symbol filter is one address in {}, so one RTX 4060 at 960 M/s\n  \
         produces {:.0} finds a second on its own.",
        32u64.pow(4),
        960.7e6 / 32f64.powi(4)
    );
}
