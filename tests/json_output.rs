//! Every line the machine-readable mode prints, parsed by a JSON parser.
//!
//! The point of `--json` is that a caller stops reading the program's prose
//! with a regular expression. That only holds if the stream has no prose left
//! in it, and a diagnostic added later is exactly how it would come back.

use std::process::Command;

fn binary() -> std::path::PathBuf {
    // The test binary lives next to the one under test.
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

#[test]
fn every_line_of_a_run_is_json() {
    let out = std::env::temp_dir().join(format!("onion-gen-json-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    let run = Command::new(binary())
        .args([
            "--json",
            "--compute",
            "cpu",
            // Fixed, so the run enumerates in a known order and starts without
            // first measuring the machine. Both are what these tests compare.
            "--batch-size",
            "8192",
            "-t",
            "2",
            "-n",
            "1",
            "-d",
            out.to_str().expect("a printable path"),
            "-F",
            "abc",
        ])
        .output()
        .expect("the binary runs");
    assert!(run.status.success(), "the run failed");

    let mut events = Vec::new();
    for stream in [&run.stdout, &run.stderr] {
        for line in String::from_utf8_lossy(stream).lines() {
            if line.trim().is_empty() {
                continue;
            }
            let value: serde_json::Value =
                serde_json::from_str(line).unwrap_or_else(|e| panic!("not JSON: {line:?} ({e})"));
            events.push(
                value
                    .get("event")
                    .and_then(|e| e.as_str())
                    .unwrap_or("none")
                    .to_string(),
            );
        }
    }
    for wanted in ["start", "hit", "summary"] {
        assert!(
            events.iter().any(|e| e == wanted),
            "no {wanted} event among {events:?}"
        );
    }
    let _ = std::fs::remove_dir_all(&out);
}

/// What the prose said has to still be sayable, or the mode is a downgrade.
#[test]
fn the_start_event_carries_what_the_prose_did() {
    let out = std::env::temp_dir().join(format!("onion-gen-json2-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    let run = Command::new(binary())
        .args([
            "--json",
            "--compute",
            "cpu",
            // Fixed, so the run enumerates in a known order and starts without
            // first measuring the machine. Both are what these tests compare.
            "--batch-size",
            "8192",
            "-t",
            "1",
            "-n",
            "1",
            "-d",
            out.to_str().expect("a printable path"),
            "-F",
            "ab",
        ])
        .output()
        .expect("the binary runs");

    let start = String::from_utf8_lossy(&run.stderr)
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .find(|v| v.get("event").and_then(|e| e.as_str()) == Some("start"))
        .expect("a start event");
    for key in [
        "arithmetic",
        "devices",
        "filters",
        "threads",
        "batch_size",
        "structures",
        "needs_checksum",
        "expected_candidates",
        "out_dir",
    ] {
        assert!(start.get(key).is_some(), "the start event has no {key}");
    }
    assert_eq!(start["threads"], 1);
    // Two symbols of prefix: one hit in 1024 candidates.
    let expected = start["expected_candidates"].as_f64().expect("a number");
    assert!((expected - 1024.0).abs() < 1.0, "{expected}");
    let _ = std::fs::remove_dir_all(&out);
}

/// The first statistics line describes a whole interval.
///
/// The requirement says so and the code does it, but until now the only thing
/// standing behind it was the measurement harness, which drops the first line
/// as warm-up and would not notice if it stopped being droppable. A first line
/// covering a tenth of a second would read as a third of the real speed, and
/// every number in the project comes from this stream.
#[test]
fn the_first_statistics_line_covers_a_full_interval() {
    use std::io::Read;

    let out = std::env::temp_dir().join(format!("onion-gen-stats-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    // Stopped by the clock rather than by a hit: a filter long enough to
    // outlast two intervals is also long enough to outlast the afternoon.
    let mut child = Command::new(binary())
        .args([
            "--json",
            "--compute",
            "cpu",
            // Fixed, so the run enumerates in a known order and starts without
            // first measuring the machine. Both are what these tests compare.
            "--batch-size",
            "8192",
            "-t",
            "2",
            "-S",
            "1",
            "-d",
            out.to_str().expect("a printable path"),
            "-F",
            "zzzzzzzzzz",
        ])
        .stderr(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .expect("the binary runs");
    // Long enough for two intervals even when the other tests in this file
    // are running searches of their own on the same cores.
    std::thread::sleep(std::time::Duration::from_millis(4500));
    let _ = child.kill();
    let mut text = String::new();
    if let Some(mut err) = child.stderr.take() {
        let _ = err.read_to_string(&mut text);
    }
    let _ = child.wait();

    let stats: Vec<serde_json::Value> = text
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| v.get("event").and_then(|e| e.as_str()) == Some("stats"))
        .collect();
    assert!(
        stats.len() >= 2,
        "only {} statistics lines in 4.5s of 1s intervals",
        stats.len()
    );
    let first = stats[0]["elapsed_sec"].as_f64().expect("a number");
    assert!(
        first >= 1.0,
        "the first line came after {first:.3}s of a 1s interval"
    );

    // And the rate it reports is the interval's, not the run's so far. Which
    // one it is can be settled exactly rather than by how the numbers look:
    // the candidates and the elapsed time are both in the event, so the rate
    // an interval implies is a division, and a cumulative average would not
    // match it.
    let at = |i: usize, key: &str| stats[i][key].as_f64().expect("a number");
    let implied =
        (at(1, "candidates") - at(0, "candidates")) / (at(1, "elapsed_sec") - at(0, "elapsed_sec"));
    let reported = at(1, "calc_per_sec");
    assert!(
        (reported - implied).abs() < implied * 0.1,
        "the line says {reported:.0}/s where the interval implies {implied:.0}/s"
    );
    let _ = std::fs::remove_dir_all(&out);
}

/// A seed and a block, instead of a file, are a complete way to keep the
/// run's place: the same two values give the same search back.
#[test]
fn a_named_seed_makes_the_run_repeatable() {
    let seed = "0011223344556677889900aabbccddeeff0011223344556677889900aabbccdd";
    let addresses = |dir: &std::path::Path, from_block: &str| {
        let _ = std::fs::remove_dir_all(dir);
        let run = Command::new(binary())
            .args([
                "--compute",
                "cpu",
                "-q",
                "-t",
                "1",
                "--seed",
                seed,
                "--from-block",
                from_block,
                "-n",
                "2",
                "-d",
                dir.to_str().expect("a printable path"),
                "-F",
                "aa",
            ])
            .output()
            .expect("the binary runs");
        assert!(run.status.success());
        let mut found: Vec<String> = String::from_utf8_lossy(&run.stdout)
            .lines()
            .map(str::to_string)
            .collect();
        found.sort();
        found
    };

    let a = addresses(&std::env::temp_dir().join("onion-gen-seed-a"), "0");
    let b = addresses(&std::env::temp_dir().join("onion-gen-seed-b"), "0");
    assert_eq!(a.len(), 2, "the run found {} addresses", a.len());
    assert_eq!(a, b, "the same seed has to give the same search");

    // A different starting block is a different part of the same space.
    let c = addresses(&std::env::temp_dir().join("onion-gen-seed-c"), "64");
    assert_ne!(a, c, "starting further along must not repeat the start");

    for name in ["onion-gen-seed-a", "onion-gen-seed-b", "onion-gen-seed-c"] {
        let _ = std::fs::remove_dir_all(std::env::temp_dir().join(name));
    }
}

/// Asking to start at a block without saying which search is a mistake worth
/// naming: against a fresh random seed a block number means nothing.
#[test]
fn a_starting_block_without_a_seed_is_refused() {
    let run = Command::new(binary())
        .args([
            "--compute",
            "cpu",
            // Fixed, so the run enumerates in a known order and starts without
            // first measuring the machine. Both are what these tests compare.
            "--batch-size",
            "8192",
            "--from-block",
            "5",
            "-n",
            "1",
            "-F",
            "aa",
        ])
        .output()
        .expect("the binary runs");
    assert!(!run.status.success(), "it should have refused");
    let said = String::from_utf8_lossy(&run.stderr);
    assert!(
        said.contains("--from-block needs a seed"),
        "it said: {said}"
    );
}
