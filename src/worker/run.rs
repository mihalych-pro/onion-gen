//! The worker's loop: take a lease, search it, hand back what turns up.
//!
//! The boundary between searching and talking is drawn on purpose. The search
//! runs where it always ran — its own threads, its own engines — and every HTTP
//! call is made from this thread through `block_on`. Nothing the search does
//! ever executes on the runtime's threads, so one worker saturating its cores
//! cannot stop another's request being answered.

use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::filter::FilterSet;
use crate::proto::{Found, Heartbeat};
use crate::run::{self, RunOptions};
use crate::worker::{Buffer, Client};

/// How much of a lease passes between heartbeats.
///
/// A third, so two may be lost before the master gives the range away. Renewing
/// more often would spend requests on nothing; less often leaves no margin.
const HEARTBEAT_SHARE: u32 = 3;

pub struct Settings {
    pub master: String,
    pub name: String,
    pub endpoint: String,
    pub buffer_dir: PathBuf,
    pub buffer_limit: usize,
}

/// Runs until stopped, which for a replica means until the orchestrator says so.
pub fn run(settings: &Settings, base: &RunOptions) -> io::Result<()> {
    let client = Client::new(&settings.master)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        // Two threads: one request is ever in flight from here, and the second
        // is for the connection task that drives it. The search gets the rest
        // of the machine.
        .worker_threads(2)
        .enable_all()
        .build()?;

    let health = Arc::new(crate::worker::Health::default());
    // Up before registration: an orchestrator starts probing as soon as the
    // container does, and a refused connection is a worse answer than an
    // honest "not ready yet".
    match settings.endpoint.parse::<std::net::SocketAddr>() {
        Ok(addr) => {
            let serving = Arc::clone(&health);
            let name = settings.name.clone();
            runtime.spawn(async move {
                if let Err(e) = crate::worker::probe::serve(addr, name, serving).await {
                    eprintln!("onion-gen: the worker endpoint stopped: {e}");
                }
            });
        }
        Err(e) => eprintln!(
            "onion-gen: --endpoint {}: {e}; not serving",
            settings.endpoint
        ),
    }

    let me = runtime.block_on(client.register(&settings.name))?;
    health.registered(true);
    eprintln!(
        "onion-gen: worker {} registered as {}, {} filter(s) from the master",
        settings.name,
        me.worker,
        me.filters.len()
    );
    let filters = FilterSet::parse_all(&me.filters)
        .map_err(|(raw, e)| io::Error::other(format!("the master sent filter {raw:?}: {e}")))?;

    let buffer = Arc::new(Mutex::new(Buffer::new(
        settings.buffer_dir.clone(),
        settings.buffer_limit,
    )));
    let stop = Arc::new(AtomicBool::new(false));
    install_stop(&stop)?;

    let mut measured: Option<f64> = None;
    let mut finished: Option<u64> = None;
    let mut total_blocks: u64 = 0;

    while !stop.load(Ordering::Relaxed) {
        // Anything still owed to the master goes first: a find held over from
        // an outage is older than anything this lease will produce.
        flush(&runtime, &client, &buffer)?;

        let lease = match runtime.block_on(client.work(me.worker, measured, finished.take())) {
            Ok(lease) => lease,
            Err(e) => {
                health.holding(false);
                eprintln!("onion-gen: no work from the master ({e}); retrying");
                sleep_unless_stopped(&stop, Duration::from_secs(5));
                continue;
            }
        };
        let seed = parse_seed(&lease.seed)?;
        let held = Arc::new(AtomicBool::new(true));
        let done = Arc::new(AtomicU64::new(0));

        let beat = heartbeat_thread(
            Arc::clone(&health),
            total_blocks,
            &runtime,
            &client,
            lease.lease,
            Duration::from_secs(lease.expires_in.max(3) / u64::from(HEARTBEAT_SHARE)),
            Arc::clone(&held),
            Arc::clone(&done),
            Arc::clone(&stop),
        );

        health.holding(true);
        let started = Instant::now();
        let summary = search(
            base,
            &filters,
            seed,
            lease.lease,
            lease.first_block,
            lease.blocks,
            &runtime,
            &client,
            &buffer,
            Arc::clone(&held),
            Arc::clone(&stop),
            Arc::clone(&done),
        )?;
        // Cleared before joining. Leaving it set meant the heartbeat thread
        // looped for ever and `join` never returned: the worker finished its
        // first lease and then sat there, holding one range, for the life of
        // the process.
        held.store(false, Ordering::Relaxed);
        beat.join().ok();

        let elapsed = started.elapsed().as_secs_f64();
        let blocks = done.load(Ordering::Relaxed) as f64;
        // Any lease that did real work is worth measuring. Demanding half a
        // second of it would disqualify a fast worker: the first lease is 256
        // blocks and finishes in less, so no rate would ever be measured, every
        // later lease would stay 256 blocks, and the fleet view would show
        // zero for ever.
        if elapsed > 0.0 && blocks > 0.0 {
            measured = Some(blocks / elapsed);
            health.rate(blocks / elapsed);
        }
        health.advanced(total_blocks);
        total_blocks += done.load(Ordering::Relaxed);
        if let Ok(b) = buffer.lock() {
            if let Ok(n) = b.len() {
                health.buffered(n as u64);
            }
        }
        let _ = summary;
        finished = Some(lease.lease);
    }
    flush(&runtime, &client, &buffer)?;
    Ok(())
}

