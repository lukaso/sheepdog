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

/// Cell 23 (mask): the root is born with exactly the caller's blocked signals, never with
/// sheepdog's own (it blocks TERM and SIGCHLD for itself; on macOS the self re-exec must
/// carry the caller's mask, or the re-exec'd image records sheepdog's mask as the caller's).
#[test]
fn cell23_the_root_gets_exactly_the_callers_mask() {
    use std::os::unix::process::CommandExt;
    let out = unsafe {
        Command::new(sheepdog())
            .args(["run", "--", fixture(), "print-mask"])
            .pre_exec(|| {
                let mut set: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut set);
                libc::sigaddset(&mut set, libc::SIGUSR1);
                libc::sigaddset(&mut set, libc::SIGWINCH);
                libc::sigprocmask(libc::SIG_SETMASK, &set, std::ptr::null_mut());
                Ok(())
            })
            .output()
            .unwrap()
    };
    let mut got: Vec<i32> = String::from_utf8_lossy(&out.stdout).lines().filter_map(|l| l.trim().parse().ok()).collect();
    got.sort();
    let mut want = vec![libc::SIGUSR1, libc::SIGWINCH];
    want.sort();
    assert_eq!(got, want, "the root's blocked signals differ from the caller's");
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

// ---- TERM ends the job (review round 5, P2-A) -----------------------------------------

fn marked_pids(marker: &str) -> Vec<i32> {
    let out = Command::new("ps").args(["-Ao", "pid=,args="]).output().expect("ps");
    assert!(out.status.success(), "ps failed");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| l.split_whitespace().any(|w| w == marker))
        .filter_map(|l| l.split_whitespace().next()?.parse().ok())
        .collect()
}
/// Live `/bin/sleep <marker>` processes only. Counting every process whose argv mentions the
/// marker also counts sheepdog itself when the marker is inside its `sh -c` argument.
fn sleeps(marker: &str) -> usize {
    let out = Command::new("ps").args(["-Ao", "pid=,args="]).output().expect("ps");
    assert!(out.status.success(), "ps failed");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| {
            let w: Vec<&str> = l.split_whitespace().collect();
            w.len() >= 3 && w[1].ends_with("sleep") && w[2] == marker
        })
        .count()
}
fn kill_marked(markers: &[&str]) {
    for m in markers {
        for p in marked_pids(m) {
            unsafe { libc::kill(p, libc::SIGKILL) };
        }
    }
}

