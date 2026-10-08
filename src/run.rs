//! Driving a search run: worker threads, stop conditions, statistics.
//!
//! Work is split into blocks: one seed plus a bounded walk along the `+8G`
//! chain. Blocks come from a single atomic counter, so no block is processed
//! twice and threads cannot duplicate work by construction.
//!
//! The run's root seed gives every block a stable identity, which is what
//! resuming and the distributed mode are built on.

use crate::batch::CHAIN_STEP;
use crate::batch4::BatchEngine4;
use crate::curve::{self, Point};
use crate::filter::{Filter, FilterSet, KeyVerdict};
use crate::key;
use crate::pairs::PairEngine;
use crate::score;
use crate::store::{Find, Store};
use sha2::{Digest, Sha512};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How many batches one block walks before the worker takes a new one.
///
/// It bounds how far a block reaches along the chain, so blocks cannot overlap,
/// and it bounds how much work is lost when a run stops mid-block.
pub const BATCHES_PER_BLOCK: u64 = 64;

/// Settings for one run, already resolved. Nothing here is read again while the
/// search is running.
///
/// Cloneable so that a worker can take its own copy per lease: the seed and the
/// block range change every time, and everything else — threads, devices,
/// filters' index width — stays as the command line set it.
#[derive(Clone)]
pub struct RunOptions {
    pub threads: usize,
    pub batch_size: usize,
    /// Stop after this many hits; `None` runs until interrupted.
    pub limit: Option<u64>,
    /// Print statistics at this interval; `None` prints none.
    pub stats_interval: Option<Duration>,
    pub out_dir: PathBuf,
    /// Keep finds as rows in a database instead of as directories under
    /// `out_dir`. A connection string: a path, or `postgres://`, `mysql://`
    /// or `sqlite://`.
    pub db: Option<String>,
    /// Print each found address on stdout.
    pub print_addresses: bool,
    pub root_seed: [u8; 32],
    /// Run the scalar arithmetic even where a vector path exists.
    ///
    /// The scalar path is kept as the reference implementation, so being able
    /// to select it is what makes the two comparable in one sitting.
    pub force_scalar: bool,
    /// Run the vector chain engine instead of the default paired one.
    pub force_vector: bool,
    /// Which devices to run on. Empty means the processor only.
    pub devices: Vec<crate::gpu::Device>,
    /// How many chains each device advances at once. Zero means the device
    /// decides.
    pub device_threads: u32,
    /// Whether a device may say what it settled on.
    pub explain_devices: bool,
    /// Chance that one candidate satisfies the filter set, if it can at all.
    ///
    /// Carried here rather than recomputed because the reporter runs on its
    /// own thread and the filter set is not its to hold.
    pub chance_per_candidate: Option<f64>,
    /// Finds scoring below this are not written. Off unless asked for: an
    /// extra directory can be deleted, whereas a key not written is gone for
    /// good — the same address will not turn up twice.
    pub min_score: Option<f64>,
    /// Where a find goes instead of to disk.
    ///
    /// Set by a worker, whose finds belong to its master: it is given the
    /// block and the offset and answers whether the run should carry on. When
    /// this is set nothing is written locally, because the worker's disk is
    /// not where the fleet's keys live.
    pub report: Option<Arc<dyn Fn(u64, u64) -> bool + Send + Sync>>,
    /// Stop after claiming this many blocks. A worker's lease is a range, and
    /// searching past its end would be searching ground the master has given
    /// to somebody else.
    pub block_limit: Option<u64>,
    /// Blocks claimed so far, for whoever is watching from outside.
    ///
    /// A worker needs it while the run is going, not after: it is what the
    /// heartbeat carries and what the next lease is sized from.
    pub progress: Option<Arc<AtomicU64>>,
    /// Where to keep what the run needs in order to continue. `None` keeps
    /// nothing, and an interrupted run starts over.
    pub state_path: Option<PathBuf>,
    /// The first block to claim, for a caller keeping the run's place itself
    /// rather than in a state file.
    pub first_block: u64,
    /// Emit machine-readable lines instead of prose.
    ///
    /// One JSON object per line and per event, on the stream the prose would
    /// have used: statistics and the summary on the diagnostic stream, a hit
    /// on the output stream, so that redirecting one still separates them.
    pub json: bool,
    /// What earlier stretches of this same search already did.
    ///
    /// Kept apart from the live counters because the live hit count is what
    /// the limit watches: starting it at what a previous run found would stop
    /// this one before it began. These are for the state file and for the
    /// odds, where the whole search is what matters.
    pub carried_candidates: u64,
    pub carried_hits: u64,
    /// Measure the machine again rather than reuse what was measured before.
    pub retune: bool,
}

impl RunOptions {
    pub fn new(out_dir: PathBuf) -> Self {
        RunOptions {
            threads: default_threads(),
            batch_size: crate::batch::DEFAULT_BATCH_SIZE,
            limit: None,
            stats_interval: None,
            out_dir,
            db: None,
            print_addresses: true,
            root_seed: random_seed(),
            force_scalar: false,
            force_vector: false,
            devices: Vec::new(),
            first_block: 0,
            // Zero means "take the largest batch the device accepts".
            device_threads: 0,
            explain_devices: true,
            chance_per_candidate: None,
            min_score: None,
            report: None,
            block_limit: None,
            progress: None,
            state_path: None,
            json: false,
            carried_candidates: 0,
            carried_hits: 0,
            retune: false,
        }
    }
}

/// What a finished run did.
///
/// No longer `Copy`: it carries the best find's address, and a score is a
/// float, so neither `Copy` nor `Eq` survives. Both were conveniences rather
/// than requirements.
#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    /// Whether the run stopped because a key failed its check. A caller must
    /// be able to tell "found nothing" from "found something broken".
    pub defect: bool,
    pub hits: u64,
    pub candidates: u64,
    /// Of those, how many the devices produced.
    pub device_candidates: u64,
    /// Survivors a device reported whose position did not derive to the key it
    /// reported. A kernel defect, and never anything else.
    pub device_mismatch: u64,
    pub blocks: u64,
    /// The highest-scoring find and its score, when there was one.
    pub best: Option<(f64, String)>,
    /// How many finds the score threshold turned away.
    pub below_score: u64,
    /// Whether the run got what it was asked for.
    ///
    /// The only thing an orchestrator has to go on. It does not talk to the
    /// process; it looks at how the process ended, so "found what was asked"
    /// and "was stopped before finding it" must not end the same way. A run
    /// with no limit has no goal to reach and so never sets this.
    pub reached_goal: bool,
}

pub fn default_threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

/// How many worker threads to run when the user has not said, given how many
/// devices are also working.
///
/// A device has a half that runs on the processor: it queues launches and
/// collects what survived. With every core taken by the search, that half has
/// no core to run on and the total falls — measured on sixteen threads with a
/// card, 958.7 M candidates/s against 963.8 on fifteen. So each device is left
/// a core.
///
/// One core is always kept working: a processor that contributes 7 % is worth
/// having, and on a single-core machine the alternative would be nothing.
pub fn default_threads_with(devices: usize) -> usize {
    default_threads().saturating_sub(devices).max(1)
}

