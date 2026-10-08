//! What the three SQL backends share: a runtime, a migration ledger, counters.
//!
//! The drivers are asynchronous and the search is not. Rather than colour the
//! whole program, each backend owns a runtime and blocks on it here.

use super::backend::Stats;
use super::Find;
use base64::prelude::{Engine as _, BASE64_STANDARD};
use std::future::Future;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::runtime::Runtime;

/// The table holding keys. Not `keys`: that is a reserved word in MySQL, and
/// quoting it differs in all three dialects.
pub const TABLE: &str = "onion_keys";
/// The table recording which migrations have run.
pub const LEDGER: &str = "onion_gen_migrations";

/// One step from one schema version to the next.
///
/// Each backend carries its own list, because the dialects disagree about
/// types even where they agree about meaning. The versions are a single
/// sequence across backends all the same, so that "schema 2" names the same
/// shape wherever it is.
pub struct Migration {
    pub version: i64,
    pub name: &'static str,
    /// Run in order, in one transaction. Several because some databases refuse
    /// more than one statement per call.
    pub statements: &'static [&'static str],
}

/// Runs a future to completion from synchronous code.
///
/// A master calls the store from inside its own runtime, where a plain
/// `block_on` panics; `block_in_place` is what makes that legal. Outside a
/// runtime — a single-machine run — the first branch is never taken.
pub fn wait<F: Future>(rt: &Runtime, future: F) -> F::Output {
    match tokio::runtime::Handle::try_current() {
        Ok(_) => tokio::task::block_in_place(|| rt.block_on(future)),
        Err(_) => rt.block_on(future),
    }
}

/// A runtime for one backend.
///
/// Multi-threaded rather than current-thread: `block_in_place` requires it,
/// and that is the call a master depends on.
pub fn runtime() -> io::Result<Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
}

/// Counters a backend keeps, shared with whoever reads the metrics.
#[derive(Default)]
pub struct Counters {
    pub rows: AtomicU64,
    pub bytes: AtomicU64,
    pub batches: AtomicU64,
    pub failures: AtomicU64,
}

impl Counters {
    pub fn committed(&self, rows: u64, bytes: u64) {
        self.rows.fetch_add(rows, Ordering::Relaxed);
        self.bytes.fetch_add(bytes, Ordering::Relaxed);
        self.batches.fetch_add(1, Ordering::Relaxed);
    }

    pub fn failed(&self) {
        self.failures.fetch_add(1, Ordering::Relaxed);
    }

    pub fn read_into(&self, connections: u32, idle: u32) -> Stats {
        Stats {
            connections,
            idle,
            rows: self.rows.load(Ordering::Relaxed),
            bytes: self.bytes.load(Ordering::Relaxed),
            batches: self.batches.load(Ordering::Relaxed),
            failures: self.failures.load(Ordering::Relaxed),
        }
    }
}

/// One find, as columns.
///
/// Built once per row and used by all three backends, so that a column added
/// in one place cannot be forgotten in another.
pub struct Row {
    pub address: String,
    pub secret_key: String,
    pub public_key: String,
    pub filter: String,
    pub score: f64,
    pub found_at: i64,
    pub block: i64,
    pub scalar_offset: i64,
    pub source: String,
}

impl Row {
    pub fn of(find: &Find, now: i64) -> Self {
        // Base64, which is how an SSH key is written down too: the key bytes
        // are not text and a column of them would not survive a client that
        // assumes UTF-8. The encoded form holds the whole file, headers
        // included, so `base64 -d` on a row produces exactly what tor reads.
        Row {
            address: find.address.clone(),
            secret_key: BASE64_STANDARD.encode(crate::output::secret_file_bytes(&find.secret)),
            public_key: BASE64_STANDARD.encode(crate::output::public_file_bytes(&find.public_key)),
            filter: find.filter.clone(),
            score: find.score,
            found_at: now,
            block: find.block as i64,
            scalar_offset: find.offset as i64,
            source: find.source.clone(),
        }
    }
}

/// Seconds since the epoch, for `found_at`.
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Key material in a batch, which is what the byte counter reports.
pub fn weight(batch: &[Find]) -> u64 {
    batch.len() as u64 * (96 + 64)
}

pub fn as_io(e: sqlx::Error) -> io::Error {
    io::Error::other(e.to_string())
}

/// Hides the password in a connection string so it can be printed.
///
/// A store describes itself in the startup line and in error messages, and a
/// password in either is a password in a log file.
pub fn redact(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    let Some((authority, tail)) = rest.split_once('@') else {
        return url.to_string();
    };
    let user = authority.split_once(':').map_or(authority, |(u, _)| u);
    format!("{scheme}://{user}:***@{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_printed_connection_string_has_no_password_in_it() {
        assert_eq!(
            redact("postgres://keeper:hunter2@db.internal:5432/onion"),
            "postgres://keeper:***@db.internal:5432/onion"
        );
        assert_eq!(
            redact("mysql://keeper@db.internal:3306/onion"),
            "mysql://keeper:***@db.internal:3306/onion"
        );
        // Nothing to hide, and nothing mangled.
        assert_eq!(redact("sqlite://keys.db"), "sqlite://keys.db");
        assert_eq!(redact("keys.db"), "keys.db");
    }

    #[test]
    fn a_row_carries_what_the_files_carry() {
        let find = Find {
            address: "abc.onion".into(),
            public_key: [3u8; 32],
            secret: [4u8; 64],
            filter: "abc".into(),
            score: 1.5,
            block: 2,
            offset: 8,
            source: "a test".into(),
        };
        let row = Row::of(&find, 100);
        assert_eq!(
            BASE64_STANDARD.decode(&row.secret_key).expect("base64"),
            crate::output::secret_file_bytes(&find.secret)
        );
        assert_eq!(
            BASE64_STANDARD.decode(&row.public_key).expect("base64"),
            crate::output::public_file_bytes(&find.public_key)
        );
    }
}
