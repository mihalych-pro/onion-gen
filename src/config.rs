//! The configuration model: three sources with a fixed precedence.
//!
//! Flags beat environment variables, which beat the YAML file, which beats the
//! built-in defaults: configuration baked into a container image has to be
//! overridable from outside, and a hand-typed flag has to win over everything.
//!
//! Everything resolves once, at startup, into an immutable [`Config`]. The hot
//! loop never reads a setting — no string pointers, no `Option` checks inside a
//! batch — so runtime configurability costs no throughput.

use clap::{Args, Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

/// The environment variable prefix for every setting.
const ENV_PREFIX: &str = "ONION_GEN_";

/// The resolved configuration. Immutable once built.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Worker threads. Zero is allowed only when a device is doing the work.
    pub threads: usize,
    /// Whether any layer named the thread count.
    ///
    /// Zero means two different things depending on the answer — "let the
    /// device do it" when asked for, and nothing at all when not — and the
    /// merged value alone cannot tell them apart. Not part of the file format:
    /// it describes how the configuration was reached, not what it says.
    ///
    /// One consequence is worth knowing. `--print-config` runs before devices
    /// are looked for, so it prints the machine's core count rather than the
    /// count a device would leave; feeding that file back pins the number
    /// instead of deciding it. The line the run prints before searching is the
    /// one that says what was actually used.
    #[serde(skip)]
    pub threads_explicit: bool,
    /// Whether any layer named the batch size.
    ///
    /// Left unsaid, the program measures this machine and picks one; named, it
    /// is taken as given. Not part of the file format: it describes how the
    /// configuration was reached, not what it says.
    #[serde(skip)]
    pub batch_size_explicit: bool,
    /// Candidates generated per batch.
    pub batch_size: usize,
    /// Stop after this many hits; `0` means run until interrupted.
    pub limit: u64,
    /// Statistics interval in seconds; `0` disables statistics.
    pub stats_interval: f64,
    /// Where key directories are created.
    pub out_dir: PathBuf,
    /// Where to keep finds instead of in key directories: a connection
    /// string, or a plain path for a SQLite file. Created if it is not there.
    pub db: Option<String>,
    /// Survey the machine before searching and say what each prefix length
    /// would cost.
    pub evaluate: bool,
    /// Measure again rather than reuse what is cached.
    pub retune: bool,
    /// Print each found address on stdout.
    pub print_addresses: bool,
    /// Filters given directly.
    pub filters: Vec<String>,
    /// A file holding one filter per line.
    pub filter_files: Vec<PathBuf>,
    /// Which field arithmetic to use: `auto`, `scalar`, `neon` or `avx2`.
    ///
    /// `auto` takes the best the processor offers. Naming one explicitly is for
    /// comparing implementations in a measurement, reproducing somebody else's
    /// run, and working around a processor that reports a set it cannot
    /// actually run.
    pub arithmetic: String,
    /// Seed every key of the run comes from, as 64 hex characters. Empty
    /// means a fresh random one.
    ///
    /// Naming it makes a run reproducible and lets a caller keep the run's
    /// place itself instead of in a file. It is a secret: on most systems the
    /// command line of a process is readable by other users, so the
    /// environment variable is the safer way to pass it.
    pub seed: Option<String>,
    /// Which block to claim first. Only meaningful together with a named seed:
    /// against a fresh random seed a block number means nothing.
    pub from_block: u64,
    /// Which path computes candidates: `auto`, `cpu` or `gpu`.
    ///
    /// `auto` takes a device when one is there. Naming one explicitly is for
    /// the same three reasons as the arithmetic: comparing paths in a
    /// measurement, reproducing somebody else's run, and working around a
    /// driver that is present but broken.
    pub compute: String,
    /// Which devices to use, by index. Empty means all of them.
    ///
    /// Exists so that part of a machine can be left to other work.
    pub devices: Vec<usize>,
    /// Emit one JSON object per line instead of prose, for a caller that would
    /// otherwise parse text with a regular expression.
    pub json: bool,
    /// Where to keep what a run needs in order to continue after an
    /// interruption. Empty keeps nothing.
    ///
    /// The file holds the root seed, which derives every key the run produces,
    /// so it is as secret as a key and is written with the same permissions.
    pub state_file: Option<PathBuf>,
    /// How many chains a device keeps in flight. Zero means: as many as it
    /// will take.
    ///
    /// The working buffers are proportional to this, and so is throughput up
    /// to a point. Naming it is for the machine where the automatic choice is
    /// too greedy — a device shared with a display, say — and for measuring
    /// the curve.
    pub device_threads: u32,
    /// Finds scoring below this are not written.
    ///
    /// Off unless asked for. An extra directory can be deleted; a key that was
    /// not written is gone, because the same address will not turn up again.
    pub min_score: Option<f64>,
    /// Width of the prefix index, in bits of the packed key.
    ///
    /// The index holds `2^bits` bits, so 24 is 2 MiB and 30 is 128 MiB. Wider
    /// means fewer false hits but a map that no longer fits in cache.
    pub index_bits: u32,
}