pub fn random_seed() -> [u8; 32] {
    use rand::Rng;
    let mut seed = [0u8; 32];
    rand::rng().fill_bytes(&mut seed);
    seed
}

/// The seed of block `index`, derived from the run's root seed.
///
/// Domain-separated so that a block seed cannot coincide with anything else
/// derived from the same root.
pub fn block_seed(root: &[u8; 32], index: u64) -> [u8; 32] {
    let mut hasher = Sha512::new();
    hasher.update(b"onion-gen block v1");
    hasher.update(root);
    hasher.update(index.to_le_bytes());
    let digest = hasher.finalize();
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&digest[..32]);
    seed
}

struct Shared {
    next_block: AtomicU64,
    candidates: AtomicU64,
    /// Counted apart from the processor's, because "the device did not attach"
    /// and "the device attached and is slow" need opposite answers.
    device_candidates: AtomicU64,
    /// Survivors whose reported position did not derive to the key the device
    /// reported. Always zero unless a kernel's bookkeeping is wrong.
    device_mismatch: AtomicU64,
    hits: AtomicU64,
    /// Finds the score threshold turned away. Counted apart from `hits`,
    /// which is what `--limit` stops on: a limit of five with a threshold set
    /// means five keys kept, not five addresses looked at and none written.
    below_score: AtomicU64,
    stop: Arc<AtomicBool>,
    /// Set when a found key failed its check. Every worker in flight will hit
    /// the same defect, and one report is a diagnosis while eight are noise.
    defect: AtomicBool,
    /// Where finds go, and the lock every find takes on its way there.
    writer: Mutex<Store>,
    /// The best find of the run, kept because the output is never reordered:
    /// finds go out as they arrive, holding them back for the sake of order
    /// would lose them all on an interrupt, and the best one can land in the
    /// middle of a long list. Behind the writer lock, which every find takes
    /// anyway.
    best: Mutex<Option<(f64, String)>>,
}

/// Whether this run has claimed as many blocks as it was allowed.
///
/// Checked where a block is claimed rather than where one finishes: claiming is
/// the moment the range grows, and a worker must not reach past its lease even
/// by one block.
#[inline]
fn block_allowed(opts: &RunOptions, block: u64) -> bool {
    match opts.block_limit {
        Some(limit) => block < opts.first_block + limit,
        None => true,
    }
}

/// Runs the search until the limit is reached or a signal arrives.
pub fn run(opts: &RunOptions, filters: &FilterSet) -> io::Result<Summary> {
    assert!(
        opts.threads > 0 || !opts.devices.is_empty(),
        "with no threads and no device there would be nobody to search"
    );

    let stop = Arc::new(AtomicBool::new(false));
    install_signal_handlers(&stop)?;

    // A batch size of zero means the caller wants one measured. Done here and
    // not before, so that a Ctrl-C during the measurement stops the run rather
    // than killing the process.
    let mut opts = opts.clone();
    // Only where the processor is actually searching. With `--threads 0` the
    // devices do all of it and the batch is never read, so measuring one would
    // be a second spent on an answer nobody uses.
    if opts.batch_size == 0 && opts.threads > 0 {
        let key = crate::tuning::cpu_key(opts.threads);
        let cached = (!opts.retune).then(|| crate::tuning::get(&key)).flatten();
        let (size, rate, remembered) = match cached {
            Some(shape) => (shape.first as usize, shape.rate, true),
            None => {
                let (size, rate) =
                    crate::estimate::tune_batch(opts.threads, Duration::from_millis(200));
                crate::tuning::put(
                    &key,
                    crate::tuning::Shape {
                        first: size as u32,
                        second: 0,
                        rate,
                        at: crate::tuning::now(),
                    },
                );
                (size, rate, false)
            }
        };
        opts.batch_size = size;
        if opts.print_addresses && !opts.json {
            let how = if remembered {
                "measured here before"
            } else {
                "the fastest measured here"
            };
            eprintln!("onion-gen: batch {size}, {how}, at {:.1} M/s", rate / 1e6);
        }
    }
    if opts.batch_size == 0 {
        opts.batch_size = crate::batch::DEFAULT_BATCH_SIZE;
    }
    let opts = &opts;

    // Where a previous run of this search got to, if there is one.
    let resumed = match &opts.state_path {
        Some(path) => RunState::load(path)?,
        None => None,
    };
    // Whichever is further along: a caller may give both a state file and a
    // block on the command line, and going backwards would re-examine what is
    // known to have been examined.
    let first_block = resumed
        .as_ref()
        .map_or(0, |s| s.next_block)
        .max(opts.first_block);

    // Opened before any worker starts: a database that cannot be opened is a
    // run that would search for hours and have nowhere to put what it found.
    let store = match &opts.db {
        Some(url) => crate::store::open(url)?,
        None => Store::directory(opts.out_dir.clone()),
    };

    let shared = Arc::new(Shared {
        next_block: AtomicU64::new(first_block),
        candidates: AtomicU64::new(0),
        device_candidates: AtomicU64::new(0),
        device_mismatch: AtomicU64::new(0),
        hits: AtomicU64::new(0),
        below_score: AtomicU64::new(0),
        best: Mutex::new(None),
        stop: Arc::clone(&stop),
        defect: AtomicBool::new(false),
        writer: Mutex::new(store),
    });

    let started = Instant::now();
    let flusher = spawn_flusher(Arc::clone(&shared));
    let keeper = opts
        .state_path
        .as_ref()
        .map(|path| spawn_state_keeper(Arc::clone(&shared), opts, filters, path.clone()));
    let reporter = opts.stats_interval.map(|interval| {
        spawn_reporter(
            Arc::clone(&shared),
            interval,
            started,
            opts.chance_per_candidate,
            opts.carried_candidates,
            opts.json,
        )
    });

    std::thread::scope(|scope| {
        for _ in 0..opts.threads {
            let shared = Arc::clone(&shared);
            scope.spawn(move || worker(opts, filters, &shared));
        }
        for device in &opts.devices {
            let shared = Arc::clone(&shared);
            scope.spawn(move || {
                if let Err(e) = device_worker(opts, filters, &shared, device) {
                    // A device that stops is not a reason to stop the run: the
                    // processor keeps going and the user is told why one path
                    // went quiet.
                    eprintln!("onion-gen: device {} stopped: {e}", device.index);
                }
            });
        }
    });

    shared.stop.store(true, Ordering::Relaxed);
    let _ = flusher.join();
    // Before the summary says how many were found: the count is already final,
    // and this is what makes it true on disk as well.
    shared
        .writer
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .flush()?;
    if let Some(handle) = reporter {
        let _ = handle.join();
    }
    if let Some(handle) = keeper {
        let _ = handle.join();
    }
    // The last word, after every worker has stopped: whatever the timer wrote
    // is at most one interval behind, and this is free.
    if let Some(path) = &opts.state_path {
        RunState::of(opts, filters, &shared).save(path)?;
    }

    // Taken out before the summary is built: the lock guard would otherwise
    // borrow `shared` for longer than the value it is being put into lives.
    let best = shared
        .best
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();

    Ok(Summary {
        defect: shared.defect.load(Ordering::Relaxed),
        hits: shared.hits.load(Ordering::Relaxed),
        candidates: shared.candidates.load(Ordering::Relaxed),
        device_candidates: shared.device_candidates.load(Ordering::Relaxed),
        device_mismatch: shared.device_mismatch.load(Ordering::Relaxed),
        blocks: shared.next_block.load(Ordering::Relaxed),
        below_score: shared.below_score.load(Ordering::Relaxed),
        reached_goal: opts
            .limit
            .is_some_and(|wanted| shared.hits.load(Ordering::Relaxed) >= wanted),
        best,
    })
}