/// Poll `cond` until it holds (up to 10 s). Tests wait for the job to exist instead of
/// sleeping a fixed time: on macOS the first launch of a freshly built binary is delayed by the
/// security scan, and a fixed sleep then races the scan (round 6).
fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
    let start = Instant::now();
    while !cond() {
        assert!(start.elapsed() < Duration::from_secs(10), "timed out waiting for: {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// PLAN.md §3.1: TERM to sheepdog ends the job. The root and an escapee are gone, and
/// sheepdog itself dies of SIGTERM (so a caller sees death by signal).
#[test]
fn term_to_sheepdog_ends_the_job() {
    use std::os::unix::process::ExitStatusExt;
    let (root, esc) = (format!("26.{}444001", std::process::id()), format!("25.{}444002", std::process::id()));
    let inner = format!("/bin/sleep {esc} & exec /bin/sleep {root}");
    let mut c = Command::new(sheepdog()).args(["run", "--", "sh", "-c", &inner]).spawn().unwrap();
    wait_until("the root and the escapee", || sleeps(&root) == 1 && sleeps(&esc) == 1);
    let before = (sleeps(&root), sleeps(&esc));
    unsafe { libc::kill(c.id() as i32, libc::SIGTERM) };
    let st = c.wait().unwrap();
    std::thread::sleep(Duration::from_millis(200));
    let after = (sleeps(&root), sleeps(&esc));
    kill_marked(&[&root, &esc]);
    assert_eq!(before, (1, 1), "the job did not start");
    assert_eq!(after, (0, 0), "TERM left the job running (root, escapee)");
    assert_eq!(st.signal(), Some(libc::SIGTERM), "sheepdog must die of SIGTERM, got {st:?}");
}

// ---- the Linux relay (review round 4) ---------------------------------------------------
// The relay exists only on Linux, when sheepdog starts with children it did not create.

#[cfg(target_os = "linux")]
mod relay {
    use super::*;
    use std::os::unix::process::{CommandExt, ExitStatusExt};

    /// `sd-fixture bg-then-exec <job> sheepdog run -- <root...>` (with a job: the relay path),
    /// or sheepdog directly (no job: the direct path). No shell in between.
    fn start(job: Option<&str>, root: &[&str]) -> Command {
        let mut c = match job {
            Some(j) => {
                let mut c = Command::new(fixture());
                c.args(["bg-then-exec", j, sheepdog(), "run", "--"]);
                c
            }
            None => {
                let mut c = Command::new(sheepdog());
                c.args(["run", "--"]);
                c
            }
        };
        c.args(root).stdout(Stdio::null()).stderr(Stdio::null());
        c
    }
    /// The sheepdog children of `pid` (the relay's supervisor).
    fn supervisors_of(pid: i32) -> Vec<i32> {
        std::fs::read_dir("/proc").unwrap().flatten()
            .filter_map(|e| e.file_name().to_str()?.parse::<i32>().ok())
            .filter(|&p| std::fs::read_to_string(format!("/proc/{p}/stat")).ok()
                .and_then(|s| s.get(s.rfind(')')? + 2..).map(|r| r.split_whitespace().nth(1) == Some(&pid.to_string())))
                .unwrap_or(false)
                && std::fs::read_to_string(format!("/proc/{p}/cmdline")).map_or(false, |c| c.contains("sheepdog")))
            .collect()
    }
    fn m(tag: u32) -> String {
        format!("27.{}55{tag:04}", std::process::id())
    }

    /// P2-1: death by a signal stays death by that signal through the relay.
    #[test]
    fn the_relay_preserves_death_by_signal() {
        let (job, r1, r2) = (m(1), m(2), m(3));
        let mut direct = start(None, &["/bin/sleep", &r1]).spawn().unwrap();
        let mut relayed = start(Some(&job), &["/bin/sleep", &r2]).spawn().unwrap();
        wait_until("both roots", || sleeps(&r1) == 1 && sleeps(&r2) == 1);
        let sups = supervisors_of(relayed.id() as i32);
        unsafe {
            libc::kill(direct.id() as i32, libc::SIGTERM);
            libc::kill(relayed.id() as i32, libc::SIGTERM);
        }
        let (d, r) = (direct.wait().unwrap(), relayed.wait().unwrap());
        kill_marked(&[&job, &r1, &r2]);
        assert_eq!(sups.len(), 1, "the relay path did not start a supervisor: {sups:?}");
        assert_eq!(d.signal(), Some(libc::SIGTERM), "control: without the relay, sheepdog dies of SIGTERM");
        assert_eq!(r.signal(), Some(libc::SIGTERM), "the relay turned death-by-signal into {r:?}");
    }

    /// P3-1: the relay adds no polling delay to the exit (median of 9, within 10 ms).
    #[test]
    fn the_relay_adds_no_exit_latency() {
        let job = m(4);
        let time = |j: Option<&str>| {
            let mut v: Vec<u128> = (0..9)
                .map(|_| {
                    let t = Instant::now();
                    start(j, &["true"]).status().unwrap();
                    t.elapsed().as_millis()
                })
                .collect();
            v.sort();
            v[4]
        };
        let direct = time(None);
        let relayed = time(Some(&job));
        kill_marked(&[&job]);
        assert!(relayed <= direct + 10, "relay median {relayed} ms vs direct {direct} ms");
    }

    /// P3-2 / review round 5 P2-A: if the relay is killed, the job still ends: the root and an
    /// escapee are gone within the kill deadline (not just the supervisor).
    #[test]
    fn a_dead_relay_still_ends_the_job() {
        let (job, root, esc) = (m(5), m(6), m(7));
        let inner = format!("/bin/sleep {esc} & exec /bin/sleep {root}");
        let mut relayed = start(Some(&job), &["sh", "-c", &inner]).spawn().unwrap();
        wait_until("the root and the escapee", || sleeps(&root) == 1 && sleeps(&esc) == 1);
        let sups = supervisors_of(relayed.id() as i32);
        let before = (sleeps(&root), sleeps(&esc));
        unsafe { libc::kill(relayed.id() as i32, libc::SIGKILL) };
        let _ = relayed.wait();
        let deadline = Instant::now() + Duration::from_secs(11);
        let mut after = (1, 1);
        while Instant::now() < deadline {
            after = (sleeps(&root), sleeps(&esc));
            if after == (0, 0) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        kill_marked(&[&job, &root, &esc]);
        for s in &sups {
            unsafe { libc::kill(*s, libc::SIGKILL) };
        }
        assert_eq!(sups.len(), 1, "expected one supervisor, found {sups:?}");
        assert_eq!(before, (1, 1), "the job did not start");
        assert_eq!(after, (0, 0), "killing the relay leaked the job (root, escapee)");
    }

    /// P3-4 / review round 5 P2-B: on the relay path the root gets the caller's signal mask.
    /// The root reads its own mask (no shell: dash resets the mask at startup), and the control
    /// must show the injected bits, or the cell measures nothing.
    #[test]
    fn the_relay_path_keeps_the_callers_mask() {
        let job = m(8);
        let read = |j: Option<&str>| -> String {
            let mut c = start(j, &["grep", "SigBlk", "/proc/self/status"]);
            c.stdout(Stdio::piped());
            let out = unsafe {
                c.pre_exec(|| {
                    let mut set: libc::sigset_t = std::mem::zeroed();
                    libc::sigemptyset(&mut set);
                    libc::sigaddset(&mut set, libc::SIGUSR1);
                    libc::sigaddset(&mut set, libc::SIGWINCH);
                    libc::sigprocmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
                    Ok(())
                })
                .output()
                .unwrap()
            };
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        let direct = read(None);
        let relayed = read(Some(&job));
        kill_marked(&[&job]);
        let bits = |s: &str| u64::from_str_radix(s.trim_start_matches("SigBlk:").trim(), 16).unwrap_or(0);
        let injected = (1u64 << (libc::SIGUSR1 - 1)) | (1u64 << (libc::SIGWINCH - 1));
        assert_eq!(bits(&direct) & injected, injected, "control: the injected mask did not reach the root: {direct:?}");
        assert_eq!(relayed, direct, "the relay path changed the root's signal mask");
    }

    /// Review round 5, P3-5: the relay forwards HUP only when it is the session leader.
    /// This cell asserts forwarding only. That the forwarded HUP must not leak the tree is
    /// the separate (ignored, phase-1) cell `a_forwarded_hup_does_not_leak_the_tree`.
    #[test]
    fn the_relay_forwards_hup_only_as_session_leader() {
        let (j1, j2, r1, r2) = (m(9), m(10), m(11), m(12));
        // not a leader: HUP to the relay is dropped; the relay keeps running
        let mut plain = start(Some(&j1), &["/bin/sleep", &r1]).spawn().unwrap();
        // session leader: HUP is forwarded; the supervisor dies of it, and so does the relay
        let mut leader = unsafe {
            start(Some(&j2), &["/bin/sleep", &r2]).pre_exec(|| {
                libc::setsid();
                Ok(())
            }).spawn().unwrap()
        };
        wait_until("both roots", || sleeps(&r1) == 1 && sleeps(&r2) == 1);
        unsafe {
            libc::kill(plain.id() as i32, libc::SIGHUP);
            libc::kill(leader.id() as i32, libc::SIGHUP);
        }
        std::thread::sleep(Duration::from_millis(300));
        let plain_alive = plain.try_wait().unwrap().is_none();
        let leader_st = leader.wait().unwrap();
        unsafe { libc::kill(plain.id() as i32, libc::SIGKILL) };
        let _ = plain.wait();
        kill_marked(&[&j1, &j2, &r1, &r2]);
        assert!(plain_alive, "a relay that is not the session leader must drop HUP");
        assert_eq!(leader_st.signal(), Some(libc::SIGHUP), "the session leader must forward HUP: {leader_st:?}");
    }
}

// ---- TERM windows (review round 6) -------------------------------------------------------

/// Start sheepdog with a seam window of `ms`, wait until it signals that it is inside the
/// window (a ready file; never a guess with a sleep), and return the child.
fn start_in_window(seam: &str, ms: &str, tag: &str, root: &[&str]) -> std::process::Child {
    let ready = std::env::temp_dir().join(format!("sd-ready-{}-{}", std::process::id(), tag));
    let _ = std::fs::remove_file(&ready);
    let mut c = Command::new(sheepdog())
        .args(["run", "--"])
        .args(root)
        .env(seam, ms)
        .env("SHEEPDOG_TEST_READY_FILE", &ready)
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let start = Instant::now();
    while !ready.exists() {
        if start.elapsed() > Duration::from_secs(10) {
            let _ = c.kill();
            let _ = c.wait();
            panic!("sheepdog never reached the {seam} window");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let _ = std::fs::remove_file(&ready);
    c
}
// Debug seams widen each window so the tests are deterministic:
// SHEEPDOG_TEST_SLEEP_BEFORE_REEXEC_MS (macOS: before the self re-exec) and
// SHEEPDOG_TEST_SLEEP_BEFORE_WAIT_MS (both: after the root is spawned, before the wait).

/// P2-1: a TERM before the root exists ends sheepdog, and the root never runs.
#[cfg(target_os = "macos")]
#[test]
fn a_term_during_the_self_reexec_is_not_lost() {
    use std::os::unix::process::ExitStatusExt;
    let root = format!("24.{}333001", std::process::id());
    // the root records that it started; a root that ran even briefly leaves this file
    let started = std::env::temp_dir().join(format!("sd-started-{}", std::process::id()));
    let _ = std::fs::remove_file(&started);
    let script = format!("touch '{}'; exec /bin/sleep {root}", started.display());
    let mut c = start_in_window("SHEEPDOG_TEST_SLEEP_BEFORE_REEXEC_MS", "300", &root, &["sh", "-c", &script]);
    unsafe { libc::kill(c.id() as i32, libc::SIGTERM) };
    let start = Instant::now();
    let st = loop {
        if let Some(st) = c.try_wait().unwrap() {
            break Some(st);
        }
        if start.elapsed() > Duration::from_secs(5) {
            let _ = c.kill();
            let _ = c.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let ran = started.exists();
    let _ = std::fs::remove_file(&started);
    kill_marked(&[&root]);
    assert_eq!(st.and_then(|s| s.signal()), Some(libc::SIGTERM), "the TERM was lost across the re-exec");
    assert!(!ran, "the root started although TERM arrived before it existed");
}

/// P2-2: a TERM between spawning the root and blocking in the wait ends the job.
#[test]
fn a_term_just_before_the_wait_is_not_lost() {
    use std::os::unix::process::ExitStatusExt;
    let root = format!("24.{}333002", std::process::id());
    let mut c = start_in_window("SHEEPDOG_TEST_SLEEP_BEFORE_WAIT_MS", "400", &root, &["/bin/sleep", &root]);
    unsafe { libc::kill(c.id() as i32, libc::SIGTERM) };
    let start = Instant::now();
    let st = loop {
        if let Some(st) = c.try_wait().unwrap() {
            break Some(st);
        }
        if start.elapsed() > Duration::from_secs(5) {
            let _ = c.kill();
            let _ = c.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let took = start.elapsed();
    std::thread::sleep(Duration::from_millis(100));
    let left = sleeps(&root);
    kill_marked(&[&root]);
    assert_eq!(st.and_then(|s| s.signal()), Some(libc::SIGTERM), "the TERM was lost before the wait");
    // the window is 400 ms; a wait that notices TERM only by a timeout takes over 1 s
    assert!(took < Duration::from_millis(1000), "TERM was noticed only after {took:?}");
    assert_eq!(left, 0, "the root survived the TERM");
}

/// Review round 6, P2-3 (a phase-1 requirement, PLAN.md §7.1): a HUP that the relay
/// forwards as session leader must not leak the tree. Today the supervisor has no HUP
/// handling and dies of it by default action, leaving the root and escapees running.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "phase 1: the supervisor's HUP handling (PLAN.md §3.1, §7.1)"]
fn a_forwarded_hup_does_not_leak_the_tree() {
    use std::os::unix::process::CommandExt;
    let (job, root, esc) = (format!("27.{}556001", std::process::id()), format!("27.{}556002", std::process::id()), format!("27.{}556003", std::process::id()));
    let inner = format!("/bin/sleep {esc} & exec /bin/sleep {root}");
    let mut c = unsafe {
        Command::new(fixture())
            .args(["bg-then-exec", &job, sheepdog(), "run", "--", "sh", "-c", &inner])
            .pre_exec(|| {
                libc::setsid();
                Ok(())
            })
            .spawn()
            .unwrap()
    };
    wait_until("the root and the escapee", || sleeps(&root) == 1 && sleeps(&esc) == 1);
    unsafe { libc::kill(c.id() as i32, libc::SIGHUP) };
    let _ = c.wait();
    std::thread::sleep(Duration::from_millis(500));
    let after = (sleeps(&root), sleeps(&esc));
    kill_marked(&[&job, &root, &esc]);
    assert_eq!(after, (0, 0), "a forwarded HUP leaked the tree (root, escapee)");
}
