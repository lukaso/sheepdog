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
// (SIGCHLD excepted: the root always gets it at default; see PLAN.md §3.1)

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

// ---- phase-0 fix review, round 2 --------------------------------------------------------

fn fixture() -> &'static str {
    env!("CARGO_BIN_EXE_sd-fixture")
}

/// P1-B: a caller that ignores SIGCHLD makes the kernel reap children automatically. The
/// supervisor must still wait for the root, kill the tree and return the root's code promptly.
/// Linux only: on macOS the no-reset mutant stays green (the defect never existed there).
#[cfg(target_os = "linux")]
#[test]
fn a_caller_that_ignores_sigchld_does_not_break_the_supervisor() {
    let marker = format!("29.{}999001", std::process::id());
    let rec = std::env::temp_dir().join(format!("sd-chld-{}", std::process::id()));
    let _ = std::fs::remove_file(&rec);
    let mut child = Command::new(fixture())
        .args(["exec-chld-ignored", sheepdog(), "run", "--", fixture(), "escape", &marker])
        .arg(&rec)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let start = Instant::now();
    let code = loop {
        if let Some(st) = child.try_wait().unwrap() {
            break st.code();
        }
        if start.elapsed() > Duration::from_secs(5) {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let survivors = Command::new("ps").args(["-Ao", "pid=,args="]).output().map(|o| {
        String::from_utf8_lossy(&o.stdout).lines().filter(|l| l.split_whitespace().any(|w| w == marker)).count()
    });
    // cleanup by marker before asserting
    if let Ok(o) = Command::new("ps").args(["-Ao", "pid=,args="]).output() {
        for l in String::from_utf8_lossy(&o.stdout).lines().filter(|l| l.split_whitespace().any(|w| w == marker)) {
            if let Some(p) = l.split_whitespace().next().and_then(|p| p.parse::<i32>().ok()) {
                unsafe { libc::kill(p, libc::SIGKILL) };
            }
        }
    }
    let _ = std::fs::remove_file(&rec);
    assert_eq!(code, Some(0), "sheepdog did not return the root's code within 5 s");
    assert_eq!(survivors.unwrap_or(1), 0, "the escapee survived");
}

/// P2: argv bytes that are not UTF-8 reach the root unchanged.
#[test]
fn non_utf8_arguments_reach_the_root_unchanged() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    let arg = OsString::from_vec(vec![0xff, 0xfe]);
    let out = Command::new(sheepdog())
        .args(["run", "--", "sh", "-c", "printf %s \"$1\" | od -An -tx1", "_"])
        .arg(&arg)
        .output()
        .unwrap();
    let hex: String = String::from_utf8_lossy(&out.stdout).split_whitespace().collect::<Vec<_>>().join(" ");
    assert_eq!(hex, "ff fe", "the root received different bytes");
}

/// P2: the deadline bounds the runtime even when the tree never looks empty (debug seam
/// SHEEPDOG_TEST_NEVER_EMPTY=1 makes every emptiness check answer "not empty").
#[test]
fn the_kill_deadline_bounds_the_runtime() {
    let mut child = Command::new(sheepdog())
        .args(["run", "--", "true"])
        .env("SHEEPDOG_TEST_NEVER_EMPTY", "1")
        .env("SHEEPDOG_TEST_DEADLINE_MS", "300")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let start = Instant::now();
    loop {
        if let Some(st) = child.try_wait().unwrap() {
            assert_eq!(st.code(), Some(125), "a missed deadline must exit 125");
            return;
        }
        if start.elapsed() > Duration::from_secs(5) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("sheepdog ran past its kill deadline");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// P3: writing to a closed stderr must not panic across the C entry point (UB before Rust
/// 1.81; measured exit 134). A usage error must still exit 125.
#[test]
fn a_closed_stderr_does_not_turn_an_error_into_a_crash() {
    use std::os::fd::FromRawFd;
    let mut fds = [0 as libc::c_int; 2];
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    unsafe { libc::close(fds[0]) }; // nobody will ever read: writes fail with EPIPE
    let stderr = unsafe { Stdio::from_raw_fd(fds[1]) };
    // Command resets SIGPIPE to default in the child (then the write kills sheepdog with
    // SIGPIPE, which is correct); ignore it explicitly so the write returns EPIPE instead
    use std::os::unix::process::CommandExt;
    let st = unsafe {
        Command::new(sheepdog())
            .arg("bogus")
            .stderr(stderr)
            .pre_exec(|| {
                libc::signal(libc::SIGPIPE, libc::SIG_IGN);
                Ok(())
            })
            .status()
            .unwrap()
    };
    assert_eq!(st.code(), Some(125));
}

/// Review round 3: a shell that starts a background job and then execs sheepdog leaves that
/// job as sheepdog's child. It is not part of the command's tree and must survive (it did not
/// on Linux: the subreaper supervisor killed it as a member).
#[test]
fn a_background_job_started_before_sheepdog_is_not_killed() {
    let marker = format!("28.{}777001", std::process::id());
    let script = format!("/bin/sleep {marker} & exec \"$0\" run -- true");
    let st = Command::new("sh").args(["-c", &script, sheepdog()]).status().unwrap();
    std::thread::sleep(Duration::from_millis(200));
    let out = Command::new("ps").args(["-Ao", "pid=,args="]).output().expect("ps");
    assert!(out.status.success());
    let alive: Vec<i32> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| l.split_whitespace().any(|w| w == marker))
        .filter_map(|l| l.split_whitespace().next()?.parse().ok())
        .collect();
    for &p in &alive {
        unsafe { libc::kill(p, libc::SIGKILL) };
    }
    assert_eq!(st.code(), Some(0));
    assert_eq!(alive.len(), 1, "the caller's background job was killed");
}

/// Review round 3, F7: a panic after the freeze must not leave members stopped. Debug seam
/// SHEEPDOG_TEST_PANIC_AFTER_STOP=1 panics right after the first SIGSTOP pass.
#[test]
fn a_panic_after_the_freeze_does_not_leave_members_stopped() {
    let marker = format!("29.{}777002", std::process::id());
    let rec = std::env::temp_dir().join(format!("sd-panic-{}", std::process::id()));
    let _ = std::fs::remove_file(&rec);
    let st = Command::new(sheepdog())
        .args(["run", "--", fixture(), "escape", &marker])
        .arg(&rec)
        .env("SHEEPDOG_TEST_PANIC_AFTER_STOP", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    let out = Command::new("ps").args(["-Ao", "pid=,stat=,args="]).output().expect("ps");
    assert!(out.status.success());
    let left: Vec<(i32, String)> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| l.split_whitespace().any(|w| w == marker))
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            Some((w.next()?.parse().ok()?, w.next()?.to_string()))
        })
        .collect();
    for (p, _) in &left {
        unsafe { libc::kill(*p, libc::SIGKILL) };
    }
    let _ = std::fs::remove_file(&rec);
    assert_eq!(st.code(), Some(125), "a panic must exit 125");
    assert!(left.is_empty(), "members were left behind after a panic: {left:?}");
}