/// Picks an engine once, outside the loop, and runs it.
///
/// The paired engine is the default: it reads two candidates from one base
/// point and runs about 1.2x the vector chain. The two enumerate the same
/// space in different orders, which is invisible above this point because a
/// find reports the scalar delta it was derived from rather than a position.
fn worker(opts: &RunOptions, filters: &FilterSet, shared: &Shared) {
    if opts.force_vector && uses_vector_path() {
        worker_vector(opts, filters, shared);
    } else {
        worker_scalar(opts, filters, shared);
    }
}

/// Whether a vector field implementation exists on this machine.
pub fn uses_vector_path() -> bool {
    crate::field4::active() != crate::field4::Implementation::Scalar
}

/// Confirms a hit whose filter the key span could only partly settle.
///
/// A literal prefix longer than the key-derived span is confirmed by comparing
/// text, which is what `canonical` is for. An expression has no canonical
/// base32 text — `canonical` holds the expression itself — so comparing it
/// here would test whether the address begins with something like `^ab[2-7]`
/// and throw every hit away. It is confirmed by running it.
fn confirms(filter: &Filter, public: &[u8; 32]) -> bool {
    let address = key::address(public);
    match filter.expression() {
        Some(expression) => expression.find(address.as_bytes()).is_some(),
        None => address.starts_with(filter.canonical()),
    }
}

/// The work shared by both engines: examine one batch, record what matched.
///
/// Returns `false` when the run should stop.
#[inline]
#[allow(clippy::too_many_arguments)]
fn scan_batch(
    opts: &RunOptions,
    filters: &FilterSet,
    shared: &Shared,
    seed: &[u8; 32],
    block: u64,
    delta: impl Fn(usize) -> u64,
    candidates: &[(u32, [u8; 32])],
    source: &str,
    mut with_sign: impl FnMut(usize, &[u8; 32]) -> [u8; 32],
) -> bool {
    // Each hit carries the filter it matched, so the report can say where in
    // the address the match happened. Only hits pay for the extra word.
    let mut matched: Vec<(usize, [u8; 32], &Filter)> = Vec::new();
    // The two loops differ only in the address pass, and the difference is
    // decided once per batch rather than once per candidate. One loop with the
    // test inside costs about a percent on prefix-only dictionary search.
    if filters.prefers_address() {
        // The whole address, every candidate, one automaton pass. Costs a
        // SHA3-256 that the key-side path would often avoid, and saves a
        // prefilter that is linear in the number of filters — which at a
        // thousand substrings is the difference between 0.1 and several
        // million candidates a second.
        for &(i, ref candidate) in candidates {
            let i = i as usize;
            match filters.examine_key(candidate) {
                KeyVerdict::Matched(f) => matched.push((i, *candidate, f)),
                KeyVerdict::No => {}
                KeyVerdict::NeedsAddress => {
                    let full = with_sign(i, candidate);
                    let address = key::address_bytes(&full);
                    if let Some(f) = filters.match_whole_address(candidate, &address) {
                        matched.push((i, *candidate, f));
                    }
                }
            }
        }
    } else if filters.needs_checksum() {
        for &(i, ref candidate) in candidates {
            let i = i as usize;
            let hit = match filters.match_key(candidate) {
                Some(f) => Some(f),
                // The only place the address is built in the hot loop. A suffix
                // cannot be settled from the key, so its candidates pay a
                // SHA3-256 here; the prefilter keeps that rare whenever the form
                // reaches into the key at all. What comes back has been matched
                // against the whole address, so it needs no confirming.
                None if filters.checksum_possible(candidate) => {
                    let full = with_sign(i, candidate);
                    match filters.match_address(&key::address_bytes(&full)) {
                        Some(f) => {
                            matched.push((i, *candidate, f));
                            continue;
                        }
                        None => None,
                    }
                }
                None => None,
            };
            let Some(f) = hit else { continue };
            if f.needs_full_check() {
                let full = with_sign(i, candidate);
                if !confirms(f, &full) {
                    continue;
                }
            }
            matched.push((i, *candidate, f));
        }
    } else if filters.has_patterns() {
        for &(i, ref candidate) in candidates {
            let i = i as usize;
            if let Some(f) = filters.match_key(candidate) {
                if f.needs_full_check() {
                    let full = with_sign(i, candidate);
                    if !confirms(f, &full) {
                        continue;
                    }
                }
                matched.push((i, *candidate, f));
            }
        }
    } else {
        // A set of nothing but literal prefixes walks exactly the loop it
        // walked before the general forms existed.
        for &(i, ref candidate) in candidates {
            let i = i as usize;
            if let Some(f) = filters.match_prefix(candidate) {
                if f.needs_full_check() {
                    // Only reachable for filters longer than the key-only span,
                    // which is astronomically rare.
                    let full = with_sign(i, candidate);
                    if !key::address(&full).starts_with(f.canonical()) {
                        continue;
                    }
                }
                matched.push((i, *candidate, f));
            }
        }
    }

    for (i, candidate, filter) in matched {
        // A stop must not be held up by a queue of pending writes. A hit that
        // has not been announced yet is simply dropped; one already being
        // written completes, because the writer lock is held for the whole of
        // it.
        if shared.stop.load(Ordering::Relaxed) {
            return false;
        }
        let candidate_offset = delta(i);
        // Clamping can break as the counter carries into the high bits; such a
        // key is not a valid ed25519 key, so the hit is dropped.
        let Some(secret) = key::expanded_secret_at_offset(seed, candidate_offset) else {
            continue;
        };
        let whence = Provenance {
            seed,
            block,
            offset: candidate_offset,
            source,
        };
        if !record_hit(
            opts,
            shared,
            &with_sign(i, &candidate),
            &secret,
            filter,
            &whence,
        ) {
            return false;
        }
    }
    true
}