/// Where found keys go when nothing says otherwise.
pub const DEFAULT_OUT_DIR: &str = "./out";

impl Default for Config {
    fn default() -> Self {
        Config {
            threads: crate::run::default_threads(),
            threads_explicit: false,
            batch_size_explicit: false,
            batch_size: crate::batch::DEFAULT_BATCH_SIZE,
            limit: 0,
            stats_interval: 0.0,
            out_dir: PathBuf::from(DEFAULT_OUT_DIR),
            db: None,
            evaluate: false,
            retune: false,
            print_addresses: true,
            filters: Vec::new(),
            filter_files: Vec::new(),
            seed: None,
            from_block: 0,
            arithmetic: "auto".to_string(),
            compute: "auto".to_string(),
            devices: Vec::new(),
            json: false,
            state_file: None,
            device_threads: 0,
            min_score: None,
            // Zero means "decide from the number of filters".
            index_bits: 0,
        }
    }
}

impl Config {
    pub fn limit(&self) -> Option<u64> {
        (self.limit > 0).then_some(self.limit)
    }

    /// Whether the run should take the scalar path.
    ///
    /// Everything that is not the scalar implementation goes through the vector
    /// engine, which dispatches to whatever the processor offers.
    pub fn force_scalar(&self) -> bool {
        self.arithmetic == "scalar"
    }

    /// Whether a vector implementation was asked for by name. The paired engine
    /// is the default on the processor, so the vector path runs only on request.
    pub fn force_vector(&self) -> bool {
        self.arithmetic == "neon" || self.arithmetic == "avx2"
    }

    pub fn stats_interval(&self) -> Option<Duration> {
        (self.stats_interval > 0.0).then(|| Duration::from_secs_f64(self.stats_interval))
    }

    /// Serialises the effective configuration in the same format the file uses,
    /// so that a run can be reproduced exactly and so that measurements record
    /// what they actually measured.
    pub fn to_yaml(&self) -> String {
        serde_yaml_ng::to_string(self).expect("configuration must serialise")
    }

    /// Rejects values that cannot work, before any search begins. Discovering a
    /// bad setting an hour into a run means losing that hour.
    pub fn validate(&self) -> Result<(), ConfigError> {
        self.validate_for(true)
    }

    /// The same checks, told whether the caller needs filters of its own.
    ///
    /// A worker does not: its set arrives from the master, and demanding one on
    /// the command line would make every worker in a fleet carry a copy of
    /// something the master is there to distribute.
    pub fn validate_for(&self, needs_filters: bool) -> Result<(), ConfigError> {
        // Zero threads is checked where the devices are known, in `main`: on
        // its own it says "let the device do it", and only the absence of a
        // device makes it wrong.
        if self.batch_size == 0 {
            return Err(ConfigError::Invalid {
                setting: "batch_size",
                reason: "must be at least 1".into(),
            });
        }
        if !self.stats_interval.is_finite() || self.stats_interval < 0.0 {
            return Err(ConfigError::Invalid {
                setting: "stats_interval",
                reason: "must be a non-negative number of seconds".into(),
            });
        }
        if self.arithmetic != "auto" {
            match crate::field4::Implementation::parse(&self.arithmetic) {
                None => {
                    return Err(ConfigError::Invalid {
                        setting: "arithmetic",
                        reason: "must be auto, scalar, neon or avx2".into(),
                    })
                }
                Some(implementation) if !implementation.is_available() => {
                    let available: Vec<&str> = crate::field4::available()
                        .into_iter()
                        .map(|i| i.name())
                        .collect();
                    return Err(ConfigError::Invalid {
                        setting: "arithmetic",
                        reason: format!(
                            "{:?} is not available on this processor; available: {}",
                            self.arithmetic,
                            available.join(", ")
                        ),
                    });
                }
                Some(_) => {}
            }
        }
        if self.index_bits != 0 && !(8..=32).contains(&self.index_bits) {
            return Err(ConfigError::Invalid {
                setting: "index_bits",
                reason: "must be between 8 and 32".into(),
            });
        }
        if needs_filters && self.filters.is_empty() && self.filter_files.is_empty() {
            return Err(ConfigError::Invalid {
                setting: "filters",
                reason: "give at least one --filter, or a --filter-file".into(),
            });
        }
        Ok(())
    }
}

/// A configuration layer: every field optional, so that an unset field defers
/// to the next source down.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Layer {
    pub threads: Option<usize>,
    pub batch_size: Option<usize>,
    pub limit: Option<u64>,
    pub stats_interval: Option<f64>,
    pub out_dir: Option<PathBuf>,
    pub db: Option<String>,
    pub evaluate: Option<bool>,
    pub retune: Option<bool>,
    pub print_addresses: Option<bool>,
    pub filters: Option<Vec<String>>,
    pub filter_files: Option<Vec<PathBuf>>,
    pub seed: Option<String>,
    pub from_block: Option<u64>,
    pub arithmetic: Option<String>,
    pub compute: Option<String>,
    pub devices: Option<Vec<usize>>,
    pub json: Option<bool>,
    pub state_file: Option<PathBuf>,
    pub device_threads: Option<u32>,
    pub min_score: Option<f64>,
    pub index_bits: Option<u32>,
}

