//! Finds that have not reached the master yet.
//!
//! A master is unreachable for minutes; a find that took a week of searching
//! will not come round again. So nothing is dropped — not on a full buffer, not
//! on a restart, not on a half-written file.
//!
//! One file per find rather than one log: acknowledging is then a single
//! unlink, and a file interrupted mid-write is detectable on its own without
//! casting doubt on the rest.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::proto::Found;

/// The unsent finds, on disk.
pub struct Buffer {
    dir: PathBuf,
    /// Past this many, say so. Not a reason to discard anything.
    limit: usize,
    warned: bool,
}

impl Buffer {
    pub fn new(dir: PathBuf, limit: usize) -> Buffer {
        Buffer {
            dir,
            limit,
            warned: false,
        }
    }

    /// Writes a find down before anything is attempted with it.
    ///
    /// Through a temporary name and a rename, so that a file under the real
    /// name is always complete. A find is identified by where it is, which is
    /// unique, so re-recording the same one overwrites rather than duplicates.
    pub fn keep(&mut self, find: &Found) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let name = format!("{}-{}-{}.json", find.lease, find.block, find.offset);
        let final_path = self.dir.join(&name);
        let temp = self.dir.join(format!(".{name}.part"));
        fs::write(&temp, serde_json::to_vec(find)?)?;
        fs::rename(&temp, &final_path)?;

        let held = self.len()?;
        if held > self.limit && !self.warned {
            self.warned = true;
            eprintln!(
                "onion-gen: {held} unsent find(s) are waiting, past the limit of {}. \
                 Nothing is discarded; the master has been unreachable for a while.",
                self.limit
            );
        }
        Ok(())
    }

    /// Everything waiting, oldest name first so the order is stable.
    pub fn pending(&self) -> io::Result<Vec<(PathBuf, Found)>> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let mut out = Vec::new();
        for entry in entries {
            let path = entry?.path();
            // A `.part` is a write that did not finish; the find it holds was
            // never acknowledged, so it will be re-recorded when found again or
            // is simply lost with the crash that made it. Skipping it is right;
            // parsing it would be guessing.
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            match fs::read(&path).map(|bytes| serde_json::from_slice::<Found>(&bytes)) {
                Ok(Ok(find)) => out.push((path, find)),
                // A file that will not parse is kept, not deleted: it is
                // evidence, and deleting it would be the one thing this module
                // exists to never do.
                _ => eprintln!(
                    "onion-gen: {} is not a readable find and is being left alone",
                    path.display()
                ),
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }

    /// Forgets a find, once the master has said it has it.
    pub fn acknowledge(&mut self, path: &Path) -> io::Result<()> {
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    pub fn len(&self) -> io::Result<usize> {
        Ok(self.pending()?.len())
    }

    pub fn is_empty(&self) -> io::Result<bool> {
        Ok(self.len()? == 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "onion-gen-buffer-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&d);
        d
    }

    fn find(lease: u64, block: u64, offset: u64) -> Found {
        Found {
            lease,
            block,
            offset,
        }
    }

    #[test]
    fn a_find_survives_being_written_and_read_back() {
        let dir = temp("roundtrip");
        let mut b = Buffer::new(dir.clone(), 8);
        b.keep(&find(1, 400, 88192)).expect("keep");

        // A fresh buffer over the same directory, as a restart would see it.
        let again = Buffer::new(dir.clone(), 8);
        let pending = again.pending().expect("pending");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].1.offset, 88192);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_acknowledged_find_is_forgotten_and_the_rest_are_not() {
        let dir = temp("ack");
        let mut b = Buffer::new(dir.clone(), 8);
        for i in 0..3 {
            b.keep(&find(1, 400 + i, i)).expect("keep");
        }
        let pending = b.pending().expect("pending");
        assert_eq!(pending.len(), 3);

        b.acknowledge(&pending[1].0).expect("ack");
        let left: Vec<u64> = b
            .pending()
            .expect("pending")
            .iter()
            .map(|(_, f)| f.block)
            .collect();
        assert_eq!(left, vec![400, 402]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn acknowledging_twice_is_not_an_error() {
        let dir = temp("twice");
        let mut b = Buffer::new(dir.clone(), 8);
        b.keep(&find(1, 400, 0)).expect("keep");
        let path = b.pending().expect("pending")[0].0.clone();
        b.acknowledge(&path).expect("first");
        b.acknowledge(&path).expect("second must not fail");
        assert!(b.is_empty().expect("empty"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_same_find_recorded_twice_is_one_file() {
        let dir = temp("dedup");
        let mut b = Buffer::new(dir.clone(), 8);
        b.keep(&find(1, 400, 7)).expect("keep");
        b.keep(&find(1, 400, 7)).expect("keep again");
        assert_eq!(
            b.len().expect("len"),
            1,
            "a find is identified by where it is"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// The rule the module exists for: a full buffer says so and keeps going.
    #[test]
    fn passing_the_limit_discards_nothing() {
        let dir = temp("limit");
        let mut b = Buffer::new(dir.clone(), 2);
        for i in 0..6 {
            b.keep(&find(1, 400, i)).expect("keep");
        }
        assert_eq!(
            b.len().expect("len"),
            6,
            "a find that took a week will not come round again"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// A write interrupted by a crash leaves a partial file. It must not be
    /// read as a find, and it must not stop the readable ones being sent.
    #[test]
    fn a_half_written_file_is_stepped_over() {
        let dir = temp("partial");
        let mut b = Buffer::new(dir.clone(), 8);
        b.keep(&find(1, 400, 1)).expect("keep");
        fs::write(dir.join(".7-1-1.json.part"), b"{\"lease\":7,\"bl").expect("partial");

        let pending = b.pending().expect("pending");
        assert_eq!(pending.len(), 1, "only the complete find");
        assert_eq!(pending[0].1.block, 400);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_directory_is_simply_empty() {
        let b = Buffer::new(temp("absent"), 8);
        assert!(b.is_empty().expect("no directory is not an error"));
    }
}
