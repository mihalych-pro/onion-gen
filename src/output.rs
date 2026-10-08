//! Writing a found key pair in the layout tor expects.
//!
//! A directory named after the address holds three files. Only the secret key
//! is strictly required — tor regenerates the other two — but all three are
//! written so that a mismatch is visible without starting tor.

use crate::key;
use std::fs;
#[cfg(unix)]
use std::io::Write;
use std::io::{self};
use std::path::{Path, PathBuf};

/// The prefix of `hs_ed25519_secret_key`: 29 ASCII bytes plus three zeros.
const SECRET_PREFIX: &[u8; 32] = b"== ed25519v1-secret: type0 ==\0\0\0";
/// The prefix of `hs_ed25519_public_key`, same shape.
const PUBLIC_PREFIX: &[u8; 32] = b"== ed25519v1-public: type0 ==\0\0\0";

/// Mode for the key directory. Tor refuses to start if the directory is
/// readable by anyone else.
const DIR_MODE: u32 = 0o700;
/// Mode for the secret key. A secret key readable by other users of the machine
/// is a compromised key.
///
/// Unix only: elsewhere the file is written without it, and
/// [`enforces_permissions`] is how a caller finds that out.
#[cfg(unix)]
const SECRET_MODE: u32 = 0o600;

/// What happened to one hit.
#[derive(Debug, PartialEq, Eq)]
pub enum Written {
    /// The directory was created and all three files written.
    Created(PathBuf),
    /// A directory of that name already existed and was left untouched.
    Skipped(PathBuf),
}

/// Writes one hit under `dir`.
///
/// An existing directory is never overwritten: a name collision means either a
/// repeat of the same address or somebody else's data, and silently replacing
/// either is unacceptable.
pub fn write_key(
    dir: &Path,
    public_key: &[u8; 32],
    expanded_secret: &[u8; 64],
) -> io::Result<Written> {
    let hostname = key::hostname(public_key);
    let target = dir.join(&hostname);

    fs::create_dir_all(dir)?;
    match fs::create_dir(&target) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            return Ok(Written::Skipped(target));
        }
        Err(e) => return Err(e),
    }
    set_mode(&target, DIR_MODE)?;

    let secret = secret_file_bytes(expanded_secret);
    write_secret(&target.join("hs_ed25519_secret_key"), &secret)?;

    let public = public_file_bytes(public_key);
    fs::write(target.join("hs_ed25519_public_key"), &public)?;

    fs::write(target.join("hostname"), format!("{hostname}\n"))?;

    Ok(Written::Created(target))
}

/// The contents of `hs_ed25519_secret_key`: the prefix and the expanded key.
pub fn secret_file_bytes(expanded_secret: &[u8; 64]) -> Vec<u8> {
    let mut v = Vec::with_capacity(96);
    v.extend_from_slice(SECRET_PREFIX);
    v.extend_from_slice(expanded_secret);
    v
}

/// The contents of `hs_ed25519_public_key`: the prefix and the key.
pub fn public_file_bytes(public_key: &[u8; 32]) -> Vec<u8> {
    let mut v = Vec::with_capacity(64);
    v.extend_from_slice(PUBLIC_PREFIX);
    v.extend_from_slice(public_key);
    v
}

/// Narrows an existing file to its owner. Used for a database holding secret
/// keys, which `sqlite` creates under the process umask.
#[cfg(unix)]
pub fn restrict(path: &Path) -> io::Result<()> {
    set_mode(path, SECRET_MODE)
}

#[cfg(not(unix))]
pub fn restrict(_path: &Path) -> io::Result<()> {
    Ok(())
}

/// Writes the secret key, creating it with restrictive permissions from the
/// outset rather than tightening them afterwards — otherwise the file is
/// briefly readable by everyone.
#[cfg(unix)]
fn write_secret(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(SECRET_MODE)
        .open(path)?;
    f.write_all(bytes)?;
    f.sync_all()
}

#[cfg(not(unix))]
fn write_secret(path: &Path, bytes: &[u8]) -> io::Result<()> {
    fs::write(path, bytes)
}

/// Sets a POSIX mode explicitly, because `create_dir` applies the process
/// `umask` and a permissive umask would leave the directory open.
#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> io::Result<()> {
    Ok(())
}

/// Whether this platform applies the POSIX permissions the format expects.
/// Callers warn once when it does not, rather than leaving the user to assume
/// the secret is protected.
pub const fn enforces_permissions() -> bool {
    cfg!(unix)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key;

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

    fn temp_dir(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("onion-gen-test-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        p
    }

    fn write_sample(dir: &Path, n: u64) -> (Written, [u8; 32]) {
        let seed = seed_from(n);
        let pk = key::public_key(&seed);
        let secret = key::expanded_secret_key(&seed);
        (write_key(dir, &pk, &secret).unwrap(), pk)
    }

    #[test]
    fn writes_three_files_in_a_directory_named_after_the_address() {
        let dir = temp_dir("layout");
        let (written, pk) = write_sample(&dir, 1);
        let Written::Created(path) = written else {
            panic!("expected a fresh directory");
        };

        assert_eq!(path.file_name().unwrap(), key::hostname(&pk).as_str());

        let hostname = fs::read_to_string(path.join("hostname")).unwrap();
        assert_eq!(hostname, format!("{}\n", key::hostname(&pk)));
        assert_eq!(
            hostname.trim_end(),
            path.file_name().unwrap().to_str().unwrap(),
            "hostname must equal the directory name"
        );

        let secret = fs::read(path.join("hs_ed25519_secret_key")).unwrap();
        assert_eq!(secret.len(), 96);
        assert_eq!(&secret[..32], SECRET_PREFIX);
        assert_eq!(&secret[32..], &key::expanded_secret_key(&seed_from(1))[..]);

        let public = fs::read(path.join("hs_ed25519_public_key")).unwrap();
        assert_eq!(public.len(), 64);
        assert_eq!(&public[..32], PUBLIC_PREFIX);
        assert_eq!(&public[32..], &pk[..]);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn permissions_hold_under_a_permissive_umask() {
        use std::os::unix::fs::PermissionsExt;

        let dir = temp_dir("modes");
        let (written, _) = write_sample(&dir, 2);
        let Written::Created(path) = written else {
            panic!("expected a fresh directory");
        };

        let dir_mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, DIR_MODE, "the key directory must be 0700");

        let secret_mode = fs::metadata(path.join("hs_ed25519_secret_key"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(secret_mode, SECRET_MODE, "the secret key must be 0600");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_existing_directory_is_left_untouched() {
        let dir = temp_dir("collision");
        let (first, pk) = write_sample(&dir, 3);
        let Written::Created(path) = first else {
            panic!("expected a fresh directory");
        };

        // Mark the directory so any overwrite is detectable.
        fs::write(path.join("hostname"), "sentinel\n").unwrap();

        let secret = key::expanded_secret_key(&seed_from(3));
        let again = write_key(&dir, &pk, &secret).unwrap();
        assert_eq!(again, Written::Skipped(path.clone()));
        assert_eq!(
            fs::read_to_string(path.join("hostname")).unwrap(),
            "sentinel\n",
            "existing files must survive"
        );

        fs::remove_dir_all(&dir).unwrap();
    }
}
