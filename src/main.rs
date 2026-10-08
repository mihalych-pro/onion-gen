//! CLI entry point.
//!
//! Everything here happens before the search starts: resolve the configuration,
//! validate it, load the filters, then hand an immutable set of options to the
//! engine. Anything that could fail is made to fail now rather than an hour
//! into a run.

use clap::Parser;
use onion_gen::config::{self, Cli, Config};
use onion_gen::filter::FilterSet;
use onion_gen::output;
use onion_gen::run::{self, RunOptions};
use std::process::ExitCode;

fn main() -> ExitCode {
    let cli = Cli::parse();

    let config = match config::resolve(&cli, |name| std::env::var(name).ok()) {
        Ok(c) => c,
        Err(e) => return fail(&e.to_string()),
    };

    // Printing the effective configuration is what makes a run reproducible, so
    // it must work before validation: it is also how one debugs a rejected one.
    if cli.print_config {
        print!("{}", config.to_yaml());
        return ExitCode::SUCCESS;
    }

    // The role decides what is validated: a worker's filters arrive from the
    // master, so demanding them here would make every node in a fleet carry a
    // copy of what the master exists to distribute.
    // Checking keys needs no filters at all; a worker's arrive from its
    // master, so demanding them here would make every node carry a copy of
    // what the master exists to distribute.
    if let Some(config::Role::Verify(args)) = &cli.role {
        let path = args.path.clone().unwrap_or_else(|| config.out_dir.clone());
        return run_verify(&path, config.json);
    }

    let needs_filters = !matches!(
        cli.role,
        Some(config::Role::Worker(_)) | Some(config::Role::Verify(_))
    );
    if let Err(e) = config.validate_for(needs_filters) {
        return fail(&e.to_string());
    }

    if let Some(config::Role::Master(args)) = &cli.role {
        return run_master(args, &config);
    }

    let worker_args = match &cli.role {
        Some(config::Role::Worker(args)) => Some(args),
        _ => None,
    };

    // A worker's set arrives from the master, so there is nothing to load and
    // nothing to complain about: an empty set here is the normal case for it.
    let (filters, _filter_text) =
        if worker_args.is_some() && config.filters.is_empty() && config.filter_files.is_empty() {
            (
                FilterSet::parse_all(Vec::<String>::new()).expect("an empty set"),
                Vec::new(),
            )
        } else {
            match load_filters(&config) {
                Ok(f) => f,
                Err(message) => return fail(&message),
            }
        };

    // Answered here rather than further down: it needs the filters and nothing
    // else, and a device that is about to be opened would only slow it.
    if let Some(config::Role::Guess(args)) = &cli.role {
        return run_guess(args, &config, &filters);
    }

    if !output::enforces_permissions() {
        eprintln!(
            "onion-gen: warning: this platform does not apply POSIX permissions; \
             the secret key will not be restricted to its owner"
        );
    }

    // Prose diagnostics and machine-readable output are two answers to the
    // same question, and mixing them defeats the second: a caller parsing
    // lines would have to skip the ones that are not JSON.
    // A worker has no filter set of its own yet — the master sends it after
    // registration — so the usual pre-run diagnostics would describe an empty
    // set and announce that no address can satisfy it. Its own line, printed
    // once the set has arrived, says something true instead.
    let prose = !cli.quiet_diagnostics && !config.json && worker_args.is_none();

    // Decided once: asking twice would explain itself twice.
    let devices = choose_devices(&config, &filters, prose);

    // Asked for, or decided. Zero asked for means "let the devices do it",
    // which needs a device; not asked for means the machine is shared with
    // whatever devices are working, and each of them is left a core.
    let threads = if config.threads_explicit {
        if config.threads == 0 && devices.is_empty() {
            eprintln!(
                "onion-gen: zero threads and no device leaves nobody to search; \
                 give --threads at least 1, or choose a device"
            );
            std::process::exit(2);
        }
        config.threads
    } else {
        onion_gen::run::default_threads_with(devices.len())
    };

    if prose {
        eprintln!(
            "onion-gen: field arithmetic: {}",
            onion_gen::field4::describe()
        );
        // What was found and what was chosen, both, because a device that is
        // present and unused is a different problem from one that is absent.
        eprintln!(
            "onion-gen: {}",
            onion_gen::gpu::describe(
                onion_gen::gpu::Choice::parse(&config.compute)
                    .map_or(onion_gen::gpu::Want::Any, |c| c.want)
            )
        );
        match devices.as_slice() {
            [] => eprintln!("onion-gen: candidates come from the processor only"),
            picked => eprintln!(
                "onion-gen: candidates come from the processor and {}",
                picked
                    .iter()
                    .map(onion_gen::gpu::Device::to_string)
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
        }
        // What the run is actually going to cost. Two symbols of filter are
        // three orders of magnitude of waiting, and nothing else on screen
        // says so.
        // An expression's share of the address space is not computable from
        // its text, so a set holding one yields a bound rather than a figure.
        // The distinction matters: "cannot be estimated" and "can never
        // match" are opposite pieces of news, and the second one told about
        // the first would send a user to look for a mistake that is not there.
        match (filters.expected_candidates(), filters.has_unknown_odds()) {
            (Some(expected), false) => eprintln!(
                "onion-gen: expect one hit in about {} candidates",
                onion_gen::run::human_count(expected)
            ),
            (Some(expected), true) => eprintln!(
                "onion-gen: expect one hit within about {} candidates: \
                 {} regular expression(s) are not counted, so the real wait is shorter",
                onion_gen::run::human_count(expected),
                filters.regex_count()
            ),
            (None, true) => eprintln!(
                "onion-gen: the wait cannot be estimated: every filter is a regular \
                 expression, and what share of addresses one matches does not follow \
                 from its text"
            ),
            (None, false) => eprintln!(
                "onion-gen: warning: no address can satisfy these filters, so the \
                 search will not end"
            ),
        }
        eprintln!(
            "onion-gen: {} filter(s), {} thread(s), batch {}, index {}",
            filters.len(),
            threads,
            if config.batch_size_explicit {
                config.batch_size.to_string()
            } else {
                // The run measures it and says which it took; promising a
                // number here that the next line contradicts helps nobody.
                "measured".to_string()
            },
            filters
                .index_memory()
                .map(|b| format!("{} KiB", b / 1024))
                .unwrap_or_else(|| "disabled".into())
        );
        for line in filters.describe_structures() {
            eprintln!("onion-gen: {line}");
        }
        if !filters.needs_checksum() {
            // Said explicitly, because its absence is the good case and the
            // user has no other way to tell which one they are in.
            eprintln!(
                "onion-gen: every filter is decided within the first {} symbols; \
                 no checksum is computed",
                onion_gen::filter::KEY_ONLY_SYMBOLS
            );
        } else {
            // The cost is worth saying out loud before a search starts: it is
            // the difference between a run that ends and one that does not.
            eprintln!(
                "onion-gen: {} filter(s) reach past symbol {} and need the address \
                 checksum: a candidate the key cannot rule out costs a SHA3-256, \
                 which measured about 5x throughput on this engine",
                filters.checksum_filter_count(),
                onion_gen::filter::KEY_ONLY_SYMBOLS,
            );
        }
    }

    if config.json {
        // Everything the prose block would have said, in the shape the rest of
        // the stream uses, so that a caller reads one kind of line throughout.
        let value = serde_json::json!({
            "event": "start",
            "arithmetic": onion_gen::field4::describe(),
            "devices": devices
                .iter()
                .map(|d| serde_json::json!({
                    "index": d.index,
                    "name": d.name,
                    "api": d.api.name(),
                }))
                .collect::<Vec<_>>(),
            "filters": filters.len(),
            "threads": threads,
            "batch_size": config.batch_size,
            "structures": filters.describe_structures(),
            "needs_checksum": filters.needs_checksum(),
            "expected_candidates": filters.expected_candidates(),
            "expected_candidates_is_bound": filters.has_unknown_odds(),
            "regexes": filters.regex_count(),
            "out_dir": config.out_dir.to_string_lossy(),
            "db": config.db.clone(),
        });
        eprintln!("{value}");
    }

    // Before the search, and only for a single run: a fleet's rate belongs to
    // the fleet, and a worker measuring itself would describe one node of it.
    if config.evaluate && worker_args.is_none() {
        print_evaluation(&config, threads);
    }

    let mut options = RunOptions::new(config.out_dir.clone());
    options.db = config.db.clone();
    options.retune = config.retune;
    options.threads = threads;
    // Zero means "measure this machine and pick one". The run does it, rather
    // than this function, because it has to happen after the signal handlers
    // are installed: a measurement is a second of work, and a Ctrl-C during it
    // would otherwise kill the process instead of stopping it.
    options.batch_size = if config.batch_size_explicit {
        config.batch_size
    } else {
        0
    };
    options.limit = config.limit();
    options.stats_interval = config.stats_interval();
    options.print_addresses = config.print_addresses;
    options.force_scalar = config.force_scalar();
    options.force_vector = config.force_vector();
    // Which devices to use, decided once and reported, so that a run on the
    // processor alone is a stated choice rather than a silent fallback.
    options.devices = devices;
    options.device_threads = config.device_threads;
    options.min_score = config.min_score;
    options.explain_devices = prose;
    options.json = config.json;
    options.chance_per_candidate = {
        let p = filters.probability();
        (p > 0.0).then_some(p)
    };
    // A named seed, if there is one. Parsed before anything else it could
    // contradict, so that a typo is an error rather than a run in the wrong
    // part of the space.
    let named_seed = match &config.seed {
        Some(text) => match parse_seed(text) {
            Ok(seed) => Some(seed),
            Err(why) => {
                eprintln!("onion-gen: --seed {why}");
                std::process::exit(2);
            }
        },
        None => None,
    };
    if let Some(seed) = named_seed {
        options.root_seed = seed;
    }
    if config.from_block > 0 && named_seed.is_none() && config.state_file.is_none() {
        eprintln!(
            "onion-gen: --from-block needs a seed to count from; give --seed as \
             well, or --state to keep the place in a file"
        );
        std::process::exit(2);
    }
    options.first_block = config.from_block;

    if let Some(path) = &config.state_file {
        // Reading it here rather than inside the run so that a mismatch is an
        // error before any work, and so that the seed of a resumed run is the
        // saved one rather than a fresh random.
        match onion_gen::run::RunState::load(path) {
            Ok(Some(state)) => {
                if !state.matches(&filters) {
                    eprintln!(
                        "onion-gen: {} holds a different search: it was left with \
                         filter(s) {}. Continuing would mix two searches into one \
                         count; use another file, or delete this one to start over.",
                        path.display(),
                        state.filters.join(", ")
                    );
                    std::process::exit(2);
                }
                let from_file = state.seed().expect("checked while loading");
                if named_seed.is_some_and(|named| named != from_file) {
                    eprintln!(
                        "onion-gen: --seed and {} name different searches; one of \
                         them has to go",
                        path.display()
                    );
                    std::process::exit(2);
                }
                options.root_seed = from_file;
                options.carried_candidates = state.candidates;
                options.carried_hits = state.hits;
                if prose {
                    eprintln!(
                        "onion-gen: continuing from {}: {} candidate(s) and {} hit(s) \
                         already behind it",
                        path.display(),
                        onion_gen::run::human_count(state.candidates as f64),
                        state.hits
                    );
                }
            }
            Ok(None) => {
                if prose {
                    // Said once, and said plainly: the file is the run's keys.
                    eprintln!(
                        "onion-gen: keeping the run's place in {}; that file holds \
                         the seed every key of this run comes from, so it is as \
                         secret as a key",
                        path.display()
                    );
                }
            }
            Err(e) => {
                eprintln!("onion-gen: cannot read {}: {e}", path.display());
                std::process::exit(2);
            }
        }
        options.state_path = Some(path.clone());
    }

    if let Some(args) = worker_args {
        let settings = onion_gen::worker::run::Settings {
            master: args.master.clone(),
            name: args.name.clone().unwrap_or_else(host_name),
            endpoint: args.endpoint.clone(),
            // Beside the output directory, because that is the one place a
            // deployment already has to think about keeping.
            buffer_dir: config.out_dir.join(".unsent"),
            buffer_limit: args.buffer_limit,
        };
        return match onion_gen::worker::run::run(&settings, &options) {
            Ok(()) => ExitCode::from(4),
            Err(e) => fail(&format!("the worker stopped: {e}")),
        };
    }

    match run::run(&options, &filters) {
        Ok(summary) => {
            if summary.defect {
                // The message and the report are already out; this is the exit
                // code, which a script is the only thing that reads.
                return ExitCode::from(3);
            }
            if options.json {
                // The last line, and the one a caller waits for: a run that
                // ended says so in the same shape as everything before it.
                let value = serde_json::json!({
                    "event": "summary",
                    "defect": summary.defect,
                    "hits": summary.hits,
                    "candidates": summary.candidates,
                    "device_candidates": summary.device_candidates,
                    // The same number `--from-block` takes.
                    "next_block": summary.blocks,
                    "below_score": summary.below_score,
                    "best_address": summary.best.as_ref().map(|(_, a)| a),
                    "best_score": summary
                        .best
                        .as_ref()
                        .map(|(s, _)| (s * 10.0).round() / 10.0),
                });
                eprintln!("{value}");
            } else if prose {
                eprintln!(
                    "onion-gen: {} hit(s) from {} candidates ({} on the device)",
                    summary.hits, summary.candidates, summary.device_candidates
                );
                if summary.device_mismatch > 0 {
                    eprintln!(
                        "onion-gen: {} device find(s) did not derive to the key the device \
                         reported and were refused; this is a defect in that kernel",
                        summary.device_mismatch
                    );
                }
                if summary.below_score > 0 {
                    eprintln!(
                        "onion-gen: {} find(s) scored below the threshold and were not written",
                        summary.below_score
                    );
                }
                // Said at the end because the output is never reordered: finds
                // go out as they arrive, and the best one can be anywhere in a
                // long list.
                if let Some((score, address)) = &summary.best {
                    eprintln!(
                        "onion-gen: best find: {address} at {score:.1}, about one address in {}",
                        onion_gen::run::human_count(onion_gen::score::one_in(*score))
                    );
                }
                if config.seed.is_some() && config.state_file.is_none() {
                    // The caller brought their own seed, so they are keeping
                    // the place themselves; this is the other half of it.
                    eprintln!(
                        "onion-gen: to continue this search, add --from-block {}",
                        summary.blocks
                    );
                }
            }
            if summary.reached_goal {
                ExitCode::SUCCESS
            } else {
                // Stopped before the goal. A distinct code because an
                // orchestrator has nothing else to go on: with zero here, a
                // pod evicted during a node drain counts as a success, a Job
                // wanting one completion is satisfied, and the whole fleet is
                // torn down without a key found. Nothing reports an error —
                // the job simply "completed".
                ExitCode::from(4)
            }
        }
        Err(e) => fail(&format!("the run failed: {e}")),
    }
}

/// Collects filters from both sources. Giving both is allowed: a dictionary
/// file plus a couple of names typed on the command line is a natural thing to
/// want.
/// The filter set, and the same filters as written.
///
/// Both, because the master hands the text on to its workers and a parsed
/// filter cannot be turned back into it: a class prints as the lowest symbol it
/// accepts, so a round trip through the parser turns `a[b-e]d` into `abd`.
fn load_filters(config: &Config) -> Result<(FilterSet, Vec<String>), String> {
    let mut collected: Vec<String> = config.filters.clone();

    for path in &config.filter_files {
        // The lines as written. Parsing them here and handing on the parsed
        // filters' text would not round-trip: a class prints as the lowest
        // symbol it accepts, so `-f` quietly turned `a[b-e]d` into `abd` and
        // every `contains:` into a prefix.
        let from_file = onion_gen::filter::read_filter_lines(path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        collected.extend(from_file);
    }

    let parsed = FilterSet::parse_all(&collected)
        .map_err(|(raw, e)| format!("filter {raw:?} was rejected: {e}"))?;

    if parsed.is_empty() {
        return Err("no usable filters were given".to_string());
    }

    // Rebuilding with the configured width. Zero means the width is chosen
    // from the number of filters, which is what the measurements say it should
    // depend on.
    let set = FilterSet::with_index_bits(parsed.into_filters(), config.index_bits);
    Ok((set, collected))
}

fn fail(message: &str) -> ExitCode {
    eprintln!("onion-gen: {message}");
    ExitCode::FAILURE
}

/// Which devices this run will use.
///
/// `cpu` means none whatever is present; `gpu` means the ones asked for, and
/// failing to find them is worth saying out loud rather than quietly falling
/// back; `auto` takes what is there.
fn choose_devices(
    config: &Config,
    filters: &FilterSet,
    explain: bool,
) -> Vec<onion_gen::gpu::Device> {
    use onion_gen::gpu::{Availability, Choice};

    if config.compute == "cpu" {
        return Vec::new();
    }
    let Some(choice) = Choice::parse(&config.compute) else {
        // Caught here rather than at parsing because this is where the list of
        // what is actually reachable lives.
        eprintln!(
            "onion-gen: --compute {} is not one of {}",
            config.compute,
            onion_gen::gpu::COMPUTE_VALUES.join(", ")
        );
        return Vec::new();
    };
    // A device rules candidates out with the prefix bitmap and nothing else.
    // Without one every candidate would have to cross back to be judged, which
    // the link cannot carry — so the device is not used, and saying so at the
    // start beats a path that quietly stops.
    if filters.index_words().is_none() {
        if explain && choice.required {
            eprintln!(
                "onion-gen: these filters build no prefix bitmap, which is the only \
                 structure a device can be asked; the processor will do the work"
            );
        }
        return Vec::new();
    }
    let found = match onion_gen::gpu::look(choice.want) {
        Availability::Devices(d) => d,
        Availability::NoDriver(why) => {
            if explain && choice.required {
                eprintln!("onion-gen: no device was found: {why}");
            }
            return Vec::new();
        }
        Availability::NoDevice => {
            if explain && choice.required {
                eprintln!("onion-gen: no device was found, so there is nothing to force");
            }
            return Vec::new();
        }
    };
    if config.devices.is_empty() {
        return found;
    }
    let mut picked = Vec::new();
    for want in &config.devices {
        if let Some(d) = found.iter().find(|d| d.index == *want) {
            picked.push(d.clone());
        } else if explain {
            eprintln!("onion-gen: device {want} was asked for and not found");
        }
    }
    picked
}

/// The 32 bytes of a seed, from the 64 hex characters a caller writes.
fn parse_seed(text: &str) -> Result<[u8; 32], String> {
    let trimmed = text.trim();
    if trimmed.len() != 64 {
        return Err(format!(
            "is {} characters, and a seed is 64 hex ones",
            trimmed.len()
        ));
    }
    let mut out = [0u8; 32];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&trimmed[i * 2..i * 2 + 2], 16)
            .map_err(|_| format!("has something that is not hex at character {}", i * 2 + 1))?;
    }
    Ok(out)
}