impl Layer {
    /// Applies this layer over `base`. Only fields this layer sets are changed.
    fn apply_to(&self, base: &mut Config) {
        if let Some(v) = self.threads {
            base.threads = v;
            base.threads_explicit = true;
        }
        if let Some(v) = self.batch_size {
            base.batch_size = v;
            base.batch_size_explicit = true;
        }
        if let Some(v) = self.limit {
            base.limit = v;
        }
        if let Some(v) = self.stats_interval {
            base.stats_interval = v;
        }
        if let Some(v) = &self.out_dir {
            base.out_dir = v.clone();
        }
        if let Some(v) = &self.db {
            base.db = Some(v.clone());
        }
        if let Some(v) = self.evaluate {
            base.evaluate = v;
        }
        if let Some(v) = self.retune {
            base.retune = v;
        }
        if let Some(v) = self.print_addresses {
            base.print_addresses = v;
        }
        if let Some(v) = &self.filters {
            base.filters = v.clone();
        }
        if let Some(v) = &self.filter_files {
            base.filter_files = v.clone();
        }
        if let Some(v) = &self.seed {
            base.seed = Some(v.clone());
        }
        if let Some(v) = self.from_block {
            base.from_block = v;
        }
        if let Some(v) = &self.arithmetic {
            base.arithmetic = v.clone();
        }
        if let Some(v) = &self.compute {
            base.compute = v.clone();
        }
        if let Some(v) = &self.devices {
            base.devices = v.clone();
        }
        if let Some(v) = self.device_threads {
            base.device_threads = v;
        }
        if let Some(v) = self.min_score {
            base.min_score = Some(v);
        }
        if let Some(v) = &self.state_file {
            base.state_file = Some(v.clone());
        }
        if let Some(v) = self.json {
            base.json = v;
        }
        if let Some(v) = self.index_bits {
            base.index_bits = v;
        }
    }
}

/// Command-line flags. Every setting is optional here so that leaving a flag
/// out defers to the layers below rather than overriding them with a default.
#[derive(Debug, Parser)]
#[command(
    name = "onion-gen",
    about = "A vanity address generator for Tor Onion Service v3",
    version
)]
pub struct Cli {
    /// A filter to search for. May be given more than once.
    ///
    /// There is no positional form. Filters and options would otherwise be
    /// told apart by position, and a mistyped flag would become a filter and
    /// send the run looking for something nobody asked for.
    #[arg(short = 'F', long = "filter", value_name = "FILTER")]
    pub filter: Vec<String>,

    /// Read filters from a file, one per line. May be given more than once.
    #[arg(short = 'f', long = "filter-file", value_name = "PATH")]
    pub filter_files: Vec<PathBuf>,

    /// The seed every key of this run comes from, as 64 hex characters.
    ///
    /// Makes a run reproducible, and together with `--from-block` lets a
    /// caller keep the run's place without a state file. It is a secret: the
    /// command line of a process is readable by other users on most systems,
    /// so prefer `ONION_GEN_SEED` in the environment.
    #[arg(long)]
    pub seed: Option<String>,

    /// Claim blocks from this one on, continuing a run with the same seed.
    #[arg(long)]
    pub from_block: Option<u64>,

    /// Directory to write key directories into [default: ./out].
    ///
    /// A found key becomes a directory named after its address, and a search
    /// left running produces them by the thousand. They go under a directory of
    /// their own so that a run started anywhere does not bury the working
    /// directory in them.
    #[arg(short = 'd', long, conflicts_with = "db")]
    pub out_dir: Option<PathBuf>,

    /// Keep found keys in a database instead of as directories.
    ///
    /// A connection string, in the form a Go program would take:
    /// `keys.db` or `sqlite://keys.db` for a file,
    /// `postgres://user:pass@host:5432/dbname`,
    /// `mysql://user:pass@host:3306/dbname`.
    ///
    /// One row per address, holding the same bytes the three files would. A
    /// master takes it too, and a fleet finding faster than a directory per
    /// key allows is what it is for.
    #[arg(long, value_name = "URL")]
    pub db: Option<String>,

    /// Before searching, measure this machine and say what each prefix length
    /// from 4 to 12 symbols would cost.
    ///
    /// A single run only: a fleet's rate is the fleet's to report, and a
    /// worker's own figure would say nothing about the search it joined.
    #[arg(long)]
    pub evaluate: bool,

    /// Measure the machine again instead of reusing what was measured before.
    ///
    /// The batch size and a device's launch shape are settled by running them,
    /// and the answer is kept so that it costs a second once rather than on
    /// every start. Pass this after a driver update, or to check what is
    /// cached against the machine as it is now.
    #[arg(long)]
    pub retune: bool,

    /// Worker threads [default: number of cores].
    #[arg(short = 't', long)]
    pub threads: Option<usize>,

