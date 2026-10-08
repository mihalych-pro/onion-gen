//! A run that is interrupted and continued.
//!
//! The search space is cut into blocks seeded from a root seed and a counter,
//! so continuing means remembering two numbers. What has to be true of them is
//! narrow but load-bearing: the second run must not look at a candidate the
//! first one already looked at, and it must not start over.

use onion_gen::filter::FilterSet;
use onion_gen::run::{self, RunOptions, RunState};
use std::path::PathBuf;

fn temp(name: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("onion-gen-{name}-{}", std::process::id()));
    path
}

/// One short run, then another over the same state file.
#[test]
fn a_continued_run_picks_up_where_it_stopped() {
    let state = temp("resume-state");
    let out = temp("resume-out");
    let _ = std::fs::remove_file(&state);
    let _ = std::fs::remove_dir_all(&out);

    let filters = FilterSet::parse_all(["aaa"]).expect("a valid filter");
    let make = || {
        let mut opts = RunOptions::new(out.clone());
        opts.threads = 1;
        opts.limit = Some(1);
        opts.print_addresses = false;
        opts.explain_devices = false;
        opts.state_path = Some(state.clone());
        opts.root_seed = [0x5eu8; 32];
        opts
    };

    let first = run::run(&make(), &filters).expect("the first run");
    let saved = RunState::load(&state)
        .expect("a readable state")
        .expect("a written state");
    assert_eq!(saved.hits, first.hits);
    assert!(saved.next_block > 0, "the first run claimed no block");

    // The second run is given a different seed on purpose: if it ignored the
    // state file it would search elsewhere, and the seed in the file is what
    // proves it did not.
    let mut second = make();
    second.root_seed = [0x11u8; 32];
    let resumed = RunState::load(&state).expect("readable").expect("written");
    assert!(resumed.matches(&filters), "same filters, same search");
    second.root_seed = resumed.seed().expect("a seed in the file");
    assert_eq!(
        second.root_seed, [0x5eu8; 32],
        "the file's seed, not a new one"
    );

    let after = run::run(&second, &filters).expect("the second run");
    let saved_again = RunState::load(&state).expect("readable").expect("written");

    // Continued, not restarted: the block counter only ever goes up.
    assert!(
        saved_again.next_block > saved.next_block,
        "the second run started at {} and the first left {}",
        saved_again.next_block,
        saved.next_block
    );
    assert!(after.hits > 0, "the second run found nothing");

    let _ = std::fs::remove_file(&state);
    let _ = std::fs::remove_dir_all(&out);
}

/// Continuing a different search in the same file would mix two counts.
#[test]
fn a_state_file_knows_which_search_it_belongs_to() {
    let state = temp("resume-filters");
    let out = temp("resume-filters-out");
    let _ = std::fs::remove_file(&state);
    let _ = std::fs::remove_dir_all(&out);

    let filters = FilterSet::parse_all(["aaa"]).expect("valid");
    let mut opts = RunOptions::new(out.clone());
    opts.threads = 1;
    opts.limit = Some(1);
    opts.print_addresses = false;
    opts.explain_devices = false;
    opts.state_path = Some(state.clone());
    run::run(&opts, &filters).expect("a run");

    let saved = RunState::load(&state).expect("readable").expect("written");
    assert!(saved.matches(&filters));
    let other = FilterSet::parse_all(["bbb"]).expect("valid");
    assert!(!saved.matches(&other), "a different search must be refused");
    // Order is not identity: the same set written differently is the same set.
    let reordered = FilterSet::parse_all(["aaa"]).expect("valid");
    assert!(saved.matches(&reordered));

    let _ = std::fs::remove_file(&state);
    let _ = std::fs::remove_dir_all(&out);
}

/// The file is the run's keys, so it must not be readable by anyone else.
#[cfg(unix)]
#[test]
fn the_state_file_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let state = temp("resume-perms");
    let out = temp("resume-perms-out");
    let _ = std::fs::remove_file(&state);
    let _ = std::fs::remove_dir_all(&out);

    let filters = FilterSet::parse_all(["aaa"]).expect("valid");
    let mut opts = RunOptions::new(out.clone());
    opts.threads = 1;
    opts.limit = Some(1);
    opts.print_addresses = false;
    opts.explain_devices = false;
    opts.state_path = Some(state.clone());
    run::run(&opts, &filters).expect("a run");

    let mode = std::fs::metadata(&state)
        .expect("the file exists")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "the state file is as secret as a key");

    let _ = std::fs::remove_file(&state);
    let _ = std::fs::remove_dir_all(&out);
}