/// Searches one leased range, reporting finds as they happen.
#[allow(clippy::too_many_arguments)]
fn search(
    base: &RunOptions,
    filters: &FilterSet,
    seed: [u8; 32],
    lease_id: u64,
    first_block: u64,
    blocks: u64,
    runtime: &tokio::runtime::Runtime,
    client: &Client,
    buffer: &Arc<Mutex<Buffer>>,
    held: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    done: Arc<AtomicU64>,
) -> io::Result<run::Summary> {
    let mut opts = base.clone();
    opts.root_seed = seed;
    // The lease's own numbering, not a copy starting at zero. A block index is
    // what the seed is derived from, so searching block 0 while telling the
    // master it was block 400 makes the master derive a different key and
    // refuse a find that was perfectly good.
    opts.first_block = first_block;
    opts.limit = None;
    opts.block_limit = Some(blocks);
    opts.progress = Some(Arc::clone(&done));
    opts.print_addresses = false;
    opts.state_path = None;

    let sink_client = client.clone();
    let sink_buffer = Arc::clone(buffer);
    let sink_runtime = runtime.handle().clone();
    let sink_stop = Arc::clone(&stop);
    let sink_held = Arc::clone(&held);
    opts.report = Some(Arc::new(move |block, offset| {
        let find = Found {
            lease: lease_id,
            block,
            offset,
        };
        // Written down before anything is attempted with it. A find that took a
        // week will not come round again, and the network is the part that
        // fails.
        if let Ok(mut b) = sink_buffer.lock() {
            let _ = b.keep(&find);
        }
        match sink_runtime.block_on(sink_client.found(&find)) {
            Ok(answer) => {
                if let Ok(mut b) = sink_buffer.lock() {
                    if let Ok(pending) = b.pending() {
                        if let Some((path, _)) = pending.iter().find(|(_, f)| {
                            f.lease == find.lease
                                && f.block == find.block
                                && f.offset == find.offset
                        }) {
                            let _ = b.acknowledge(path);
                        }
                    }
                }
                if !answer.accepted {
                    eprintln!(
                        "onion-gen: the master refused a find: {}",
                        answer.why.unwrap_or_else(|| "no reason given".into())
                    );
                }
            }
            // Kept in the buffer and sent later. Not an error the run should
            // stop for: the master comes back, the find is still here.
            Err(e) => eprintln!("onion-gen: could not hand over a find ({e}); it is buffered"),
        }
        !sink_stop.load(Ordering::Relaxed) && sink_held.load(Ordering::Relaxed)
    }));

    run::run(&opts, filters)
}

