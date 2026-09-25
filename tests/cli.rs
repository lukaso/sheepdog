//! Basic `run` behaviour. Every spawn here is bounded, so a hang fails the test instead of
//! hanging the suite (the phase-0 spike once re-exec'd itself forever).

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn sheepdog() -> &'static str {
    env!("CARGO_BIN_EXE_sheepdog")
}

/// Run to completion within `limit`, or kill it and panic.
fn run_bounded(args: &[&str], limit: Duration) -> i32 {
    let mut child = Command::new(sheepdog()).args(args).stdout(Stdio::null()).spawn().unwrap();
    let start = Instant::now();
    loop {
        if let Some(st) = child.try_wait().unwrap() {
            return st.code().expect("sheepdog died from a signal");
        }
        if start.elapsed() > limit {
            let _ = child.kill();
            let _ = child.wait();
            panic!("sheepdog {args:?} did not finish within {limit:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_trivial_command_finishes_promptly() {
    assert_eq!(run_bounded(&["run", "--", "true"], Duration::from_secs(5)), 0);
}

#[test]
fn the_commands_exit_code_passes_through() {
    assert_eq!(run_bounded(&["run", "--", "sh", "-c", "exit 7"], Duration::from_secs(5)), 7);
}

#[test]
fn a_missing_command_exits_127() {
    assert_eq!(run_bounded(&["run", "--", "/nonexistent/x"], Duration::from_secs(5)), 127);
}

/// PLAN.md §3.1 "at most one re-exec attempt": with the responsibility SPI answering "no
/// fact" (debug-only seam), the self re-exec never takes effect. sheepdog must fall back and
/// finish, not re-exec forever.
#[cfg(target_os = "macos")]
#[test]
fn a_failing_self_reexec_falls_back_instead_of_looping() {
    let mut child = Command::new(sheepdog())
        .args(["run", "--", "true"])
        .env("SHEEPDOG_TEST_SPI", "broken")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let start = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if start.elapsed() > Duration::from_secs(5) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("sheepdog re-exec'd forever when the SPI gave no answer");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stderr).contains("tracking is degraded"));
}

// ---- cell 23 (PLAN.md §3.1): the root gets the caller's signal state ------------------

/// Run `script` under `sh -c`, with the given signals ignored by the caller, then
/// `exec sheepdog run -- sh -c <inner>`. Returns (exit code, stderr).
fn with_caller_ignoring(ignored: &str, inner: &str) -> (i32, String) {
    let script = format!("trap '' {ignored}; exec \"$0\" run -- sh -c \"$1\"");
    let out = Command::new("sh")
        .args(["-c", &script, sheepdog(), inner])
        .stdout(Stdio::null())
        .output()
        .unwrap();
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stderr).into_owned())
}

#[test]
fn cell23_signals_the_caller_ignored_stay_ignored() {
    // a shell cannot un-ignore a signal ignored on entry, so each kill must do nothing
    let (code, err) = with_caller_ignoring("HUP INT TERM", "kill -HUP $$; kill -INT $$; kill -TERM $$; exit 0");
    assert_eq!(code, 0, "the root died from a signal its caller ignored; stderr: {err}");
}

#[test]
fn cell23_a_default_sigpipe_stays_default() {
    // with SIGPIPE at its default, `yes` dies silently when `head` exits
    let (code, err) = with_caller_ignoring("USR2", "yes | head -c1 >/dev/null");
    assert_eq!(code, 0);
    assert!(!err.contains("Broken pipe"), "SIGPIPE was ignored in the job: {err}");
}

#[test]
fn cell23_an_ignored_sigpipe_stays_ignored() {
    // with SIGPIPE ignored by the caller, `yes` gets EPIPE and says so
    let (_, err) = with_caller_ignoring("PIPE", "yes | head -c1 >/dev/null");
    assert!(err.contains("Broken pipe") || err.contains("EPIPE"), "SIGPIPE was reset to default: {err:?}");
}
