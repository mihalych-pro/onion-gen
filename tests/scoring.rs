//! What the score changes about a run.
//!
//! The score itself is tested where it is computed. What is tested here is the
//! part that can lose a key: a threshold decides what reaches the disk, and a
//! key not written is gone — the same address will not turn up a second time.

use onion_gen::filter::FilterSet;
use onion_gen::run::{self, RunOptions};
use onion_gen::score;
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::process::{Command, Stdio};

fn temp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "onion-gen-scoring-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ))
}

fn options(out: &Path, limit: u64, seed: u8) -> RunOptions {
    let mut opts = RunOptions::new(out.to_path_buf());
    opts.threads = 2;
    opts.limit = Some(limit);
    opts.print_addresses = false;
    opts.explain_devices = false;
    opts.root_seed = [seed; 32];
    opts
}

fn written(out: &Path) -> Vec<String> {
    fs::read_dir(out)
        .expect("an output directory")
        .map(|e| {
            e.expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter_map(|n| n.strip_suffix(".onion").map(str::to_string))
        .collect()
}

fn score_of(address: &str) -> f64 {
    // Without a placement: the test compares what the threshold let through
    // against the text features alone, which is the part it can check without
    // knowing where the filter landed.
    score::score(address.as_bytes(), None).total
}

/// Off unless asked for. This is the default and the one that cannot lose a
/// key, so it is the one worth pinning down.
#[test]
fn without_a_threshold_every_find_is_written() {
    let out = temp("no-threshold");
    let _ = fs::remove_dir_all(&out);
    let filters = FilterSet::parse_all(["contains:abc"]).expect("a valid filter");
    let summary = run::run(&options(&out, 6, 0x31), &filters).expect("the run");

    assert_eq!(
        summary.below_score, 0,
        "nothing should have been turned away"
    );
    assert_eq!(written(&out).len(), 6, "every find should be on disk");
    let _ = fs::remove_dir_all(&out);
}

/// With a threshold, what reaches the disk is what cleared it — and the limit
/// still counts keys kept, not addresses looked at.
#[test]
fn a_threshold_keeps_only_what_clears_it() {
    let out = temp("threshold");
    let _ = fs::remove_dir_all(&out);
    let filters = FilterSet::parse_all(["contains:abc"]).expect("a valid filter");
    let mut opts = options(&out, 3, 0x47);
    // Above what a plain address reaches, so most finds are turned away and
    // the check is not vacuous.
    opts.min_score = Some(6.0);
    let summary = run::run(&opts, &filters).expect("the run");

    assert_eq!(summary.hits, 3, "the limit counts keys kept");
    let names = written(&out);
    assert_eq!(names.len(), 3, "wrote {names:?}");
    assert!(
        summary.below_score > 0,
        "a threshold that turns nothing away has not been tested"
    );
    let _ = fs::remove_dir_all(&out);
}

/// The best find is named, and it is the best of the ones actually written.
#[test]
fn the_run_names_its_best_find() {
    let out = temp("best");
    let _ = fs::remove_dir_all(&out);
    let filters = FilterSet::parse_all(["contains:abc"]).expect("a valid filter");
    let summary = run::run(&options(&out, 8, 0x53), &filters).expect("the run");

    let (best_score, best_address) = summary.best.clone().expect("a run with finds has a best");
    let names = written(&out);
    assert!(
        names.iter().any(|n| best_address.starts_with(n)),
        "the best find {best_address} is not among {names:?}"
    );
    // Every other find scores no higher on its text features alone. The
    // placement part is left out here because it depends on where the filter
    // landed, which this test does not track; the inequality has to hold on
    // the part it can see.
    let best_text = score_of(best_address.trim_end_matches(".onion"));
    for name in &names {
        assert!(
            score_of(name) <= best_score + 1e-9,
            "{name} scores {} on text alone, above the run's best total {best_score}",
            score_of(name)
        );
    }
    assert!(best_text <= best_score + 1e-9);
    let _ = fs::remove_dir_all(&out);
}

/// A run that found nothing says nothing about a best find.
///
/// Driven through the binary rather than `run::run`, because the library has
/// no way to bound a run that never reaches its limit: an unreachable filter
/// searches forever, and being interrupted is the only way such a run ends.
/// That is also the case worth checking — the summary of an interrupted run is
/// the one somebody reads.
///
/// Unix only: Windows has no signal one process can send another to ask for an
/// orderly stop, so there is no way to produce the summary this checks.
#[test]
#[cfg(unix)]
fn a_run_without_finds_has_no_best() {
    let out = temp("empty");
    let _ = fs::remove_dir_all(&out);
    let child = Command::new(binary())
        .args([
            "--json",
            "--compute",
            "cpu",
            "-t",
            "1",
            "-d",
            out.to_str().expect("a path"),
            "-F",
            // Thirteen symbols: not going to be found in the second this run
            // is given.
            "abcdefghijklm",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the binary runs");

    std::thread::sleep(std::time::Duration::from_millis(800));
    // The same signal a person sends with ctrl-c, which is what the summary
    // after an interrupt is for.
    unsafe { libc_kill(child.id() as i32) };
    let output = child.wait_with_output().expect("it stops");
    let text = String::from_utf8_lossy(&output.stderr);

    let summary = text
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .find(|v| v["event"] == "summary")
        .unwrap_or_else(|| panic!("no summary in {text}"));

    assert_eq!(summary["hits"], 0, "this filter should not have been found");
    assert!(
        summary["best_address"].is_null(),
        "a run with no finds named a best: {summary}"
    );
    assert!(summary["best_score"].is_null(), "{summary}");
    let _ = fs::remove_dir_all(&out);
}

/// `SIGTERM`, without pulling in a crate for one call.
#[cfg(unix)]
unsafe fn libc_kill(pid: i32) {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    unsafe { kill(pid, 15) };
}

#[cfg(unix)]
fn binary() -> PathBuf {
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
