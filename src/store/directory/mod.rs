//! Keys as a directory each, in the layout tor reads. The default.

use super::backend::{Backend, Stats};
use super::Find;
use crate::output::{self, Written};
use std::io;
use std::path::PathBuf;

pub struct Directory {
    root: PathBuf,
    rows: u64,
    bytes: u64,
    batches: u64,
    failures: u64,
}

impl Directory {
    pub fn new(root: PathBuf) -> Self {
        Directory {
            root,
            rows: 0,
            bytes: 0,
            batches: 0,
            failures: 0,
        }
    }
}

impl Backend for Directory {
    fn kind(&self) -> &'static str {
        "directory"
    }

    fn describe(&self) -> String {
        self.root.display().to_string()
    }

    /// Nothing to migrate: the schema is the file system's.
    fn migrate(&mut self) -> io::Result<usize> {
        Ok(0)
    }

    fn holds(&mut self, address: &str) -> io::Result<bool> {
        Ok(self.root.join(address).exists())
    }

    fn write(&mut self, batch: &[Find]) -> io::Result<()> {
        for find in batch {
            match output::write_key(&self.root, &find.public_key, &find.secret) {
                Ok(Written::Created(_)) => {
                    self.rows += 1;
                    self.bytes += 96 + 64;
                }
                Ok(Written::Skipped(_)) => {}
                Err(e) => {
                    self.failures += 1;
                    return Err(e);
                }
            }
        }
        self.batches += 1;
        Ok(())
    }

    fn stats(&self) -> Stats {
        Stats {
            // A file system is not connected to, but reporting nothing would
            // make the metric ambiguous: zero would mean both "no store" and
            // "a store that does not connect".
            connections: 1,
            idle: 1,
            rows: self.rows,
            bytes: self.bytes,
            batches: self.batches,
            failures: self.failures,
        }
    }
}
