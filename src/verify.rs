//! Checking a found key, and saying what it is.
//!
//! Four questions, in the order that matters. Do the files parse; does the
//! secret key produce the public key; does the public key produce the address
//! the directory is named after; and does an implementation that is not ours
//! agree. The last one is the point: our own arithmetic checking itself proves
//! only that it is consistent, and a defect that reaches both sides of that
//! comparison survives it.
//!
//! The outside opinion comes from `tor-hscrypto`, the onion-service types the
//! Arti project uses in the Tor client it ships. Nothing here touches the
//! network — there is no network code in this module and none in that crate's
//! address handling, so a key is checked without its address ever being said
//! out loud to anybody.

use crate::key;
use crate::score;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// The prefix of `hs_ed25519_secret_key`.
const SECRET_PREFIX: &[u8; 32] = b"== ed25519v1-secret: type0 ==\0\0\0";
/// The prefix of `hs_ed25519_public_key`.
const PUBLIC_PREFIX: &[u8; 32] = b"== ed25519v1-public: type0 ==\0\0\0";

/// What one key turned out to be.
pub struct Report {
    pub path: PathBuf,
    pub address: String,
    /// Checks that passed, in the order they were made.
    pub passed: Vec<&'static str>,
    /// The first thing that was wrong, if anything was.
    pub fault: Option<String>,
    pub score: f64,
    /// Measured share of addresses reaching that score: one in this many.
    pub one_in: f64,
    /// Whether the files are readable only by their owner. `None` off unix.
    pub private: Option<bool>,
    /// Whether the public key file was there and matched. It is optional —
    /// tor regenerates it — so its absence is not a fault.
    pub public_key_file: bool,
    pub hostname_file: bool,
}

impl Report {
    pub fn ok(&self) -> bool {
        self.fault.is_none()
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mark = if self.ok() { "ok" } else { "FAILED" };
        writeln!(f, "{mark}  {}", self.address)?;
        writeln!(f, "      {}", self.path.display())?;
        if let Some(why) = &self.fault {
            writeln!(f, "      fault: {why}")?;
        }
        writeln!(f, "      checks: {}", self.passed.join(", "))?;
        writeln!(
            f,
            "      score {:.1}, about one address in {}",
            self.score,
            crate::run::human_count(self.one_in)
        )?;
        let mut notes: Vec<String> = Vec::new();
        if !self.public_key_file {
            notes.push("no public key file (tor regenerates it)".into());
        }
        if !self.hostname_file {
            notes.push("no hostname file (tor regenerates it)".into());
        }
        match self.private {
            Some(true) => {}
            Some(false) => notes.push("readable by other users — tor will refuse to start".into()),
            None => {}
        }
        if !notes.is_empty() {
            writeln!(f, "      note: {}", notes.join("; "))?;
        }
        Ok(())
    }
}

