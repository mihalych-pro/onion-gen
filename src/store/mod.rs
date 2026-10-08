//! Where found keys are kept: a directory per key, or rows in a database.
//!
//! Everything above this module sees one [`Store`]. Which kind it is follows
//! from the connection string, and what each kind has to do is the four
//! methods of [`backend::Backend`] — so a fifth kind is one file and one arm
//! of [`open`].
//!
//! Writing is batched whichever kind it is. A key costs a directory and three
//! files, and a master collecting from a fleet spends most of its time on
//! exactly that — measured at 162 keys a second, which a four-symbol filter on
//! one card overruns six times over. The same finds as rows, a thousand to a
//! transaction: 244 000 a second into SQLite, 32 000 into PostgreSQL, 22 700
//! into MySQL (`cargo bench --bench store-rate`).
//!
//! The batch is bounded twice: by how many finds are waiting and by how long
//! the oldest has waited. A find is therefore stored within a second of being
//! found, and a crash inside that second loses what is still in hand — which
//! is the trade this exists to make. Every ordinary exit flushes first.

pub mod backend;
mod directory;
mod mysql;
mod postgres;
mod sql;
mod sqlite;

pub use backend::Stats;

use backend::Backend;
use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Finds held before a flush is forced.
const MAX_PENDING: usize = 1000;
/// How long the oldest find waits before a flush is forced.
const MAX_WAIT: Duration = Duration::from_secs(1);

/// A find on its way to storage.
#[derive(Clone)]
pub struct Find {
    pub address: String,
    pub public_key: [u8; 32],
    pub secret: [u8; 64],
    /// The filter it matched, as the user wrote it.
    pub filter: String,
    pub score: f64,
    /// Where in the search this key sits. With the root seed the run was
    /// given, the two reproduce the key.
    pub block: u64,
    pub offset: u64,
    /// Which engine found it: a processor path, a device, or a worker.
    pub source: String,
}

pub struct Store {
    backend: Box<dyn Backend>,
    pending: Vec<Find>,
    /// Addresses waiting to be written. A find is checked against the store
    /// before it is taken, and the store does not yet know about these.
    waiting: HashSet<String>,
    oldest: Option<Instant>,
}

/// Opens whatever the connection string names.
///
/// The forms are the ones a Go program would take:
///
/// ```text
/// keys.db                                     a SQLite file
/// sqlite://keys.db                            the same, said plainly
/// postgres://user:pass@host:5432/dbname       PostgreSQL
/// postgresql://user:pass@host:5432/dbname     the same
/// mysql://user:pass@host:3306/dbname          MySQL
/// ```
///
/// A bare path is SQLite because that is what a path can be and nothing else;
/// anything carrying a scheme is taken at its word, so a typo in it is an
/// error rather than a file with a surprising name.
pub fn open(url: &str) -> io::Result<Store> {
    let backend: Box<dyn Backend> = match scheme_of(url) {
        Some("postgres" | "postgresql") => Box::new(postgres::Postgres::open(url)?),
        Some("mysql") => Box::new(mysql::MySql::open(url)?),
        Some("sqlite") => {
            let rest = url.trim_start_matches("sqlite://");
            // `sqlite:///abs/path` and `sqlite://relative` both mean a file.
            Box::new(sqlite::Sqlite::open(Path::new(rest))?)
        }
        Some(other) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "{other}: not a kind of store this knows. \
                     Use a path, sqlite://, postgres:// or mysql://"
                ),
            ))
        }
        None => Box::new(sqlite::Sqlite::open(Path::new(url))?),
    };
    let mut store = Store {
        backend,
        pending: Vec::new(),
        waiting: HashSet::new(),
        oldest: None,
    };
    store.backend.migrate()?;
    Ok(store)
}

