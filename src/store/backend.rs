//! What every place keys can be kept has to be able to do.
//!
//! The four methods below are the whole contract, and they are deliberately
//! narrow: a batch of finds goes in, an address is asked about, and the thing
//! says what it is for a message. Everything a particular database needs —
//! which placeholder it uses, what it calls a floating-point column, how it
//! reports a duplicate — stays behind this line.
//!
//! Adding a fifth kind means writing one of these and one arm in
//! [`super::open`]. Nothing above this module changes.

use super::Find;
use std::io;

/// What a store is doing, for the metrics a master publishes.
///
/// Counted rather than measured: these are the numbers that answer "is the
/// database keeping up" and "is it still there", which is what an operator
/// watching a fleet needs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    /// Connections the pool holds, idle ones included. Always 1 for a file.
    pub connections: u32,
    /// Of those, how many are not in use.
    pub idle: u32,
    /// Rows this process has written. Rows, not finds: a repeat does not count.
    pub rows: u64,
    /// Bytes of key material written, before any encoding. The size of what
    /// would be lost, which is the number worth watching.
    pub bytes: u64,
    /// Batches committed.
    pub batches: u64,
    /// Batches that failed. A find is not lost by one — the error reaches the
    /// caller — but a store erroring under a fleet is worth an alert.
    pub failures: u64,
}

pub trait Backend: Send {
    /// One word naming the kind, for a metric label: `directory`, `sqlite`,
    /// `postgres`, `mysql`. Fixed strings, so a label can never carry a host
    /// name or anything else from a connection string.
    fn kind(&self) -> &'static str;

    /// Where finds are going, for the line a run prints and for a message
    /// about an address that is already there.
    fn describe(&self) -> String;

    /// Brings the schema up to date, creating it if this is the first run.
    ///
    /// Returns how many migrations it applied, which is what the startup line
    /// reports: a database that silently gained a column is a database nobody
    /// can reason about afterwards.
    fn migrate(&mut self) -> io::Result<usize>;

    /// Whether this address is already stored.
    ///
    /// Asked before a find is taken, so that a repeat is reported rather than
    /// counted twice.
    fn holds(&mut self, address: &str) -> io::Result<bool>;

    /// Writes a batch. An address already present is left as it is.
    fn write(&mut self, batch: &[Find]) -> io::Result<()>;

    fn stats(&self) -> Stats;
}