/// Checks one key directory.
pub fn one(dir: &Path) -> io::Result<Report> {
    let secret_path = dir.join("hs_ed25519_secret_key");
    let raw = fs::read(&secret_path)?;

    let mut report = Report {
        path: dir.to_path_buf(),
        address: String::new(),
        passed: Vec::new(),
        fault: None,
        score: 0.0,
        one_in: 1.0,
        private: file_is_private(&secret_path),
        public_key_file: false,
        hostname_file: false,
    };

    if raw.len() != 96 || &raw[..32] != SECRET_PREFIX {
        report.fault = Some(format!(
            "hs_ed25519_secret_key is {} bytes or has the wrong header; a key file is 96 bytes \
             beginning \"== ed25519v1-secret: type0 ==\"",
            raw.len()
        ));
        return Ok(report);
    }
    report.passed.push("file shape");

    let mut secret = [0u8; 64];
    secret.copy_from_slice(&raw[32..]);

    // Clamping is not cosmetic: an unclamped scalar is a key tor will not use,
    // and it is the one defect that a round trip through our own code would
    // not notice.
    if secret[0] & 7 != 0 || secret[31] & 64 == 0 || secret[31] & 128 != 0 {
        report.fault = Some("the secret scalar is not clamped as ed25519 requires".into());
        return Ok(report);
    }
    report.passed.push("clamping");

    let public = key::public_key_from_secret(&secret);
    if !key::secret_produces(&secret, &public) {
        report.fault = Some("the secret key does not produce its own public key".into());
        return Ok(report);
    }
    report.passed.push("secret produces public");

    let hostname = key::hostname(&public);
    report.address = hostname.clone();

    // The public key file is optional, but when it is there it has to agree.
    match fs::read(dir.join("hs_ed25519_public_key")) {
        Ok(bytes) => {
            report.public_key_file = true;
            if bytes.len() != 64 || &bytes[..32] != PUBLIC_PREFIX || bytes[32..] != public[..] {
                report.fault = Some("hs_ed25519_public_key does not match the secret key".into());
                return Ok(report);
            }
            report.passed.push("public key file");
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }

    match fs::read_to_string(dir.join("hostname")) {
        Ok(text) => {
            report.hostname_file = true;
            if text.trim() != hostname {
                report.fault = Some(format!(
                    "hostname says {} but the key gives {hostname}",
                    text.trim()
                ));
                return Ok(report);
            }
            report.passed.push("hostname file");
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }

    // The directory name, when it looks like an address, is a fourth copy of
    // the same claim and worth checking against the other three.
    if let Some(name) = dir.file_name().and_then(|n| n.to_str()) {
        if name.ends_with(".onion") && name != hostname {
            report.fault = Some(format!(
                "the directory is named {name} but the key gives {hostname}"
            ));
            return Ok(report);
        }
    }

    if let Err(why) = agrees_with_arti(&hostname, &public) {
        report.fault = Some(why);
        return Ok(report);
    }
    report.passed.push("arti agrees");

    let scored = score::score(hostname.trim_end_matches(".onion").as_bytes(), None);
    // `+ 0.0` so that a score of exactly zero never prints as "-0.0".
    report.score = scored.total + 0.0;
    report.one_in = score::one_in(scored.total);
    Ok(report)
}

/// Asks Arti's onion-service types whether this address is this key.
///
/// Two directions, because either alone would miss something: the address has
/// to parse to the key we hold, and the key has to render back to the address.
fn agrees_with_arti(hostname: &str, public: &[u8; 32]) -> Result<(), String> {
    use tor_hscrypto::pk::{HsId, HsIdKey};

    let id =
        HsId::from_str(hostname).map_err(|e| format!("arti will not parse the address: {e}"))?;
    let parsed = HsIdKey::try_from(id)
        .map_err(|e| format!("arti will not take the address as a key: {e}"))?;
    if parsed.as_ref().as_bytes() != public {
        return Err("arti reads a different key out of this address".into());
    }
    let rendered = safelog::DisplayRedacted::display_unredacted(&HsId::from(parsed)).to_string();
    if rendered != hostname {
        return Err(format!("arti writes this key as {rendered}"));
    }
    Ok(())
}

/// Checks a directory of key directories, or a single one.
///
/// Sorted, so that two runs over the same tree produce the same output and a
/// difference between them is a difference in the keys.
pub fn tree(root: &Path) -> io::Result<Vec<Report>> {
    if root.join("hs_ed25519_secret_key").exists() {
        return Ok(vec![one(root)?]);
    }
    let mut dirs: Vec<PathBuf> = fs::read_dir(root)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.join("hs_ed25519_secret_key").exists())
        .collect();
    dirs.sort();
    dirs.iter().map(|d| one(d)).collect()
}

#[cfg(unix)]
fn file_is_private(path: &Path) -> Option<bool> {
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::metadata(path).ok()?.permissions().mode();
    Some(mode & 0o077 == 0)
}

#[cfg(not(unix))]
fn file_is_private(_path: &Path) -> Option<bool> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output;

    fn seed_from(n: u64) -> [u8; 32] {
        let mut b = [0u8; 32];
        let mut x = n.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(0x5150);
        for chunk in b.chunks_mut(8) {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            chunk.copy_from_slice(&x.to_le_bytes());
        }
        b
    }

    fn written(tag: &str, n: u64) -> (PathBuf, PathBuf) {
        let root =
            std::env::temp_dir().join(format!("onion-gen-verify-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let seed = seed_from(n);
        let pk = key::public_key(&seed);
        let secret = key::expanded_secret_key(&seed);
        output::write_key(&root, &pk, &secret).expect("write");
        (root.clone(), root.join(key::hostname(&pk)))
    }

    #[test]
    fn a_key_we_wrote_passes_every_check() {
        let (root, dir) = written("good", 1);
        let r = one(&dir).expect("read");
        assert!(r.ok(), "{:?}", r.fault);
        assert!(r.passed.contains(&"arti agrees"), "{:?}", r.passed);
        assert!(r.passed.contains(&"secret produces public"));
        assert!(r.public_key_file && r.hostname_file);
        assert_eq!(r.address, dir.file_name().unwrap().to_str().unwrap());
        let _ = fs::remove_dir_all(&root);
    }

    /// The check that matters: one flipped byte in the secret must not pass.
    #[test]
    fn a_tampered_secret_key_is_caught() {
        let (root, dir) = written("tampered", 2);
        let path = dir.join("hs_ed25519_secret_key");
        let mut bytes = fs::read(&path).expect("read");
        bytes[40] ^= 0x01;
        fs::write(&path, &bytes).expect("write");

        let r = one(&dir).expect("read");
        assert!(!r.ok(), "a changed key must not verify");
        let _ = fs::remove_dir_all(&root);
    }

    /// A hostname saying one thing and a key saying another is the failure
    /// that looks most like success.
    #[test]
    fn a_hostname_that_disagrees_is_caught() {
        let (root, dir) = written("hostname", 3);
        fs::write(
            dir.join("hostname"),
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaad.onion\n",
        )
        .expect("write");
        let r = one(&dir).expect("read");
        assert!(!r.ok());
        assert!(r.fault.as_deref().unwrap_or("").contains("hostname"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_truncated_file_is_refused_rather_than_misread() {
        let (root, dir) = written("short", 4);
        fs::write(dir.join("hs_ed25519_secret_key"), b"too short").expect("write");
        let r = one(&dir).expect("read");
        assert!(!r.ok());
        assert!(r.fault.as_deref().unwrap_or("").contains("96 bytes"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_whole_tree_is_checked_and_sorted() {
        let root =
            std::env::temp_dir().join(format!("onion-gen-verify-tree-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for n in 10..14u64 {
            let seed = seed_from(n);
            output::write_key(
                &root,
                &key::public_key(&seed),
                &key::expanded_secret_key(&seed),
            )
            .expect("write");
        }
        let reports = tree(&root).expect("walk");
        assert_eq!(reports.len(), 4);
        assert!(reports.iter().all(|r| r.ok()));
        let names: Vec<&String> = reports.iter().map(|r| &r.address).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted, "output must not depend on directory order");
        let _ = fs::remove_dir_all(&root);
    }

    /// Given a single key directory rather than a parent, it checks that one.
    #[test]
    fn a_single_directory_is_accepted_too() {
        let (root, dir) = written("single", 5);
        assert_eq!(tree(&dir).expect("walk").len(), 1);
        let _ = fs::remove_dir_all(&root);
    }
}