/// The scheme of a connection string, if it has one.
///
/// A Windows path is the reason this is not `split_once(':')`: `C:\keys.db`
/// would otherwise look like a store of kind `C`.
fn scheme_of(url: &str) -> Option<&str> {
    let (scheme, _) = url.split_once("://")?;
    (!scheme.is_empty()
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+'))
    .then_some(scheme)
}

impl Store {
    /// A directory holding one subdirectory per key, in the layout tor reads.
    pub fn directory(dir: PathBuf) -> Self {
        Store {
            backend: Box::new(directory::Directory::new(dir)),
            pending: Vec::new(),
            waiting: HashSet::new(),
            oldest: None,
        }
    }

    /// Where finds are going, for the line the run prints. Never carries a
    /// password: a connection string is shown redacted.
    pub fn describe(&self) -> String {
        self.backend.describe()
    }

    /// Where a find of this address will end up, for a message about one that
    /// is already there.
    pub fn locate(&self, address: &str) -> String {
        let where_ = self.backend.describe();
        format!("{where_}: {address}")
    }

    /// Whether this address is already stored or already waiting to be.
    pub fn holds(&mut self, address: &str) -> io::Result<bool> {
        if self.waiting.contains(address) {
            return Ok(true);
        }
        self.backend.holds(address)
    }

    /// Takes a find. Flushes when the batch is full or the oldest has waited
    /// long enough.
    pub fn put(&mut self, find: Find) -> io::Result<()> {
        if self.pending.is_empty() {
            self.oldest = Some(Instant::now());
        }
        self.waiting.insert(find.address.clone());
        self.pending.push(find);
        if self.pending.len() >= MAX_PENDING || self.overdue() {
            return self.flush();
        }
        Ok(())
    }

    /// Whether the oldest find has waited longer than the window.
    pub fn overdue(&self) -> bool {
        self.oldest.is_some_and(|at| at.elapsed() >= MAX_WAIT)
    }

    /// Writes everything held.
    pub fn flush(&mut self) -> io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let batch = std::mem::take(&mut self.pending);
        self.oldest = None;
        let outcome = self.backend.write(&batch);
        // Cleared whatever happened: a find that failed to go down is not
        // going to succeed by being remembered as present.
        self.waiting.clear();
        outcome
    }

    pub fn stats(&self) -> Stats {
        self.backend.stats()
    }

    /// One word naming the kind, for a metric label.
    pub fn kind(&self) -> &'static str {
        self.backend.kind()
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        // A find in hand at the end of a run is a key nobody has. Losing it to
        // tidiness would be worse than any error this could report.
        if let Err(e) = self.flush() {
            eprintln!("onion-gen: could not write the last of the keys: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key;

    fn find(n: u8) -> Find {
        let mut public_key = [0u8; 32];
        public_key[0] = n;
        Find {
            address: key::hostname(&public_key),
            public_key,
            secret: [n; 64],
            filter: "test".to_string(),
            score: f64::from(n),
            block: u64::from(n),
            offset: 8,
            source: "a test".to_string(),
        }
    }

    fn temp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("onion-gen-store-{}-{name}", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    #[test]
    fn a_connection_string_picks_the_store() {
        assert_eq!(scheme_of("postgres://u:p@h/db"), Some("postgres"));
        assert_eq!(scheme_of("mysql://u@h/db"), Some("mysql"));
        assert_eq!(scheme_of("sqlite://keys.db"), Some("sqlite"));
        // A plain path is a file, on either kind of system.
        assert_eq!(scheme_of("keys.db"), None);
        assert_eq!(scheme_of("/var/lib/onion-gen/keys.db"), None);
        assert_eq!(scheme_of(r"C:\keys.db"), None);
    }

    #[test]
    fn an_unknown_scheme_is_refused_rather_than_guessed() {
        match open("oracle://u:p@h/db") {
            Err(e) => assert!(e.to_string().contains("oracle"), "{e}"),
            Ok(_) => panic!("that is not a store this has"),
        }
    }

    #[test]
    fn a_database_keeps_every_find() {
        let path = temp("rows.db");
        {
            let mut store = open(path.to_str().expect("a path")).expect("a database");
            store.put(find(1)).expect("put");
            store.put(find(2)).expect("put");
            store.flush().expect("flush");
            assert_eq!(store.stats().rows, 2);
        }
        let mut again = open(path.to_str().expect("a path")).expect("reopen");
        assert!(again.holds(&find(1).address).expect("ask"));
        assert!(again.holds(&find(2).address).expect("ask"));
        drop(again);
        let _ = std::fs::remove_file(&path);
    }

    /// A repeat has to be visible to the caller before it is taken: that is
    /// what keeps the find count honest.
    #[test]
    fn a_repeat_is_seen_whether_it_is_waiting_or_stored() {
        let path = temp("repeat.db");
        let mut store = open(path.to_str().expect("a path")).expect("a database");
        let f = find(4);
        assert!(!store.holds(&f.address).expect("ask"));
        store.put(f.clone()).expect("put");
        assert!(store.holds(&f.address).expect("ask"), "still in hand");
        store.flush().expect("flush");
        assert!(store.holds(&f.address).expect("ask"), "now on disk");
        drop(store);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_same_address_twice_is_one_row() {
        let path = temp("once.db");
        let mut store = open(path.to_str().expect("a path")).expect("a database");
        store.put(find(5)).expect("put");
        store.flush().expect("flush");
        store.put(find(5)).expect("put");
        store.flush().expect("flush");
        drop(store);

        let mut again = open(path.to_str().expect("a path")).expect("reopen");
        assert!(again.holds(&find(5).address).expect("ask"));
        drop(again);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_full_batch_flushes_without_being_asked() {
        let path = temp("full.db");
        let mut store = open(path.to_str().expect("a path")).expect("a database");
        for n in 0..MAX_PENDING {
            let mut f = find(0);
            f.public_key[1] = (n >> 8) as u8;
            f.public_key[2] = n as u8;
            f.address = key::hostname(&f.public_key);
            store.put(f).expect("put");
        }
        assert_eq!(
            store.stats().rows,
            MAX_PENDING as u64,
            "it should have gone alone"
        );
        drop(store);
        let _ = std::fs::remove_file(&path);
    }

    /// Dropping a store must not lose what it is holding.
    #[test]
    fn what_is_in_hand_is_written_at_the_end() {
        let path = temp("drop.db");
        {
            let mut store = open(path.to_str().expect("a path")).expect("a database");
            store.put(find(9)).expect("put");
        }
        let mut again = open(path.to_str().expect("a path")).expect("reopen");
        assert!(again.holds(&find(9).address).expect("ask"));
        drop(again);
        let _ = std::fs::remove_file(&path);
    }

    /// Running twice against the same file must not try to build the schema
    /// again, and must not lose what the first run put there.
    #[test]
    fn migrations_run_once_and_only_once() {
        let path = temp("migrate.db");
        {
            let mut store = open(path.to_str().expect("a path")).expect("a database");
            store.put(find(6)).expect("put");
            store.flush().expect("flush");
        }
        {
            let mut store = open(path.to_str().expect("a path")).expect("reopen");
            assert!(store.holds(&find(6).address).expect("ask"), "still there");
            store.put(find(7)).expect("put");
            store.flush().expect("flush");
        }
        let _ = std::fs::remove_file(&path);
    }

    /// The directory backend has to go on producing what tor reads.
    #[test]
    fn a_directory_still_holds_the_three_files() {
        let dir = temp("files.dir");
        let f = find(6);
        {
            let mut store = Store::directory(dir.clone());
            store.put(f.clone()).expect("put");
        }
        let target = dir.join(&f.address);
        assert_eq!(
            std::fs::read(target.join("hs_ed25519_secret_key")).expect("secret"),
            crate::output::secret_file_bytes(&f.secret)
        );
        assert_eq!(
            std::fs::read_to_string(target.join("hostname")).expect("hostname"),
            format!("{}\n", f.address)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