    /// Candidates per batch.
    #[arg(long)]
    pub batch_size: Option<usize>,

    /// Stop after this many hits.
    #[arg(short = 'n', long)]
    pub limit: Option<u64>,

    /// Print statistics every N seconds.
    ///
    /// `-S` is accepted as well as `-s` so that one command line drives this
    /// binary and `mkp224o` alike, which is what lets a comparison be taken
    /// with the same harness. A measurement concession, not CLI compatibility
    /// in general.
    #[arg(short = 's', short_alias = 'S', long)]
    pub stats_interval: Option<f64>,

    /// Do not print found addresses on stdout.
    #[arg(short = 'x', long)]
    pub quiet: bool,

    /// Suppress informational messages on stderr. Statistics still appear.
    #[arg(global = true, short = 'q', long)]
    pub quiet_diagnostics: bool,

    /// Read settings from a YAML file.
    #[arg(global = true, short = 'c', long)]
    pub config: Option<PathBuf>,

    /// Which path computes candidates: auto, cpu, gpu, portable, vendor,
    /// cuda, vulkan, metal or dx12.
    #[arg(long)]
    pub compute: Option<String>,

    /// Devices to use, by index, comma separated. Default is all of them.
    #[arg(long, value_delimiter = ',')]
    pub devices: Option<Vec<usize>>,

    /// How many chains a device keeps in flight; 0 takes as many as it will.
    #[arg(long)]
    pub device_threads: Option<u32>,

    /// Do not write finds scoring below this. Off by default.
    #[arg(long, value_name = "SCORE")]
    pub min_score: Option<f64>,

    /// One JSON object per line instead of prose.
    #[arg(global = true, long)]
    pub json: bool,

    /// Keep the run's place in this file, and continue from it if it exists.
    ///
    /// The file holds the seed every key of the run comes from, so treat it
    /// as a secret key.
    #[arg(long)]
    pub state: Option<PathBuf>,

    /// Field arithmetic: auto, scalar, neon or avx2.
    #[arg(long)]
    pub arithmetic: Option<String>,

    /// Width of the prefix index in bits (24 is 2 MiB, 30 is 128 MiB).
    #[arg(long)]
    pub index_bits: Option<u32>,

    /// Print the effective configuration and exit.
    #[arg(global = true, long)]
    pub print_config: bool,

    /// Which role to take. Absent means a single-machine run, which is what
    /// the program is ordinarily used for and so needs no name.
    #[command(subcommand)]
    pub role: Option<Role>,
}

/// The roles beyond a single-machine run.
///
/// Subcommands rather than flags because the settings barely overlap: a master
/// needs an address to listen on, a lease period and somewhere to keep what it
/// collects; a worker needs the master's address. A flag that only means
/// something alongside another flag reports its error later and worse than a
/// subcommand that simply has no such flag.
#[derive(Debug, Subcommand)]
pub enum Role {
    /// Coordinate a fleet: hand out work, take in what is found.
    Master(MasterArgs),
    /// Search on a master's behalf.
    Worker(WorkerArgs),
    /// Say how long a filter would take on this machine, without searching.
    Guess(GuessArgs),
    /// Check found keys and say what they are.
    Verify(VerifyArgs),
}

/// What a filter set is built from. Taken by anything that has to know what
/// is being looked for.
#[derive(Debug, Args, Default)]
pub struct FilterFlags {
    /// A filter to search for. May be given more than once.
    #[arg(short = 'F', long = "filter", value_name = "FILTER")]
    pub filter: Vec<String>,

    /// Read filters from a file, one per line. May be given more than once.
    #[arg(short = 'f', long = "filter-file", value_name = "PATH")]
    pub filter_files: Vec<PathBuf>,

    /// Bits of the prefix bitmap; 0 chooses from the filter count.
    #[arg(long, value_name = "BITS")]
    pub index_bits: Option<u32>,
}

/// What decides how candidates are produced. Taken by anything that searches.
#[derive(Debug, Args, Default)]
pub struct EngineFlags {
    /// Worker threads [default: number of cores].
    #[arg(short = 't', long)]
    pub threads: Option<usize>,

    /// Candidates per batch [default: measured].
    #[arg(long)]
    pub batch_size: Option<usize>,

    /// Which path computes candidates: auto, cpu, gpu, portable, vendor,
    /// cuda, vulkan, metal or dx12.
    #[arg(long)]
    pub compute: Option<String>,

    /// Devices to use, by index, comma separated. Default is all of them.
    #[arg(long, value_delimiter = ',')]
    pub devices: Option<Vec<usize>>,

    /// How many chains a device keeps in flight; 0 takes as many as it will.
    #[arg(long)]
    pub device_threads: Option<u32>,

    /// Which field arithmetic to use: auto, scalar, neon or avx2.
    #[arg(long)]
    pub arithmetic: Option<String>,

    /// Measure the machine again instead of reusing what was measured before.
    #[arg(long)]
    pub retune: bool,
}