/// `verify`: check keys and say what they are.
///
/// Exits non-zero when any key fails, so that a script can act on it without
/// reading the text.
fn run_verify(path: &std::path::Path, json: bool) -> ExitCode {
    let reports = match onion_gen::verify::tree(path) {
        Ok(r) => r,
        Err(e) => return fail(&format!("{}: {e}", path.display())),
    };
    if reports.is_empty() {
        eprintln!("onion-gen: no keys under {}", path.display());
        return ExitCode::SUCCESS;
    }
    let bad = reports.iter().filter(|r| !r.ok()).count();
    if json {
        let mut out = std::io::stdout().lock();
        for r in &reports {
            let value = serde_json::json!({
                "event": "key",
                "address": r.address,
                "path": r.path.to_string_lossy(),
                "ok": r.ok(),
                "fault": r.fault,
                "checks": r.passed,
                "score": (r.score * 10.0).round() / 10.0,
                "score_one_in": r.one_in.round(),
                "owner_only": r.private,
                "has_public_key_file": r.public_key_file,
                "has_hostname_file": r.hostname_file,
            });
            let _ = std::io::Write::write_all(&mut out, format!("{value}\n").as_bytes());
        }
        let _ = std::io::Write::flush(&mut out);
    } else {
        for r in &reports {
            print!("{r}");
        }
        println!("{} key(s) checked, {} failed", reports.len(), bad);
    }
    if bad > 0 {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

/// `guess`: how long this filter set would take here, without searching.
fn run_guess(args: &config::GuessArgs, config: &config::Config, filters: &FilterSet) -> ExitCode {
    let window = std::time::Duration::from_secs_f64(args.measure_seconds.clamp(0.2, 60.0));
    let threads = if config.threads == 0 {
        onion_gen::run::default_threads()
    } else {
        config.threads
    };
    if !config.json {
        eprintln!(
            "onion-gen: measuring this machine for {:.1}s",
            window.as_secs_f64()
        );
    }
    let rate = onion_gen::estimate::measure_rate(threads, config.batch_size, window);
    let estimate = onion_gen::estimate::for_filters(filters, rate);

    if config.json {
        let value = serde_json::json!({
            "event": "guess",
            "threads": threads,
            "calc_per_sec": rate,
            "filters": filters.len(),
            "chance_per_candidate": filters.probability(),
            "unknown_odds": filters.has_unknown_odds(),
            "median_seconds": estimate.map(|e| e.median_seconds),
            "ninety_percent_seconds": estimate.map(|e| e.ninety_seconds),
            "candidates": estimate.map(|e| e.candidates),
        });
        println!("{value}");
        return ExitCode::SUCCESS;
    }

    println!("{:.1} M candidates/s on {threads} thread(s)", rate / 1e6);
    match estimate {
        Some(e) => {
            println!(
                "one address in {} matches",
                onion_gen::run::human_count(1.0 / filters.probability())
            );
            println!(
                "half of searches finish within {}",
                onion_gen::estimate::human_time(e.median_seconds)
            );
            println!(
                "nine in ten within {}",
                onion_gen::estimate::human_time(e.ninety_seconds)
            );
        }
        None => println!(
            "the odds of this set cannot be counted - it holds a regular expression, \
             and how often one matches is not something to work out without searching"
        ),
    }
    ExitCode::SUCCESS
}

/// `--evaluate`: what each prefix length would cost on this machine.
fn print_evaluation(config: &config::Config, threads: usize) {
    let rate = onion_gen::estimate::measure_rate(
        threads,
        config.batch_size,
        std::time::Duration::from_secs(2),
    );
    if config.json {
        let rows: Vec<_> = onion_gen::estimate::by_length(rate)
            .into_iter()
            .map(|(n, e)| {
                serde_json::json!({
                    "symbols": n,
                    "median_seconds": e.median_seconds,
                    "ninety_percent_seconds": e.ninety_seconds,
                    "candidates": e.candidates,
                })
            })
            .collect();
        let value = serde_json::json!({
            "event": "evaluation",
            "threads": threads,
            "calc_per_sec": rate,
            "by_length": rows,
        });
        eprintln!("{value}");
        return;
    }
    eprintln!(
        "onion-gen: this machine does {:.1} M candidates/s on {threads} thread(s)",
        rate / 1e6
    );
    eprintln!("onion-gen: symbols   half of searches   nine in ten");
    for (n, e) in onion_gen::estimate::by_length(rate) {
        eprintln!(
            "onion-gen: {n:>7}   {:>16}   {}",
            onion_gen::estimate::human_time(e.median_seconds),
            onion_gen::estimate::human_time(e.ninety_seconds)
        );
    }
}

/// Runs the master: no searching of its own, only coordination.
fn run_master(args: &config::MasterArgs, config: &config::Config) -> ExitCode {
    let addr: std::net::SocketAddr = match args.listen.parse() {
        Ok(addr) => addr,
        Err(e) => return fail(&format!("--listen {}: {e}", args.listen)),
    };

    let (filters, filter_text) = match load_filters(config) {
        Ok(pair) => pair,
        Err(message) => return fail(&message),
    };
    if let Err(message) = check_worth_distributing(&filters, config.db.is_some()) {
        return fail(&message);
    }

    // The seed every key of this fleet comes from. Named for a reproducible
    // run, random otherwise — the same rule a single run follows.
    let root = match &config.seed {
        Some(text) => match parse_seed(text) {
            Ok(seed) => seed,
            Err(why) => return fail(&why),
        },
        None => {
            let mut seed = [0u8; 32];
            rand::Rng::fill_bytes(&mut rand::rng(), &mut seed);
            seed
        }
    };

    let store = match &config.db {
        Some(url) => match onion_gen::store::open(url) {
            Ok(store) => store,
            Err(e) => return fail(&format!("could not open the store: {e}")),
        },
        None => onion_gen::store::Store::directory(args.store.clone()),
    };

    eprintln!(
        "onion-gen: master, {} filter(s), leases of {}s sized to {}s of work, store {}",
        filter_text.len(),
        args.lease_seconds,
        args.lease_work_seconds,
        // Never the raw connection string: a password in a startup line is a
        // password in a log file.
        store.describe()
    );
    if config.seed.is_none() {
        // Said out loud because it is the difference between a fleet that can
        // be resumed and one that cannot.
        eprintln!(
            "onion-gen: the root seed is random; pass --seed to make this fleet's \
             search reproducible"
        );
    }

    let master = std::sync::Arc::new(std::sync::Mutex::new(onion_gen::master::Master::new(
        root,
        config.from_block,
        filters,
        filter_text,
        store,
        std::time::Duration::from_secs(args.lease_seconds),
        args.lease_work_seconds,
    )));

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => return fail(&format!("could not start the runtime: {e}")),
    };
    match runtime.block_on(onion_gen::master::serve(addr, master)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(&format!("the master stopped: {e}")),
    }
}

/// What to call this worker when it has not been told.
///
/// The host name, because that is what an operator recognises in a fleet view:
/// in Kubernetes it is the pod name, on a virtual machine it is the machine.
fn host_name() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| {
            std::process::Command::new("hostname")
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "worker".to_string())
        })
}