/// Keeps the lease alive while the search runs.
#[allow(clippy::too_many_arguments)]
fn heartbeat_thread(
    health: Arc<crate::worker::Health>,
    blocks_before: u64,
    runtime: &tokio::runtime::Runtime,
    client: &Client,
    lease: u64,
    every: Duration,
    held: Arc<AtomicBool>,
    done: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
) -> std::thread::JoinHandle<()> {
    let handle = runtime.handle().clone();
    let client = client.clone();
    std::thread::spawn(move || {
        while held.load(Ordering::Relaxed) && !stop.load(Ordering::Relaxed) {
            std::thread::sleep(every);
            if !held.load(Ordering::Relaxed) {
                break;
            }
            let so_far = done.load(Ordering::Relaxed);
            // Progress inside a lease counts as movement: a long range must not
            // look wedged just because it has not finished.
            health.advanced(blocks_before + so_far);
            let beat = Heartbeat {
                lease,
                blocks_done: so_far,
                candidates: 0,
            };
            match handle.block_on(client.heartbeat(&beat)) {
                Ok(answer) if !answer.held => {
                    // The master gave this range to somebody else. Carrying on
                    // would be searching ground that is now another worker's.
                    eprintln!("onion-gen: the lease was lost; stopping this range");
                    held.store(false, Ordering::Relaxed);
                }
                Ok(_) => {}
                // An unreachable master is not a reason to abandon the range:
                // the work is still worth doing and the finds are still worth
                // keeping.
                Err(e) => eprintln!("onion-gen: heartbeat failed ({e}); carrying on"),
            }
        }
    })
}

/// Hands over everything the buffer is still holding.
fn flush(
    runtime: &tokio::runtime::Runtime,
    client: &Client,
    buffer: &Arc<Mutex<Buffer>>,
) -> io::Result<()> {
    let pending = {
        let b = buffer.lock().unwrap_or_else(|e| e.into_inner());
        b.pending()?
    };
    if pending.is_empty() {
        return Ok(());
    }
    eprintln!(
        "onion-gen: {} find(s) waiting to be handed over",
        pending.len()
    );
    for (path, find) in pending {
        match runtime.block_on(client.found(&find)) {
            Ok(_) => {
                let mut b = buffer.lock().unwrap_or_else(|e| e.into_inner());
                b.acknowledge(&path)?;
            }
            // Stop at the first failure: the master is down, and walking the
            // rest would only produce the same error once per find.
            Err(e) => {
                eprintln!("onion-gen: the master is still unreachable ({e})");
                break;
            }
        }
    }
    Ok(())
}

fn parse_seed(text: &str) -> io::Result<[u8; 32]> {
    if text.len() != 64 {
        return Err(io::Error::other(
            "the master sent a seed that is not 64 hex characters",
        ));
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16)
            .map_err(|e| io::Error::other(format!("the master sent an unreadable seed: {e}")))?;
    }
    Ok(out)
}

#[cfg(unix)]
fn install_stop(stop: &Arc<AtomicBool>) -> io::Result<()> {
    for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        signal_hook::flag::register(signal, Arc::clone(stop))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn install_stop(stop: &Arc<AtomicBool>) -> io::Result<()> {
    // The registration hands back an id for removing the handler again, which
    // nothing here wants: the flag lives as long as the process does.
    signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(stop))?;
    Ok(())
}

fn sleep_unless_stopped(stop: &Arc<AtomicBool>, how_long: Duration) {
    let until = Instant::now() + how_long;
    while Instant::now() < until && !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_seed_round_trips_through_its_hex() {
        let text = "2f".repeat(32);
        assert_eq!(parse_seed(&text).expect("valid"), [0x2fu8; 32]);
    }

    #[test]
    fn a_seed_of_the_wrong_shape_is_refused() {
        assert!(parse_seed("2f").is_err(), "too short");
        assert!(parse_seed(&"zz".repeat(32)).is_err(), "not hex");
    }
}