/// What `guess` takes.
#[derive(Debug, Args)]
pub struct GuessArgs {
    #[command(flatten)]
    pub filters: FilterFlags,

    /// Worker threads to measure with [default: number of cores].
    #[arg(short = 't', long)]
    pub threads: Option<usize>,

    /// How long to measure this machine for before answering.
    ///
    /// The rate is taken from the engine that would do the searching, so the
    /// answer is about this machine rather than about some other one.
    #[arg(long, value_name = "SECONDS", default_value_t = 2.0)]
    pub measure_seconds: f64,
}

/// What `verify` takes.
#[derive(Debug, Args)]
pub struct VerifyArgs {
    /// A key directory, or a directory holding them. Defaults to `--out-dir`.
    #[arg(value_name = "PATH")]
    pub path: Option<PathBuf>,
}

/// What only the master takes.
#[derive(Debug, Args)]
pub struct MasterArgs {
    #[command(flatten)]
    pub filters: FilterFlags,

    /// The seed every key of this fleet comes from, as 64 hex characters.
    #[arg(long)]
    pub seed: Option<String>,

    /// Hand out blocks from this one on, continuing a fleet with the same seed.
    #[arg(long)]
    pub from_block: Option<u64>,

    /// Keep finds in a database instead of as directories. A connection
    /// string: a path, `sqlite://`, `postgres://` or `mysql://`.
    #[arg(long, value_name = "URL")]
    pub db: Option<String>,

    /// Address to listen on, such as `0.0.0.0:8080`.
    #[arg(long, value_name = "ADDR", default_value = "0.0.0.0:8080")]
    pub listen: String,

    /// Where found keys are written, in the same layout a single run uses.
    #[arg(long, value_name = "DIR", default_value = "keys")]
    pub store: PathBuf,

    /// How long a worker holds a range before it returns to the queue.
    ///
    /// Short enough that a lost worker's range comes back quickly, long enough
    /// that a working one is not renewing constantly.
    #[arg(long, value_name = "SECONDS", default_value_t = 60)]
    pub lease_seconds: u64,

    /// How much work a lease should be, measured in seconds of the worker's
    /// own rate.
    ///
    /// The point of sizing by time rather than by blocks: the measured spread
    /// between the fastest card and a processor is 14.3x, so one block count
    /// is wrong for one of them whatever it is.
    #[arg(long, value_name = "SECONDS", default_value_t = 60)]
    pub lease_work_seconds: u64,
}

/// What only the worker takes.
#[derive(Debug, Args)]
pub struct WorkerArgs {
    #[command(flatten)]
    pub engine: EngineFlags,

    /// The master to take work from, such as `http://master:8080`.
    #[arg(long, value_name = "URL")]
    pub master: String,

    /// A name for this worker in the master's fleet view. Defaults to the
    /// host name, which is what an operator recognises.
    #[arg(long, value_name = "NAME")]
    pub name: Option<String>,

    /// Where to answer metrics and probes, such as `0.0.0.0:9100`.
    ///
    /// Read-only: nothing reachable here can give the worker instructions, so
    /// it does not undo the rule that coordination travels one way.
    #[arg(long, value_name = "ADDR", default_value = "0.0.0.0:9100")]
    pub endpoint: String,

    /// How many unsent finds to keep on disk before saying so.
    ///
    /// Reaching it is not a reason to discard anything: a find that took a
    /// week will not come round again.
    #[arg(long, value_name = "COUNT", default_value_t = 1024)]
    pub buffer_limit: usize,
}

impl Cli {
    fn layer(&self) -> Layer {
        let mut base = Layer {
            threads: self.threads,
            batch_size: self.batch_size,
            limit: self.limit,
            stats_interval: self.stats_interval,
            out_dir: self.out_dir.clone(),
            db: self.db.clone(),
            // Absent must not override a lower layer, the same as `--quiet`.
            evaluate: self.evaluate.then_some(true),
            retune: self.retune.then_some(true),
            // `--quiet` is the only way to set this from the command line, so
            // its absence must not override a lower layer.
            print_addresses: self.quiet.then_some(false),
            filters: (!self.filter.is_empty()).then(|| self.filter.clone()),
            filter_files: (!self.filter_files.is_empty()).then(|| self.filter_files.clone()),
            seed: self.seed.clone(),
            from_block: self.from_block,
            arithmetic: self.arithmetic.clone(),
            compute: self.compute.clone(),
            devices: self.devices.clone(),
            state_file: self.state.clone(),
            // `--json` is a flag, so its absence must not override a lower
            // layer — the same reason `--quiet` is written this way.
            json: self.json.then_some(true),
            device_threads: self.device_threads,
            min_score: self.min_score,
            index_bits: self.index_bits,
        };
        // A subcommand carries only the flags it has a use for, so what it was
        // given is laid over the root's. Without this the flags would parse
        // and then do nothing, which is worse than not offering them.
        match &self.role {
            Some(Role::Master(a)) => {
                base.apply_filters(&a.filters);
                if a.seed.is_some() {
                    base.seed = a.seed.clone();
                }
                if a.from_block.is_some() {
                    base.from_block = a.from_block;
                }
                if a.db.is_some() {
                    base.db = a.db.clone();
                }
            }
            Some(Role::Worker(a)) => base.apply_engine(&a.engine),
            Some(Role::Guess(a)) => {
                base.apply_filters(&a.filters);
                if a.threads.is_some() {
                    base.threads = a.threads;
                }
            }
            Some(Role::Verify(_)) | None => {}
        }
        base
    }
}