/// The shortest filter a fleet is allowed to search for, in symbols.
///
/// A guard against the absurd rather than against the merely unwise: one or two
/// symbols would have a fleet reporting finds faster than the master can look
/// at them, and nobody wants a million keys anyway.
const FLEET_MIN_SYMBOLS: usize = 4;

/// What a find rate has to stay under, measured: a key directory takes about
/// 6 ms to write, so some 160 a second and no more
/// (`cargo bench --bench store-rate`).
const STORE_PER_SECOND: f64 = 160.0;

/// The same, for a database. A thousand finds share one transaction, which is
/// what lifts the ceiling: measured against 162 a second for directories on
/// the same machine, SQLite took 244 000, PostgreSQL 32 000 and MySQL 22 700
/// (`cargo bench --bench store-rate`).
///
/// The figure here is the slowest of those, rounded down. What a database
/// waits for is one flush per batch, and how long that takes belongs to the
/// file system or the network rather than to this program.
const DATABASE_PER_SECOND: f64 = 20_000.0;

/// Refuses a filter set too broad to distribute, and prices the rest.
///
/// Lease traffic is not the constraint: the master answers 20 000 requests a
/// second and a worker sends four a minute. Finds are, because each one is a
/// key derivation and three files under one lock.
fn check_worth_distributing(filters: &FilterSet, database: bool) -> Result<(), String> {
    let ceiling = if database {
        DATABASE_PER_SECOND
    } else {
        STORE_PER_SECOND
    };
    let shortest = filters.iter().map(|f| f.len()).min().unwrap_or(0);
    if shortest < FLEET_MIN_SYMBOLS {
        return Err(format!(
            "a fleet will not search for {shortest} symbol(s): at that length the \
             workers report finds far faster than the master can take them. Use at \
             least {FLEET_MIN_SYMBOLS}, or run a single search instead"
        ));
    }

    // The number worth saying out loud, because the limit above does not imply
    // it: four symbols clears the floor and still floods a master from one
    // card.
    let share = filters.probability();
    if share > 0.0 {
        let per_gigahash = share * 1e9;
        if per_gigahash > ceiling {
            eprintln!(
                "onion-gen: warning: this set matches one address in {}, so a fleet \
                 running at 1 Gh/s finds {per_gigahash:.0} a second and the store \
                 takes about {ceiling:.0}. Workers will queue.",
                onion_gen::run::human_count(1.0 / share)
            );
        } else if per_gigahash >= 1.0 {
            eprintln!(
                "onion-gen: at 1 Gh/s this set yields about {per_gigahash:.1} find(s) \
                 a second; the store takes about {ceiling:.0}"
            );
        } else {
            // Below one a second the rate rounds to nothing and says nothing.
            // How long to wait for one is the useful form, and for most real
            // filters that is the form it will take.
            eprintln!(
                "onion-gen: at 1 Gh/s this set yields about one find every {}",
                onion_gen::run::human_duration(1.0 / per_gigahash)
            );
        }
    }
    Ok(())
}
