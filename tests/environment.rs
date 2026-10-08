//! Settings that arrive through the environment rather than the command line.
//!
//! The seed is the reason this file exists. It is a secret — every key of the
//! run comes from it — and a process's command line is readable by other users
//! on most systems, so the environment is the way it is meant to be passed.
//! A unit test with a stand-in lookup proves the wiring; only a real process
//! with a real environment proves the variable.

use std::process::Command;

fn binary() -> std::path::PathBuf {
    let mut path = std::env::current_exe().expect("a path to this test");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.push(if cfg!(windows) {
        "onion-gen.exe"
    } else {
        "onion-gen"
    });
    path
}

const SEED: &str = "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";

/// Runs a short search and returns the addresses it printed, sorted.
fn addresses(name: &str, env: &[(&str, &str)], args: &[&str]) -> Vec<String> {
    let dir = std::env::temp_dir().join(format!("onion-gen-env-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut command = Command::new(binary());
    command.args([
        "--compute",
        "cpu",
        "-q",
        "-t",
        "1",
        // Named, so that the run enumerates in a fixed order. Left unsaid the
        // program measures the machine and picks a batch size, and a different
        // size walks the same space in a different order - every key is still
        // correct, but which two come first is no longer fixed, and this test
        // compares them by name.
        "--batch-size",
        "8192",
        "-n",
        "2",
        "-d",
        dir.to_str().expect("a printable path"),
        "-F",
        "aa",
    ]);
    command.args(args);
    for (key, value) in env {
        command.env(key, value);
    }
    let run = command.output().expect("the binary runs");
    assert!(
        run.status.success(),
        "the run failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let mut found: Vec<String> = String::from_utf8_lossy(&run.stdout)
        .lines()
        .map(str::to_string)
        .collect();
    found.sort();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(found.len(), 2, "expected two addresses, got {found:?}");
    found
}

/// The variable and the flag name the same thing, so they have to give the
/// same search.
#[test]
fn the_seed_arrives_through_the_environment() {
    let by_env = addresses("seed-env", &[("ONION_GEN_SEED", SEED)], &[]);
    let by_flag = addresses("seed-flag", &[], &["--seed", SEED]);
    assert_eq!(by_env, by_flag);
}

/// And the block to start at, so that the whole of a run's place can be kept
/// out of the command line.
#[test]
fn the_starting_block_arrives_through_the_environment() {
    let from_start = addresses("block-zero", &[("ONION_GEN_SEED", SEED)], &[]);
    let further_on = addresses(
        "block-64",
        &[("ONION_GEN_SEED", SEED), ("ONION_GEN_FROM_BLOCK", "64")],
        &[],
    );
    assert_ne!(
        from_start, further_on,
        "starting at block 64 searched the same place as block 0"
    );

    // And it is the same place the flag reaches.
    let by_flag = addresses(
        "block-flag",
        &[("ONION_GEN_SEED", SEED)],
        &["--from-block", "64"],
    );
    assert_eq!(further_on, by_flag);
}

/// A flag outranks the environment, as every other setting does.
#[test]
fn a_flag_outranks_the_variable() {
    let other = "abababababababababababababababababababababababababababababababab";
    let by_flag = addresses("override", &[("ONION_GEN_SEED", SEED)], &["--seed", other]);
    let alone = addresses("override-alone", &[], &["--seed", other]);
    assert_eq!(
        by_flag, alone,
        "the variable won where the flag should have"
    );
}

/// A seed that is not a seed is refused before any work, whichever way it came.
#[test]
fn a_malformed_seed_is_refused() {
    for (name, env, args) in [
        ("short", vec![("ONION_GEN_SEED", "abcd")], vec![]),
        (
            "not-hex",
            vec![("ONION_GEN_SEED", &"zz".repeat(32)[..])],
            vec![],
        ),
        ("flag", vec![], vec!["--seed", "abcd"]),
    ] {
        let mut command = Command::new(binary());
        command.args([
            "--compute",
            "cpu",
            "-q",
            "--batch-size",
            "8192",
            "-n",
            "1",
            "-F",
            "aa",
        ]);
        command.args(&args);
        for (key, value) in &env {
            command.env(key, value);
        }
        let run = command.output().expect("the binary runs");
        assert!(!run.status.success(), "{name}: a bad seed was accepted");
        let said = String::from_utf8_lossy(&run.stderr);
        assert!(said.contains("seed"), "{name}: it said {said:?}");
    }
}