impl Layer {
    /// Takes whatever a subcommand's filter flags were given.
    fn apply_filters(&mut self, flags: &FilterFlags) {
        if !flags.filter.is_empty() {
            self.filters = Some(flags.filter.clone());
        }
        if !flags.filter_files.is_empty() {
            self.filter_files = Some(flags.filter_files.clone());
        }
        if flags.index_bits.is_some() {
            self.index_bits = flags.index_bits;
        }
    }

    /// The same for the flags that decide how candidates are produced.
    fn apply_engine(&mut self, flags: &EngineFlags) {
        if flags.threads.is_some() {
            self.threads = flags.threads;
        }
        if flags.batch_size.is_some() {
            self.batch_size = flags.batch_size;
        }
        if flags.compute.is_some() {
            self.compute = flags.compute.clone();
        }
        if flags.devices.is_some() {
            self.devices = flags.devices.clone();
        }
        if flags.device_threads.is_some() {
            self.device_threads = flags.device_threads;
        }
        if flags.arithmetic.is_some() {
            self.arithmetic = flags.arithmetic.clone();
        }
        if flags.retune {
            self.retune = Some(true);
        }
    }
}

/// What went wrong while building the configuration.
#[derive(Debug)]
pub enum ConfigError {
    Invalid {
        setting: &'static str,
        reason: String,
    },
    Env {
        variable: String,
        value: String,
        reason: String,
    },
    File {
        path: PathBuf,
        reason: String,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Invalid { setting, reason } => {
                write!(f, "setting {setting:?} is invalid: {reason}")
            }
            ConfigError::Env {
                variable,
                value,
                reason,
            } => write!(
                f,
                "environment variable {variable}={value:?} is invalid: {reason}"
            ),
            ConfigError::File { path, reason } => {
                write!(f, "config file {}: {reason}", path.display())
            }
        }
    }
}

impl std::error::Error for ConfigError {}

/// Reads a layer from the environment.
fn env_layer<F>(get: F) -> Result<Layer, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    fn parse<T: std::str::FromStr>(
        name: &str,
        raw: Option<String>,
        expected: &str,
    ) -> Result<Option<T>, ConfigError> {
        match raw {
            None => Ok(None),
            Some(value) => value
                .trim()
                .parse::<T>()
                .map(Some)
                .map_err(|_| ConfigError::Env {
                    variable: format!("{ENV_PREFIX}{name}"),
                    value,
                    reason: expected.to_string(),
                }),
        }
    }

    let fetch = |name: &str| get(&format!("{ENV_PREFIX}{name}"));

    Ok(Layer {
        threads: parse("THREADS", fetch("THREADS"), "a positive integer")?,
        batch_size: parse("BATCH_SIZE", fetch("BATCH_SIZE"), "a positive integer")?,
        limit: parse("LIMIT", fetch("LIMIT"), "a non-negative integer")?,
        stats_interval: parse(
            "STATS_INTERVAL",
            fetch("STATS_INTERVAL"),
            "a number of seconds",
        )?,
        out_dir: fetch("OUT_DIR").map(PathBuf::from),
        db: fetch("DB"),
        evaluate: parse("EVALUATE", fetch("EVALUATE"), "true or false")?,
        retune: parse("RETUNE", fetch("RETUNE"), "true or false")?,
        print_addresses: parse("PRINT_ADDRESSES", fetch("PRINT_ADDRESSES"), "true or false")?,
        filters: fetch("FILTERS").map(|v| {
            v.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect()
        }),
        filter_files: fetch("FILTER_FILES")
            .or_else(|| fetch("FILTER_FILE"))
            .map(|v| {
                v.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(PathBuf::from)
                    .collect()
            }),
        seed: fetch("SEED"),
        from_block: parse("FROM_BLOCK", fetch("FROM_BLOCK"), "a block number")?,
        arithmetic: fetch("ARITHMETIC"),
        compute: fetch("COMPUTE"),
        devices: match fetch("DEVICES") {
            None => None,
            Some(v) => Some(
                v.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| {
                        s.parse().map_err(|_| ConfigError::Env {
                            variable: format!("{ENV_PREFIX}DEVICES"),
                            value: s.to_string(),
                            reason: "expected a comma separated list of device indices".to_string(),
                        })
                    })
                    .collect::<Result<Vec<usize>, _>>()?,
            ),
        },
        state_file: fetch("STATE_FILE").map(PathBuf::from),
        json: parse("JSON", fetch("JSON"), "true or false")?,
        device_threads: parse(
            "DEVICE_THREADS",
            fetch("DEVICE_THREADS"),
            "how many chains a device keeps in flight, or 0 for automatic",
        )?,
        min_score: parse(
            "MIN_SCORE",
            fetch("MIN_SCORE"),
            "the lowest score worth writing to disk",
        )?,
        index_bits: parse(
            "INDEX_BITS",
            fetch("INDEX_BITS"),
            "a width between 8 and 32",
        )?,
    })
}

