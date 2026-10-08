//! Byte-for-byte compatibility with the reference implementation's output.
//!
//! The test reads key directories produced by `mkp224o`, feeds the same key
//! material through our writer and compares the resulting files byte for byte.
//! This is the acceptance criterion for the output layout: tor has to accept
//! our files, and the reference's files are known to be accepted.
//!
//! The directory holding `*.onion` subdirectories is taken from
//! `ONION_GEN_REFERENCE_KEYS`; without it the test reports that it was skipped,
//! because the reference output is not part of the repository.

use onion_gen::{key, output};
use std::fs;
use std::path::PathBuf;

fn reference_dirs() -> Vec<PathBuf> {
    let Ok(root) = std::env::var("ONION_GEN_REFERENCE_KEYS") else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p.to_string_lossy().ends_with(".onion"))
        .collect()
}

#[test]
fn our_files_match_the_reference_byte_for_byte() {
    let dirs = reference_dirs();
    if dirs.is_empty() {
        eprintln!("skipped: set ONION_GEN_REFERENCE_KEYS to a directory of mkp224o output");
        return;
    }

    let out = std::env::temp_dir().join(format!("onion-gen-compat-{}", std::process::id()));
    let _ = fs::remove_dir_all(&out);

    let mut checked = 0usize;
    for dir in &dirs {
        let their_secret = fs::read(dir.join("hs_ed25519_secret_key")).unwrap();
        let their_public = fs::read(dir.join("hs_ed25519_public_key")).unwrap();
        let their_hostname = fs::read(dir.join("hostname")).unwrap();

        let mut expanded = [0u8; 64];
        expanded.copy_from_slice(&their_secret[32..]);
        let mut public_key = [0u8; 32];
        public_key.copy_from_slice(&their_public[32..]);

        // The address we derive must match the one the reference wrote.
        assert_eq!(
            key::hostname(&public_key).as_bytes(),
            &their_hostname[..their_hostname.len() - 1],
            "address mismatch for {}",
            dir.display()
        );

        let written = output::write_key(&out, &public_key, &expanded).unwrap();
        let output::Written::Created(ours) = written else {
            panic!("expected a fresh directory for {}", dir.display());
        };

        for name in ["hs_ed25519_secret_key", "hs_ed25519_public_key", "hostname"] {
            assert_eq!(
                fs::read(ours.join(name)).unwrap(),
                fs::read(dir.join(name)).unwrap(),
                "{name} differs for {}",
                dir.display()
            );
        }
        checked += 1;
    }

    fs::remove_dir_all(&out).unwrap();
    assert!(checked > 0);
    eprintln!("compared {checked} key directories against the reference");
}
