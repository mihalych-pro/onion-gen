//! Search, recover, write, verify.
//!
//! The batch engine finds candidates matching a short prefix, the secret key is
//! recovered from the seed and the candidate's offset, the pair is written in
//! tor's layout, and the independent Python verifier confirms that the secret
//! really produces the address. The verifier shares no code with the generator,
//! so a mistake in the group arithmetic or in the offset bookkeeping surfaces
//! here rather than in someone's live service.

use onion_gen::batch::{BatchEngine, CHAIN_STEP};
use onion_gen::{curve, key, output};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn seed_from(n: u64) -> [u8; 32] {
    let mut b = [0u8; 32];
    let mut x = n.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(0xFACE);
    for chunk in b.chunks_mut(8) {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        chunk.copy_from_slice(&x.to_le_bytes());
    }
    b
}

/// Searches until `wanted` hits are written, then returns the output directory.
fn generate_hits(prefix: &str, wanted: usize) -> PathBuf {
    let out = std::env::temp_dir().join(format!("onion-gen-e2e-{}", std::process::id()));
    let _ = fs::remove_dir_all(&out);

    let mut engine = BatchEngine::new(1024);
    let mut written = 0usize;
    let mut seed_index = 0u64;

    'outer: while written < wanted {
        let seed = seed_from(seed_index);
        seed_index += 1;

        let sk = key::secret_scalar(&seed);
        let mut acc = curve::scalar_base_mult(&sk, &curve::Point::basepoint(), &curve::two_d());
        let mut base_offset = 0u64;

        // A bounded walk per seed keeps the test from running away if the
        // prefix turns out to be rarer than expected.
        for _ in 0..64 {
            engine.run(&mut acc);
            for i in 0..engine.batch_size() {
                let packed = engine.packed_with_sign(i);
                if !key::address(&packed).starts_with(prefix) {
                    continue;
                }
                let offset = base_offset + CHAIN_STEP * i as u64;
                let Some(secret) = key::expanded_secret_at_offset(&seed, offset) else {
                    // Clamping broke: the reference discards such a hit too.
                    continue;
                };
                output::write_key(&out, &packed, &secret).unwrap();
                written += 1;
                if written == wanted {
                    break 'outer;
                }
            }
            base_offset += CHAIN_STEP * engine.batch_size() as u64;
        }
    }

    out
}

#[test]
fn found_keys_pass_the_independent_verifier() {
    let out = generate_hits("ab", 3);

    let dirs: Vec<_> = fs::read_dir(&out)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    assert_eq!(dirs.len(), 3, "expected three hits");

    for d in &dirs {
        let name = d.file_name().unwrap().to_string_lossy().to_string();
        assert!(name.starts_with("ab"), "address {name} lacks the prefix");
        assert!(name.ends_with(".onion"));
    }

    let script =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/verify/verify-onion-address.py");
    let result = Command::new(&script).arg(&out).arg("--all").output();

    match result {
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            eprintln!("{stdout}");
            assert!(
                o.status.success(),
                "the verifier rejected the keys:\n{stdout}{}",
                String::from_utf8_lossy(&o.stderr)
            );
            assert!(
                stdout.contains("failed: 0"),
                "the verifier reported failures:\n{stdout}"
            );
        }
        // The path is baked in at build time, so a binary cross-compiled on
        // one machine and run on another will not find the script. Skipping
        // loudly beats failing: the verifier agreeing is checked wherever the
        // repository is present, which is every machine that builds.
        Err(e) if !script.exists() => {
            eprintln!(
                "SKIPPED: the verifier script is not at {} ({e}); \
                 this binary was built elsewhere",
                script.display()
            );
        }
        Err(e) => panic!("could not run {}: {e}", script.display()),
    }

    fs::remove_dir_all(&out).unwrap();
}