/// Reads a layer from a YAML file.
fn file_layer(path: &PathBuf) -> Result<Layer, ConfigError> {
    let text = std::fs::read_to_string(path).map_err(|e| ConfigError::File {
        path: path.clone(),
        reason: e.to_string(),
    })?;
    serde_yaml_ng::from_str(&text).map_err(|e| ConfigError::File {
        path: path.clone(),
        reason: e.to_string(),
    })
}

/// Resolves the three sources into one immutable configuration.
///
/// `get_env` is a parameter rather than a direct `std::env::var` call so the
/// precedence rules can be tested without mutating the process environment.
pub fn resolve<F>(cli: &Cli, get_env: F) -> Result<Config, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let env = env_layer(&get_env)?;
    let cli_layer = cli.layer();

    // The file itself may be named by a flag or by the environment, in that
    // same order of precedence.
    let config_path = cli
        .config
        .clone()
        .or_else(|| get_env(&format!("{ENV_PREFIX}CONFIG")).map(PathBuf::from));

    let mut config = Config::default();
    if let Some(path) = &config_path {
        file_layer(path)?.apply_to(&mut config);
    }
    env.apply_to(&mut config);
    cli_layer.apply_to(&mut config);

    Ok(config)
}

#[cfg(test)]
mod tests {
    /// Filters are asked of whoever needs them.
    #[test]
    fn a_worker_needs_no_filters_of_its_own() {
        let mut c = super::Config::default();
        c.filters.clear();
        c.filter_files.clear();
        assert!(
            c.validate().is_err(),
            "a single run without filters is still an error"
        );
        assert!(
            c.validate_for(false).is_ok(),
            "a worker's set arrives from the master"
        );
    }

    use super::*;

    fn cli(args: &[&str]) -> Cli {
        let mut full = vec!["onion-gen"];
        full.extend_from_slice(args);
        Cli::parse_from(full)
    }

    fn no_env(_: &str) -> Option<String> {
        None
    }

    fn write_yaml(tag: &str, body: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("onion-gen-cfg-{tag}-{}", std::process::id()));
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn defaults_apply_last() {
        let c = resolve(&cli(&["-F", "abc"]), no_env).unwrap();
        assert_eq!(c.threads, crate::run::default_threads());
        assert_eq!(c.batch_size, crate::batch::DEFAULT_BATCH_SIZE);
        assert_eq!(c.limit, 0);
        assert!(c.print_addresses);
        assert_eq!(c.filters, vec!["abc".to_string()]);
    }

    #[test]
    fn a_flag_beats_an_environment_variable() {
        let env = |k: &str| (k == "ONION_GEN_THREADS").then(|| "3".to_string());
        let c = resolve(&cli(&["-t", "5", "-F", "abc"]), env).unwrap();
        assert_eq!(c.threads, 5);
    }

