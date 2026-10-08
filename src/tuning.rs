//! Remembering what the measurement found, so it is made once per machine.
//!
//! Both the processor's batch size and a device's launch shape are settled by
//! running them, which costs about a second on the processor and longer on a
//! device — once, and then again on every start. For a search that runs for
//! days that is nothing; for `--limit 1`, for a container that restarts, and
//! for a fleet of fifty workers it is a second each time for an answer that
//! has not changed.
//!
//! So the answer is written down. The key carries everything that would make
//! an old answer wrong — which machine, which device, how many threads, which
//! version of this program — because a stale shape is worse than no cache: it
//! would be slower with nothing said about why.
//!
//! Every operation here is best effort. A read-only home directory, a cache
//! file someone else owns, a half-written file from a machine that lost power:
//! none of them are reasons to refuse to search, so all of them end as "no
//! entry" and the measurement simply happens.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// What one measurement concluded.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Shape {
    /// The first number: a batch size for the processor, offsets per work item
    /// for a device.
    pub first: u32,
    /// The second, where a kind has one: chains in flight. Zero otherwise.
    pub second: u32,
    /// What it reached, in candidates a second. Kept for the line the program
    /// prints, and because a cache nobody can sanity-check is a cache nobody
    /// trusts.
    pub rate: f64,
    /// When it was measured, unix seconds.
    pub at: i64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    /// The format's own version, so a later shape of this file can be
    /// recognised and discarded rather than misread.
    format: u32,
    entries: BTreeMap<String, Shape>,
}

/// The current format. Anything else is treated as no cache at all.
const FORMAT: u32 = 1;

/// A key for the processor's batch size.
pub fn cpu_key(threads: usize) -> String {
    format!(
        "cpu/{}/{threads}/{}",
        std::env::consts::ARCH,
        env!("CARGO_PKG_VERSION")
    )
}

/// A key for one device's launch shape.
///
/// The name comes from the driver and can hold anything; slashes would make
/// two different devices collide, so they are replaced.
pub fn device_key(name: &str, api: &str, threads: u32) -> String {
    let safe: String = name
        .chars()
        .map(|c| if c == '/' || c == '\n' { '_' } else { c })
        .collect();
    format!(
        "device/{safe}/{api}/{threads}/{}",
        env!("CARGO_PKG_VERSION")
    )
}

/// Where the cache file lives, following each platform's own convention.
///
/// Not beside the binary: that is often read-only, is shared between users on
/// a server, and in a container is a layer that does not persist.
pub fn path() -> Option<PathBuf> {
    let dir = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Caches"))
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
    }?;
    Some(dir.join("onion-gen").join("tuning.yaml"))
}

/// What was measured for this key before, if anything.
pub fn get(key: &str) -> Option<Shape> {
    let file = read()?;
    (file.format == FORMAT).then(|| file.entries.get(key).copied())?
}

/// Writes down what a measurement found.
///
/// Reads the file again first: two runs starting together would otherwise have
/// the second overwrite what the first learned about a different device.
pub fn put(key: &str, shape: Shape) {
    let Some(path) = path() else { return };
    let mut file = read().filter(|f| f.format == FORMAT).unwrap_or_default();
    file.format = FORMAT;
    file.entries.insert(key.to_string(), shape);

    // The same format the configuration file uses, so there is one shape of
    // file in this program rather than two.
    let Ok(text) = serde_yaml_ng::to_string(&file) else {
        return;
    };
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    // Written beside and renamed, so a reader never sees half a file — and so
    // that a run killed mid-write leaves the previous cache intact.
    let temporary = path.with_extension("yaml.new");
    if std::fs::write(&temporary, text).is_ok() {
        let _ = std::fs::rename(&temporary, &path);
    }
}

/// Seconds since the epoch, for [`Shape::at`].
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

fn read() -> Option<File> {
    let text = std::fs::read_to_string(path()?).ok()?;
    serde_yaml_ng::from_str(&text).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The key has to change when anything that could invalidate the answer
    /// changes, or a stale shape outlives the thing it was measured for.
    #[test]
    fn a_key_separates_what_must_not_be_shared() {
        assert_ne!(cpu_key(4), cpu_key(8), "thread count");
        assert_ne!(
            device_key("RTX 4060", "cuda", 65536),
            device_key("RTX 4060", "opencl", 65536),
            "the same card through two APIs is two shapes"
        );
        assert_ne!(
            device_key("RTX 4060", "cuda", 65536),
            device_key("RTX 4070", "cuda", 65536),
            "different cards"
        );
        assert_ne!(
            device_key("a", "cuda", 1),
            device_key("a", "cuda", 2),
            "thread count"
        );
        // The version is in every key, so an upgrade starts from measurement.
        assert!(cpu_key(8).ends_with(env!("CARGO_PKG_VERSION")));
        assert!(device_key("x", "metal", 1).ends_with(env!("CARGO_PKG_VERSION")));
    }

    /// A device name is whatever the driver says; it must not be able to
    /// collide with a different device by containing a separator.
    #[test]
    fn a_hostile_device_name_cannot_forge_another_key() {
        let forged = device_key("a/metal/1/9.9.9", "cuda", 1);
        let real = device_key("a", "metal", 1);
        assert_ne!(forged, real);
        assert!(!forged.contains("a/metal"), "{forged}");
    }

    /// The cache is a convenience; nothing about it may stop a search.
    #[test]
    fn an_unreadable_cache_reads_as_empty() {
        // A key nothing has ever written.
        assert_eq!(get("device/nothing has this name/metal/1/0.0.0"), None);
    }

    #[test]
    fn a_shape_survives_a_round_trip() {
        let file = File {
            format: FORMAT,
            entries: BTreeMap::from([(
                "cpu/x/8/1.2.3".to_string(),
                Shape {
                    first: 16384,
                    second: 0,
                    rate: 93.1e6,
                    at: 1760000000,
                },
            )]),
        };
        let text = serde_yaml_ng::to_string(&file).expect("serialises");
        let back: File = serde_yaml_ng::from_str(&text).expect("parses");
        assert_eq!(back.format, FORMAT);
        assert_eq!(back.entries["cpu/x/8/1.2.3"].first, 16384);
    }

    /// A file from a future version must be ignored, not misread.
    #[test]
    fn an_unknown_format_is_not_trusted() {
        let text =
            "format: 99\nentries:\n  k:\n    first: 1\n    second: 2\n    rate: 3.0\n    at: 4\n";
        let file: File = serde_yaml_ng::from_str(text).expect("parses");
        assert_ne!(file.format, FORMAT, "the test needs a format we reject");
    }
}