fn worker_scalar(opts: &RunOptions, filters: &FilterSet, shared: &Shared) {
    let mut engine = PairEngine::new(opts.batch_size);
    let batch_size = engine.batch_size() as u64;
    // Built once: a set decided by the leading bits can be asked about a
    // candidate while it is still four limbs in registers, which is what keeps
    // the canonical encoding off the hot path.
    let probe = filters.leading_probe();

    loop {
        if shared.stop.load(Ordering::Relaxed) || limit_reached(opts, shared) {
            return;
        }
        let block = shared.next_block.fetch_add(1, Ordering::Relaxed);
        if !block_allowed(opts, block) {
            return;
        }
        if let Some(progress) = &opts.progress {
            progress.store(block + 1 - opts.first_block, Ordering::Relaxed);
        }
        let seed = block_seed(&opts.root_seed, block);
        let scalar = key::secret_scalar(&seed);
        let mut acc = curve::scalar_base_mult(&scalar, &Point::basepoint(), &curve::two_d());
        engine.begin(&mut acc);

        for _ in 0..BATCHES_PER_BLOCK {
            if shared.stop.load(Ordering::Relaxed) || limit_reached(opts, shared) {
                return;
            }
            // Matching reads the sign-free packed keys: the sign bit lands in
            // the top bit of the last byte, beyond any filter's reach, so
            // computing it per candidate would be pure waste.
            engine.run(&mut acc, probe.as_ref());
            shared.candidates.fetch_add(batch_size, Ordering::Relaxed);

            if !scan_batch(
                opts,
                filters,
                shared,
                &seed,
                block,
                |i| engine.delta(i),
                engine.survivors(),
                "the processor, scalar path",
                |i, bytes| engine.packed_with_sign(i, bytes),
            ) {
                return;
            }
        }
    }
}

fn worker_vector(opts: &RunOptions, filters: &FilterSet, shared: &Shared) {
    let mut engine = BatchEngine4::new(opts.batch_size);
    let batch_size = engine.batch_size() as u64;
    // The chain engine hands over a plain array; the scan wants each candidate
    // to carry its index, because the paired engine passes only survivors.
    let mut indexed: Vec<(u32, [u8; 32])> = Vec::with_capacity(engine.batch_size());

    loop {
        if shared.stop.load(Ordering::Relaxed) || limit_reached(opts, shared) {
            return;
        }
        let block = shared.next_block.fetch_add(1, Ordering::Relaxed);
        if !block_allowed(opts, block) {
            return;
        }
        if let Some(progress) = &opts.progress {
            progress.store(block + 1 - opts.first_block, Ordering::Relaxed);
        }
        let seed = block_seed(&opts.root_seed, block);
        let scalar = key::secret_scalar(&seed);
        let start = curve::scalar_base_mult(&scalar, &Point::basepoint(), &curve::two_d());
        let mut chains = BatchEngine4::split_chains(&start);
        let mut offset = 0u64;

        for _ in 0..BATCHES_PER_BLOCK {
            if shared.stop.load(Ordering::Relaxed) || limit_reached(opts, shared) {
                return;
            }
            engine.run(&mut chains);
            shared.candidates.fetch_add(batch_size, Ordering::Relaxed);

            indexed.clear();
            indexed.extend(
                engine
                    .packed()
                    .iter()
                    .enumerate()
                    .map(|(i, p)| (i as u32, *p)),
            );
            if !scan_batch(
                opts,
                filters,
                shared,
                &seed,
                block,
                |i| offset + CHAIN_STEP * i as u64,
                &indexed,
                "the processor, vector path",
                |i, _| engine.packed_with_sign(i),
            ) {
                return;
            }
            offset += CHAIN_STEP * batch_size;
        }
    }
}

fn limit_reached(opts: &RunOptions, shared: &Shared) -> bool {
    opts.limit
        .is_some_and(|limit| shared.hits.load(Ordering::Relaxed) >= limit)
}

/// Writes and announces one hit. Returns `false` once the run should stop.
///
/// The lock serialises hits so that the limit counts exactly and so that a hit
/// is fully on disk before it is counted — a run stopping at its limit must
/// leave every announced key complete.
/// Where a candidate came from, carried only so that a failed check can say
/// enough to be acted on.
///
/// Nothing here is read on the happy path. It is three words wide and built per
/// hit, which is a rare event by construction.
pub struct Provenance<'a> {
    /// The block's seed, not the run's root seed: it is what reproduces this
    /// candidate, and it is secret for the same reason the root seed is.
    pub seed: &'a [u8; 32],
    /// Which block of the root seed's space this came from.
    ///
    /// Carried because it is half of the coordinate a master needs: given the
    /// root it handed out, `block` and `offset` reproduce the key exactly, and
    /// nothing else has to cross the wire.
    pub block: u64,
    /// Steps along the chain from that block's start.
    pub offset: u64,
    /// Which path produced it. A defect that shows only on one of them is the
    /// likeliest kind, so the report must not have to guess.
    pub source: &'a str,
}