    #[test]
    fn an_environment_variable_beats_the_file() {
        let path = write_yaml("env-over-file", "threads: 2\nbatch_size: 64\n");
        let env = |k: &str| (k == "ONION_GEN_THREADS").then(|| "9".to_string());
        let c = resolve(&cli(&["-c", path.to_str().unwrap(), "-F", "abc"]), env).unwrap();
        assert_eq!(c.threads, 9, "the environment must win");
        assert_eq!(c.batch_size, 64, "and the file must still supply the rest");
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn the_file_beats_the_defaults() {
        let path = write_yaml("file-over-default", "threads: 2\nlimit: 7\n");
        let c = resolve(&cli(&["-c", path.to_str().unwrap(), "-F", "abc"]), no_env).unwrap();
        assert_eq!(c.threads, 2);
        assert_eq!(c.limit, 7);
        assert_eq!(c.batch_size, crate::batch::DEFAULT_BATCH_SIZE);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn an_absent_flag_does_not_override_a_lower_layer() {
        // -x is a switch: not passing it must leave the file's value alone.
        let path = write_yaml("switch", "print_addresses: false\n");
        let c = resolve(&cli(&["-c", path.to_str().unwrap(), "-F", "abc"]), no_env).unwrap();
        assert!(!c.print_addresses);
        std::fs::remove_file(&path).unwrap();
    }

    /// The flag repeats, and the short and long spellings are the same flag.
    #[test]
    fn filters_arrive_by_a_flag_that_repeats() {
        let c = resolve(
            &cli(&["-F", "abc", "--filter", "bcd", "-F", "cde", "-n", "1"]),
            no_env,
        )
        .unwrap();
        assert_eq!(c.filters, vec!["abc", "bcd", "cde"]);
    }

    /// There is no positional form, and a word that looks like one is an error
    /// rather than a filter. A mistyped flag would otherwise become a filter
    /// and send the run looking for something nobody asked for.
    #[test]
    fn a_bare_word_is_not_a_filter() {
        let mut full = vec!["onion-gen", "-n", "1", "abc"];
        assert!(Cli::try_parse_from(full.clone()).is_err(), "{full:?}");
        full = vec!["onion-gen", "-F", "abc", "bcd"];
        assert!(Cli::try_parse_from(full.clone()).is_err(), "{full:?}");
    }

    /// Several dictionaries in one run: one of names, one of words, one of
    /// whatever else, without having to concatenate them first.
    #[test]
    fn more_than_one_filter_file_is_accepted() {
        let c = resolve(&cli(&["-f", "a.txt", "-f", "b.txt", "-n", "1"]), no_env).unwrap();
        assert_eq!(
            c.filter_files,
            vec![PathBuf::from("a.txt"), PathBuf::from("b.txt")]
        );
        // And a run with only files and no words is a run, not an error.
        assert!(c.validate().is_ok());
    }

    /// The environment is the safer way to pass a seed, so it has to work.
    #[test]
    fn the_seed_and_the_block_come_from_the_environment_too() {
        let env = |name: &str| match name {
            "ONION_GEN_SEED" => Some("ab".repeat(32)),
            "ONION_GEN_FROM_BLOCK" => Some("17".to_string()),
            _ => None,
        };
        let c = resolve(&cli(&["-F", "abc", "-n", "1"]), env).unwrap();
        assert_eq!(c.seed.as_deref(), Some("ab".repeat(32).as_str()));
        assert_eq!(c.from_block, 17);
        // And a flag outranks the environment, as every other setting does.
        let c = resolve(&cli(&["-F", "abc", "-n", "1", "--from-block", "3"]), env).unwrap();
        assert_eq!(c.from_block, 3);
    }

    #[test]
    fn invalid_values_are_rejected_before_any_work() {
        // Zero threads is no longer refused here: it means "let the device do
        // it", and whether that is possible depends on the devices, which this
        // layer does not know. `main` refuses it when there is no device.
        let c = resolve(&cli(&["-t", "0", "-F", "abc"]), no_env).unwrap();
        assert!(c.validate().is_ok());
        assert_eq!(c.threads, 0);
        assert!(c.threads_explicit, "asked for, not defaulted");

        let c = resolve(&cli(&["-F", "abc"]), no_env).unwrap();
        assert!(
            !c.threads_explicit,
            "not asked for, so the device count may lower it"
        );

        let c = resolve(&cli(&["--batch-size", "0", "-F", "abc"]), no_env).unwrap();
        assert!(matches!(
            c.validate(),
            Err(ConfigError::Invalid {
                setting: "batch_size",
                ..
            })
        ));

        let c = resolve(&cli(&[]), no_env).unwrap();
        assert!(matches!(
            c.validate(),
            Err(ConfigError::Invalid {
                setting: "filters",
                ..
            })
        ));
    }

    #[test]
    fn a_bad_environment_value_names_the_variable() {
        let env = |k: &str| (k == "ONION_GEN_THREADS").then(|| "many".to_string());
        match resolve(&cli(&["-F", "abc"]), env) {
            Err(ConfigError::Env { variable, .. }) => {
                assert_eq!(variable, "ONION_GEN_THREADS")
            }
            other => panic!("expected an env error, got {other:?}"),
        }
    }

    #[test]
    fn an_unreadable_file_names_the_path() {
        let missing = PathBuf::from("/nonexistent/onion-gen.yaml");
        match resolve(
            &cli(&["-c", missing.to_str().unwrap(), "-F", "abc"]),
            no_env,
        ) {
            Err(ConfigError::File { path, .. }) => assert_eq!(path, missing),
            other => panic!("expected a file error, got {other:?}"),
        }

        let path = write_yaml("garbage", "threads: [this is not a number\n");
        assert!(matches!(
            resolve(&cli(&["-c", path.to_str().unwrap(), "-F", "abc"]), no_env),
            Err(ConfigError::File { .. })
        ));
        std::fs::remove_file(&path).unwrap();
    }

    /// The printed configuration, fed back as the only source, must reproduce
    /// the same configuration. Without that a run is not reproducible.
    #[test]
    fn the_printed_configuration_round_trips() {
        let original = resolve(
            &cli(&[
                "-t", "3", "-n", "5", "-d", "/tmp/x", "-F", "abc", "-F", "xyz",
            ]),
            no_env,
        )
        .unwrap();
        let path = write_yaml("roundtrip", &original.to_yaml());
        let restored = resolve(&cli(&["-c", path.to_str().unwrap()]), no_env).unwrap();

        // The printed file names the batch size, so reading it back is naming
        // it — and a run from that file must use the size the first run used
        // rather than measure a new one. That is the point of printing it.
        let mut expected = original.clone();
        expected.batch_size_explicit = true;
        assert_eq!(restored, expected);
        assert_eq!(restored.batch_size, original.batch_size);
        std::fs::remove_file(&path).unwrap();
    }
}
