//! What the run tells an orchestrator when it ends.
//!
//! An orchestrator does not talk to the process; it looks at how the process
//! ended. That makes the exit code the whole of the control surface, and it
//! used to carry no information: a run stopped by `SIGTERM` having found
//! nothing exited zero, exactly like a run that found everything asked of it.
//!
//! Under a Kubernetes `Job` wanting one completion that is not a nuisance but
//! a fleet-wide failure. A pod evicted during a node drain exits zero, the Job
//! counts a success, the completion is satisfied, and every remaining pod is
//! torn down with no key found anywhere. Nothing reports an error; the job
//! simply "completed", and nobody goes looking.

use std::path::PathBuf;
use std::process::{Command, Stdio};

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

fn out_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "onion-gen-exit-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Runs the binary and returns its exit code, interrupting it after `stop_after`
/// when one is given.
fn code(args: &[&str], stop_after: Option<std::time::Duration>) -> i32 {
    let child = Command::new(binary())
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the binary runs");

    if let Some(wait) = stop_after {
        std::thread::sleep(wait);
        // The signal a node drain sends, and the one ctrl-c sends. Windows has
        // no equivalent that one process can deliver to another, so the tests
        // that need it are unix-only.
        #[cfg(unix)]
        unsafe {
            extern "C" {
                fn kill(pid: i32, sig: i32) -> i32;
            }
            kill(child.id() as i32, 15);
        }
        #[cfg(not(unix))]
        let _ = &child;
    }
    child
        .wait_with_output()
        .expect("it ends")
        .status
        .code()
        .expect("an exit code rather than a signal")
}

#[test]
fn reaching_the_goal_exits_zero() {
    let dir = out_dir("goal");
    let got = code(
        &[
            "-F",
            "abc",
            "-n",
            "1",
            "--compute",
            "cpu",
            "-t",
            "2",
            "-d",
            dir.to_str().expect("a path"),
        ],
        None,
    );
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(got, 0, "a run that found what was asked must exit zero");
}

#[test]
#[cfg(unix)]
fn being_stopped_short_of_the_goal_does_not_exit_zero() {
    let dir = out_dir("short");
    let got = code(
        &[
            // Thirteen symbols: not going to be found in the second it is given.
            "-F",
            "abcdefghijklm",
            "-n",
            "1",
            "--compute",
            "cpu",
            "-t",
            "1",
            "-d",
            dir.to_str().expect("a path"),
        ],
        Some(std::time::Duration::from_millis(900)),
    );
    let _ = std::fs::remove_dir_all(&dir);
    assert_ne!(
        got, 0,
        "an interrupted run that found nothing must not look like a success: \
         under a Job wanting one completion, zero here tears down the fleet"
    );
    assert_eq!(got, 4, "and it must be the code reserved for that");
}

/// A run with no limit has no goal, so being stopped is the only way it can
/// end. Documented rather than special-cased: a `Job` over such a run can
/// never complete, and that has to be visible rather than surprising.
#[test]
#[cfg(unix)]
fn a_run_without_a_goal_cannot_report_success() {
    let dir = out_dir("nogoal");
    let got = code(
        &[
            "-F",
            "abc",
            "-n",
            "0",
            "--compute",
            "cpu",
            "-t",
            "1",
            "-d",
            dir.to_str().expect("a path"),
        ],
        Some(std::time::Duration::from_millis(900)),
    );
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(got, 4, "no limit means no goal to have reached");
}

/// The codes have to stay distinguishable from the ones already in use, or an
/// orchestrator cannot tell "stopped early" from "started wrong".
#[test]
fn a_usage_error_keeps_its_own_code() {
    let got = code(&["--filter"], None);
    assert_eq!(got, 2, "a bad command line is not a stopped run");
}