fn record_hit(
    opts: &RunOptions,
    shared: &Shared,
    public_key: &[u8; 32],
    secret: &[u8; 64],
    filter: &Filter,
    whence: &Provenance<'_>,
) -> bool {
    let mut store = shared.writer.lock().unwrap_or_else(|e| e.into_inner());

    if let Some(limit) = opts.limit {
        if shared.hits.load(Ordering::Relaxed) >= limit {
            return false;
        }
    }

    // A worker's finds belong to its master, and go out as a position rather
    // than a key. Before the check below, deliberately: the master derives the
    // key itself and checks it there, so doing the scalar multiplication twice
    // would only slow a worker down without catching anything the master will
    // not catch.
    if let Some(report) = &opts.report {
        return report(whence.block, whence.offset);
    }

    // Before anything is written down. A key that does not open its address
    // is the one failure this program can have that looks like success, and
    // the moment to catch it is before it becomes a file somebody trusts.
    if !key::secret_produces(secret, public_key) {
        // The writer lock is held for the whole of this function, so the first
        // worker through here is the one that reports.
        if !shared.defect.swap(true, Ordering::Relaxed) {
            report_mismatch(opts, public_key, secret, whence);
        }
        shared.stop.store(true, Ordering::Relaxed);
        return false;
    }

    // Scored here and nowhere earlier: 254 ns against the 6 ms this find is
    // about to spend on a directory and three files, and zero against every
    // candidate that never got this far.
    let hostname = key::hostname(public_key);
    let address = key::address_bytes(public_key);
    let scored = score::of_find(public_key, filter);

    // Turned away before the key is written, which is the only place the
    // threshold saves anything: the write is what costs.
    if opts.min_score.is_some_and(|floor| scored.total < floor) {
        shared.below_score.fetch_add(1, Ordering::Relaxed);
        return true;
    }

    {
        let mut best = shared.best.lock().unwrap_or_else(|e| e.into_inner());
        if best.as_ref().is_none_or(|(had, _)| scored.total > *had) {
            *best = Some((scored.total, hostname.clone()));
        }
    }

    // Asked before the find is taken, so that a repeat is reported rather than
    // counted twice. Only a master handing the same range out again can
    // produce one, but then it produces many.
    match store.holds(&hostname) {
        Ok(true) => {
            eprintln!(
                "onion-gen: already present, skipping: {}",
                store.locate(&hostname)
            );
            return true;
        }
        Ok(false) => {}
        Err(e) => {
            eprintln!("onion-gen: could not read the store: {e}");
            return true;
        }
    }

    let where_it_goes = store.locate(&hostname);
    let find = Find {
        address: hostname.clone(),
        public_key: *public_key,
        secret: *secret,
        filter: filter.source_text().to_string(),
        score: scored.total,
        block: whence.block,
        offset: whence.offset,
        source: whence.source.to_string(),
    };
    if let Err(e) = store.put(find) {
        eprintln!("onion-gen: could not write the key: {e}");
        return true;
    }

    if opts.json {
        let value = serde_json::json!({
            "event": "hit",
            "address": hostname,
            "filter": filter.source_text(),
            "stored_at": where_it_goes,
            "matched_at_symbol": filter.match_offset(&address, true).map(|at| at + 1),
            "score": (scored.total * 10.0).round() / 10.0,
            // What the score means, which the score alone does not
            // say: it is a sort key, and this is the measured share of
            // addresses reaching it.
            "score_one_in": score::one_in(scored.total).round(),
            "score_parts": scored
                .parts
                .iter()
                .map(|(f, b)| serde_json::json!({
                    "feature": f.name(),
                    "bits": (b * 10.0).round() / 10.0,
                }))
                .collect::<Vec<_>>(),
        });
        let mut out = io::stdout().lock();
        let _ = writeln!(out, "{value}");
        let _ = out.flush();
    } else if opts.print_addresses {
        let mut out = io::stdout().lock();
        let _ = writeln!(out, "{hostname}");
        let _ = out.flush();
        // On the diagnostic stream, so that standard output stays a
        // plain list of addresses and `onion-gen | while read` needs
        // no parsing.
        if scored.total > 0.0 {
            eprintln!(
                "onion-gen: score {:.1} ({}), about one address in {}",
                scored.total,
                scored
                    .parts
                    .iter()
                    .map(|(f, b)| format!("{} {:.1}", f.name(), b))
                    .collect::<Vec<_>>()
                    .join(", "),
                human_count(score::one_in(scored.total))
            );
        }
        // Where the match happened, on the diagnostic stream so that
        // stdout stays a plain list of addresses. A form anchored to
        // the start always matches at symbol one, so saying so is
        // noise.
        if !filter.is_anchored_at_start() {
            let address = key::address_bytes(public_key);
            if let Some(at) = filter.match_offset(&address, true) {
                eprintln!(
                    "onion-gen: {} matched at symbol {}",
                    filter.source_text(),
                    at + 1
                );
            }
        }
    }

    let found = shared.hits.fetch_add(1, Ordering::Relaxed) + 1;
    // Keep going unless this hit was the last one asked for.
    !matches!(opts.limit, Some(limit) if found >= limit)
}

/// Writes down whatever the store is holding once it has waited long enough.
///
/// A full batch goes down on the find that fills it, so this is only for the
/// end of a burst: without it the last few finds of a search would sit in
/// memory until the run ended.
fn spawn_flusher(shared: Arc<Shared>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        while !shared.stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(100));
            let mut store = shared.writer.lock().unwrap_or_else(|e| e.into_inner());
            if store.overdue() {
                if let Err(e) = store.flush() {
                    eprintln!("onion-gen: could not write the keys: {e}");
                }
            }
        }
    })
}

/// Prints one line per interval, each describing a complete interval.
///
/// The reference implementation emits its first line 0.1 s after startup, which
/// every measurement then has to discard; here the first line appears only once
/// a full interval has elapsed.
fn spawn_reporter(
    shared: Arc<Shared>,
    interval: Duration,
    started: Instant,
    // Chance that any one candidate is a hit, from the filter set. `None`
    // when the set can never match, where no estimate would mean anything.
    chance: Option<f64>,
    // What earlier stretches of this search already examined, so that the odds
    // are of the whole search and not of this sitting.
    carried: u64,
    json: bool,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let tick = Duration::from_millis(100).min(interval);
        let mut previous = 0u64;
        let mut last = Instant::now();
        loop {
            let deadline = Instant::now() + interval;
            while Instant::now() < deadline {
                if shared.stop.load(Ordering::Relaxed) {
                    return;
                }
                std::thread::sleep(tick);
            }
            if shared.stop.load(Ordering::Relaxed) {
                return;
            }

            let total = shared.candidates.load(Ordering::Relaxed);
            let delta = total - previous;
            previous = total;
            // Over the span that actually passed, not the one asked for. The
            // wait above overshoots by up to a tick and by however long the
            // machine was busy, and dividing by the nominal interval reports a
            // rate the run never reached.
            let now = Instant::now();
            let span = now.duration_since(last).as_secs_f64();
            last = now;
            let rate = if span > 0.0 { delta as f64 / span } else { 0.0 };
            // `calc/sec:` first, and in `mkp224o`'s own shape: a comparison
            // is only a comparison when one parser reads both. Anything added
            // here goes after it.
            let hits = shared.hits.load(Ordering::Relaxed);
            let elapsed = started.elapsed().as_secs_f64();
            let odds_and_half = chance.map(|chance| {
                (
                    1.0 - (-((total + carried) as f64) * chance).exp(),
                    (rate > 0.0).then(|| std::f64::consts::LN_2 / (chance * rate)),
                )
            });
            if json {
                let value = serde_json::json!({
                    "event": "stats",
                    "calc_per_sec": rate,
                    "hits": hits,
                    "elapsed_sec": elapsed,
                    "candidates": total + carried,
                    "device_candidates": shared.device_candidates.load(Ordering::Relaxed),
                    // What `--from-block` takes, so a caller keeping the run's
                    // place itself can checkpoint from this stream instead of
                    // waiting for the run to end.
                    "next_block": shared.next_block.load(Ordering::Relaxed),
                    "found_by_now": odds_and_half.map(|(odds, _)| odds),
                    "half_way_sec": odds_and_half.and_then(|(_, half)| half),
                });
                eprintln!("{value}");
                continue;
            }
            let mut line = format!(">calc/sec:{rate:.6}, hits:{hits}, elapsed:{elapsed:.3}sec");
            if let Some(chance) = chance {
                // Two numbers, and neither of them is a progress bar. The
                // search has no memory: candidates already examined do not
                // bring the next one closer, so "percent done" would be a
                // lie. What is true is how likely a hit would have happened
                // by now, and how long the half-way point is from any moment
                // — which for a memoryless process is the same number all
                // the way through.
                let odds = 1.0 - (-((total + carried) as f64) * chance).exp();
                line.push_str(&format!(", found-by-now:{:.1}%", odds * 100.0));
                if rate > 0.0 {
                    let median = std::f64::consts::LN_2 / (chance * rate);
                    line.push_str(&format!(", half-way:{}", human_duration(median)));
                }
            }
            eprintln!("{line}");
        }
    })
}

#[cfg(unix)]
fn install_signal_handlers(stop: &Arc<AtomicBool>) -> io::Result<()> {
    for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        signal_hook::flag::register(signal, Arc::clone(stop))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn install_signal_handlers(stop: &Arc<AtomicBool>) -> io::Result<()> {
    signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(stop))?;
    Ok(())
}

/// One device, advancing its own chains and reporting what survives.
///
/// The device rules candidates out; it never rules them in. Everything that
/// crosses back is checked here against the whole filter set, so a bitmap that
/// says "maybe" too often costs time and never correctness.
fn device_worker(
    opts: &RunOptions,
    filters: &FilterSet,
    shared: &Shared,
    device: &crate::gpu::Device,
) -> Result<(), String> {
    let Some((bitmap, bits)) = filters.index_words() else {
        return Err("no prefix bitmap, so there is nothing for it to filter on".to_string());
    };

    let block = shared.next_block.fetch_add(1, Ordering::Relaxed);
    if !block_allowed(opts, block) {
        return Ok(());
    }
    if let Some(progress) = &opts.progress {
        progress.store(block + 1 - opts.first_block, Ordering::Relaxed);
    }
    let seed = block_seed(&opts.root_seed, block);
    let scalar = key::secret_scalar(&seed);
    let start = curve::scalar_base_mult(&scalar, &Point::basepoint(), &curve::two_d());

    let mut engine = crate::gpu::Engine::new(
        device,
        &start,
        opts.device_threads,
        bitmap,
        bits,
        opts.retune,
    )?;
    if opts.explain_devices {
        // A device that quietly settled for a smaller batch would otherwise
        // show up only as a throughput nobody can account for.
        eprintln!(
            "onion-gen: device {} runs {} chains at once, {} candidates a launch",
            device.index,
            engine.threads(),
            engine.batch()
        );
    }

    loop {
        if shared.stop.load(Ordering::Relaxed) || limit_reached(opts, shared) {
            return Ok(());
        }
        let survivors = engine.advance()?;
        let examined = engine.batch();
        shared.candidates.fetch_add(examined, Ordering::Relaxed);
        shared
            .device_candidates
            .fetch_add(examined, Ordering::Relaxed);

        for hit in survivors {
            let Some(filter) = filters.match_key(&hit.packed) else {
                // The bitmap said maybe and the exact check said no. Normal.
                continue;
            };
            if shared.stop.load(Ordering::Relaxed) {
                return Ok(());
            }
            let Some(secret) = key::expanded_secret_at_offset(&seed, hit.offset) else {
                continue;
            };
            let mut sc = [0u8; 32];
            sc.copy_from_slice(&secret[..32]);
            let point = curve::scalar_base_mult(&sc, &Point::basepoint(), &curve::two_d());
            let public = key::pack(&point);
            // The device said where, and the key is derived from that position
            // rather than taken from it. The two must agree: a device whose
            // position bookkeeping is wrong would otherwise have its mismatched
            // key written to disk, which is how one kernel's offsets went
            // unnoticed. Sign bits aside, these are the same 32 bytes.
            if public[..31] != hit.packed[..31] || public[31] & 0x7f != hit.packed[31] & 0x7f {
                shared.device_mismatch.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            let whence = Provenance {
                seed: &seed,
                block,
                offset: hit.offset,
                source: &format!("device {} via {}", device.index, device.api.name()),
            };
            if !record_hit(opts, shared, &public, &secret, filter, &whence) {
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// A key that fails its check must not reach the disk, and the run must
    /// end in a way a script can tell from "found nothing".
    #[test]
    fn a_key_that_fails_its_check_is_not_written() {
        let dir = std::env::temp_dir().join(format!("onion-gen-defect-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let seed = [0x21u8; 32];
        let secret = key::expanded_secret_key(&seed);
        // Somebody else's address, so the pair does not belong together.
        let public = key::public_key(&[0x22u8; 32]);
        assert!(
            !key::secret_produces(&secret, &public),
            "the setup is wrong"
        );

        let mut opts = RunOptions::new(dir.clone());
        opts.print_addresses = false;
        let stop = Arc::new(AtomicBool::new(false));
        let shared = Shared {
            next_block: AtomicU64::new(0),
            candidates: AtomicU64::new(0),
            device_candidates: AtomicU64::new(0),
            device_mismatch: AtomicU64::new(0),
            hits: AtomicU64::new(0),
            below_score: AtomicU64::new(0),
            best: Mutex::new(None),
            stop: Arc::clone(&stop),
            defect: AtomicBool::new(false),
            writer: Mutex::new(Store::directory(dir.clone())),
        };
        let filter = crate::filter::Filter::parse("aa").expect("a valid filter");
        let whence = Provenance {
            seed: &seed,
            block: 0,
            offset: 0,
            source: "a test",
        };

        let carry_on = record_hit(&opts, &shared, &public, &secret, &filter, &whence);
        assert!(!carry_on, "the run has to stop");
        assert!(shared.defect.load(Ordering::Relaxed), "and say why");
        assert!(stop.load(Ordering::Relaxed), "and tell the other workers");
        assert_eq!(
            shared.hits.load(Ordering::Relaxed),
            0,
            "a key that does not work is not a hit"
        );

        // The report is there, and no key beside it.
        let written: Vec<String> = std::fs::read_dir(&dir)
            .expect("the directory exists")
            .map(|e| {
                e.expect("readable")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(written.len(), 1, "expected only a report, got {written:?}");
        assert!(
            written[0].starts_with("onion-gen-mismatch-"),
            "the one file is {written:?}"
        );
        let body = std::fs::read_to_string(dir.join(&written[0])).expect("readable");
        for wanted in ["block seed:", "offset:", "secret:", "dalek says:", "a test"] {
            assert!(body.contains(wanted), "the report has no {wanted:?}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn block_seeds_are_distinct_and_stable() {
        let root = [7u8; 32];
        let seeds: HashSet<[u8; 32]> = (0..1000).map(|i| block_seed(&root, i)).collect();
        assert_eq!(seeds.len(), 1000, "block seeds must not collide");
        assert_eq!(
            block_seed(&root, 42),
            block_seed(&root, 42),
            "and be stable"
        );

        let other = [8u8; 32];
        assert_ne!(
            block_seed(&root, 42),
            block_seed(&other, 42),
            "a different run must explore different blocks"
        );
    }

    /// Two workers must never look at the same candidate. Blocks come from one
    /// atomic counter, so this holds by construction; the test guards the
    /// property rather than the implementation.
    #[test]
    fn workers_do_not_share_candidates() {
        let root = [3u8; 32];
        let mut engine = PairEngine::new(16);
        let mut seen: HashSet<[u8; 32]> = HashSet::new();

        for block in 0..8u64 {
            let seed = block_seed(&root, block);
            let scalar = key::secret_scalar(&seed);
            let mut acc = curve::scalar_base_mult(&scalar, &Point::basepoint(), &curve::two_d());
            engine.begin(&mut acc);
            for _ in 0..2 {
                engine.run(&mut acc, None);
                for &(i, ref bytes) in engine.survivors() {
                    assert!(
                        seen.insert(engine.packed_with_sign(i as usize, bytes)),
                        "candidate repeated across blocks"
                    );
                }
            }
        }
        assert_eq!(seen.len(), 8 * 2 * engine.batch_size());
    }

    #[test]
    fn a_run_stops_at_its_limit_and_writes_every_hit() {
        let out = std::env::temp_dir().join(format!("onion-gen-run-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&out);

        let filters = FilterSet::parse_all(["a"]).unwrap();
        let mut opts = RunOptions::new(out.clone());
        opts.threads = 2;
        opts.batch_size = 256;
        opts.limit = Some(4);
        opts.print_addresses = false;
        opts.root_seed = [11u8; 32];

        let summary = run(&opts, &filters).unwrap();
        assert_eq!(summary.hits, 4, "the run must stop exactly at the limit");

        let dirs: Vec<_> = std::fs::read_dir(&out)
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(dirs.len(), 4, "every hit must be on disk");
        for d in dirs {
            let path = d.path();
            for name in ["hs_ed25519_secret_key", "hs_ed25519_public_key", "hostname"] {
                let f = path.join(name);
                assert!(f.exists(), "{} missing", f.display());
                assert!(std::fs::metadata(&f).unwrap().len() > 0, "{name} is empty");
            }
            let hostname = std::fs::read_to_string(path.join("hostname")).unwrap();
            assert!(hostname.starts_with('a'), "hit does not match the filter");
        }

        std::fs::remove_dir_all(&out).unwrap();
    }
}

/// A span of seconds in the largest unit that still reads as a number.
///
/// Used for waiting times, which in this program run from milliseconds to
/// centuries: the point of the estimate is that the reader sees "17 days" and
/// stops, rather than reading 1.5e6 seconds and not stopping.
pub fn human_duration(seconds: f64) -> String {
    const MINUTE: f64 = 60.0;
    const HOUR: f64 = 60.0 * MINUTE;
    const DAY: f64 = 24.0 * HOUR;
    const YEAR: f64 = 365.25 * DAY;
    if !seconds.is_finite() {
        return "never".to_string();
    }
    for (limit, unit, name) in [
        (MINUTE, 1.0, "s"),
        (HOUR, MINUTE, "min"),
        (DAY, HOUR, "h"),
        (YEAR, DAY, "days"),
        (f64::INFINITY, YEAR, "years"),
    ] {
        if seconds < limit {
            let value = seconds / unit;
            // Three significant figures at most: an estimate of a wait is not
            // known to more than that, and printing more invites belief.
            return if value >= 100.0 {
                format!("{value:.0}{name}")
            } else if value >= 10.0 {
                format!("{value:.1}{name}")
            } else {
                format!("{value:.2}{name}")
            };
        }
    }
    unreachable!("the last limit is infinite")
}

/// A count in the form a reader can compare at a glance.
///
/// Three significant figures, for the same reason the durations have three:
/// two symbols of filter are a thousandfold, and that is what the reader is
/// meant to see. More digits would suggest the estimate knows more than it
/// does.
pub fn human_count(n: f64) -> String {
    if !n.is_finite() {
        return "uncountably many".to_string();
    }
    for (limit, unit, name) in [
        (1e3, 1.0, ""),
        (1e6, 1e3, " thousand"),
        (1e9, 1e6, " million"),
        (1e12, 1e9, " billion"),
        (1e15, 1e12, " trillion"),
    ] {
        if n < limit {
            let value = n / unit;
            return if value >= 100.0 || unit == 1.0 {
                format!("{value:.0}{name}")
            } else if value >= 10.0 {
                format!("{value:.1}{name}")
            } else {
                format!("{value:.2}{name}")
            };
        }
    }
    format!("{n:.2e}")
}

#[cfg(test)]
mod estimate_tests {
    use super::*;

    #[test]
    fn durations_read_as_numbers() {
        assert_eq!(human_duration(0.5), "0.50s");
        assert_eq!(human_duration(90.0), "1.50min");
        assert_eq!(human_duration(23663.0), "6.57h");
        assert_eq!(human_duration(1.17e6), "13.5days");
        assert_eq!(human_duration(3.75e7), "1.19years");
        assert_eq!(human_duration(f64::INFINITY), "never");
    }

    #[test]
    fn counts_keep_three_figures() {
        assert_eq!(human_count(950.0), "950");
        assert_eq!(human_count(1.0995e12), "1.10 trillion");
        assert_eq!(human_count(3.518e13), "35.2 trillion");
        // Past what the units cover, plain scientific notation rather than a
        // word nobody agrees on.
        assert_eq!(human_count(1.126e15), "1.13e15");
    }

    #[test]
    fn a_device_is_left_a_core() {
        let cores = default_threads();
        assert_eq!(default_threads_with(0), cores);
        if cores > 1 {
            assert_eq!(default_threads_with(1), cores - 1);
        }
        // Never nothing: a processor that adds seven per cent is worth having,
        // and on a single-core machine the alternative would be no worker at
        // all.
        assert_eq!(default_threads_with(cores + 5), 1);
    }

    /// The whole point of printing the estimate: the reader has to be able to
    /// see that two symbols are not a small change.
    #[test]
    fn two_symbols_are_visible_in_the_text() {
        let eight = human_duration(1.0995e12 * std::f64::consts::LN_2 / 1e9);
        let ten = human_duration(1.126e15 * std::f64::consts::LN_2 / 1e9);
        assert_eq!(eight, "12.7min");
        assert_eq!(ten, "9.03days");
    }
}

/// What a run has to remember to be able to continue.
///
/// The search space is cut into blocks, each seeded from the root seed and a
/// counter, so where a run has got to is exactly those two numbers. Everything
/// else here is for the reader and for refusing to continue the wrong search.
///
/// **This file is as secret as a key.** The root seed derives every key the run
/// will ever produce, so whoever holds it holds them all. It is written with
/// the same permissions as a secret key, and the run says so the first time.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RunState {
    /// The format, so that a later version can refuse an older file rather
    /// than misread it.
    pub version: u32,
    /// Hex, because a state file is meant to be readable by the person who has
    /// to decide whether to trust it.
    pub root_seed: String,
    /// The first block no worker has claimed.
    pub next_block: u64,
    pub hits: u64,
    pub candidates: u64,
    /// The filters this run is for, as written. Continuing a different search
    /// in the same file would silently mix two searches into one count.
    pub filters: Vec<String>,
}

/// The only version this build writes or accepts.
const STATE_VERSION: u32 = 1;

impl RunState {
    fn of(opts: &RunOptions, filters: &FilterSet, shared: &Shared) -> RunState {
        RunState {
            version: STATE_VERSION,
            root_seed: opts.root_seed.iter().map(|b| format!("{b:02x}")).collect(),
            next_block: shared.next_block.load(Ordering::Relaxed),
            hits: opts.carried_hits + shared.hits.load(Ordering::Relaxed),
            candidates: opts.carried_candidates + shared.candidates.load(Ordering::Relaxed),
            filters: filters
                .iter()
                .map(|f| f.source_text().to_string())
                .collect(),
        }
    }

    pub fn seed(&self) -> Option<[u8; 32]> {
        let bytes: Vec<u8> = (0..self.root_seed.len() / 2)
            .map(|i| u8::from_str_radix(&self.root_seed[i * 2..i * 2 + 2], 16))
            .collect::<Result<_, _>>()
            .ok()?;
        bytes.try_into().ok()
    }

    /// Reads a state file, or `None` when there is none to read.
    pub fn load(path: &PathBuf) -> io::Result<Option<RunState>> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let state: RunState = serde_yaml_ng::from_str(&text)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        if state.version != STATE_VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "the state file is version {}, this build writes {STATE_VERSION}",
                    state.version
                ),
            ));
        }
        if state.seed().is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the state file has no readable root seed",
            ));
        }
        Ok(Some(state))
    }

    /// Whether this state belongs to the same search as `filters`.
    pub fn matches(&self, filters: &FilterSet) -> bool {
        let mut mine: Vec<&str> = self.filters.iter().map(String::as_str).collect();
        let mut theirs: Vec<&str> = filters.iter().map(|f| f.source_text()).collect();
        mine.sort_unstable();
        theirs.sort_unstable();
        mine == theirs
    }

    /// Writes the file, replacing it in one step so that a crash mid-write
    /// cannot leave a half-file where a resume would read it.
    fn save(&self, path: &PathBuf) -> io::Result<()> {
        let text = serde_yaml_ng::to_string(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        let temp = path.with_extension("tmp");
        std::fs::write(&temp, text)?;
        restrict(&temp)?;
        std::fs::rename(&temp, path)
    }
}

/// Owner-only, the same as a secret key: this file derives every key of the run.
#[cfg(unix)]
fn restrict(path: &PathBuf) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn restrict(_path: &PathBuf) -> io::Result<()> {
    Ok(())
}

/// How often the run writes down where it has got to.
///
/// By the clock and not by the block: blocks are claimed thousands of times a
/// second on a device, and writing that often would put a file system in the
/// hot loop. Ten seconds costs nothing and loses at most ten seconds of work,
/// against a search measured in days.
const STATE_INTERVAL: Duration = Duration::from_secs(10);

/// The thread takes the seed and the filter texts by value rather than holding
/// the options and the filter set: it outlives neither, but borrowing them
/// would tie it to this call's lifetime for no gain.
fn spawn_state_keeper(
    shared: Arc<Shared>,
    opts: &RunOptions,
    filters: &FilterSet,
    path: PathBuf,
) -> std::thread::JoinHandle<()> {
    let root_seed = opts.root_seed;
    let carried_hits = opts.carried_hits;
    let carried_candidates = opts.carried_candidates;
    let specs: Vec<String> = filters
        .iter()
        .map(|f| f.source_text().to_string())
        .collect();
    std::thread::spawn(move || {
        // A second a wake, not five: on a machine whose every core is running
        // a worker, a timer thread that wakes often is a timer thread that
        // takes a core away, and this one measured a percent at 200 ms. A
        // second is still prompt enough to stop with everyone else.
        let tick = Duration::from_secs(1);
        loop {
            let deadline = Instant::now() + STATE_INTERVAL;
            while Instant::now() < deadline {
                if shared.stop.load(Ordering::Relaxed) {
                    return;
                }
                std::thread::sleep(tick);
            }
            let state = RunState {
                version: STATE_VERSION,
                root_seed: root_seed.iter().map(|b| format!("{b:02x}")).collect(),
                next_block: shared.next_block.load(Ordering::Relaxed),
                hits: carried_hits + shared.hits.load(Ordering::Relaxed),
                candidates: carried_candidates + shared.candidates.load(Ordering::Relaxed),
                filters: specs.clone(),
            };
            if let Err(e) = state.save(&path) {
                eprintln!("onion-gen: could not write the state file: {e}");
                return;
            }
        }
    })
}

/// Says what went wrong in enough detail to be acted on, and stops short of
/// putting the secret part on a terminal.
///
/// The two public keys go to the screen: they are public by definition, and
/// "we claimed this, an unrelated implementation says that" is the whole
/// diagnosis in one line. Everything needed to reproduce the candidate — the
/// block's seed, the offset, the secret itself — goes to a file with a secret
/// key's permissions, because a block seed derives every key of its block,
/// including the ones that were fine.
fn report_mismatch(
    opts: &RunOptions,
    public_key: &[u8; 32],
    secret: &[u8; 64],
    whence: &Provenance<'_>,
) {
    let hex = |bytes: &[u8]| -> String { bytes.iter().map(|b| format!("{b:02x}")).collect() };
    let derived = key::public_key_from_secret(secret);

    eprintln!(
        "onion-gen: a found key does not open its own address. The run is \
         stopping: this is a defect in the arithmetic, not in what you asked \
         for, and going on would write keys that do not work."
    );
    eprintln!("  produced by:   {}", whence.source);
    eprintln!("  arithmetic:    {}", crate::field4::describe());
    eprintln!(
        "  we computed:   {} ({})",
        hex(public_key),
        key::hostname(public_key)
    );
    eprintln!(
        "  dalek says:    {} ({})",
        hex(&derived),
        key::hostname(&derived)
    );
    let differing: Vec<usize> = (0..32).filter(|&i| public_key[i] != derived[i]).collect();
    eprintln!(
        "  they differ in {} of 32 bytes, at {:?}",
        differing.len(),
        differing
    );

    let path = opts
        .out_dir
        .join(format!("onion-gen-mismatch-{}.txt", std::process::id()));
    let body = format!(
        "onion-gen {} could not verify a key it found.\n\
         \n\
         This file is as secret as a key: the block seed below derives every\n\
         candidate of its block, including the ones that were correct.\n\
         \n\
         version:      {}\n\
         produced by:  {}\n\
         arithmetic:   {}\n\
         target:       {}\n\
         \n\
         we computed:  {}\n\
         dalek says:   {}\n\
         differing:    {:?}\n\
         \n\
         To reproduce, the candidate is the block seed advanced by the offset:\n\
         block seed:   {}\n\
         offset:       {}\n\
         secret:       {}\n",
        env!("CARGO_PKG_VERSION"),
        env!("CARGO_PKG_VERSION"),
        whence.source,
        crate::field4::describe(),
        std::env::consts::ARCH,
        hex(public_key),
        hex(&derived),
        differing,
        hex(whence.seed),
        whence.offset,
        hex(secret),
    );
    match write_secret_file(&path, &body) {
        Ok(()) => eprintln!(
            "  what reproduces it, including the seed, is in {} — treat that \
             file as a secret key",
            path.display()
        ),
        Err(e) => eprintln!(
            "  and the report could not be written to {}: {e}",
            path.display()
        ),
    }
}

/// Writes owner-only, the same as a key.
fn write_secret_file(path: &PathBuf, body: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, body)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}
