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
    // the job's pid comes from `$!`, so the check does not depend on the job having exec'd yet
    let pidf = std::env::temp_dir().join(format!("sd-bg-{}", std::process::id()));
    let _ = std::fs::remove_file(&pidf);
    let script = format!("/bin/sleep 30 & echo $! > '{}'; exec \"$0\" run -- true", pidf.display());
    let st = Command::new("sh").args(["-c", &script, sheepdog()]).status().unwrap();
    let pid: i32 = std::fs::read_to_string(&pidf).unwrap().trim().parse().unwrap();
    let _ = std::fs::remove_file(&pidf);
    // sheepdog has exited, so a job it killed is reaped by init promptly: wait for that, bounded
    let t = Instant::now();
    let mut alive = true;
    while t.elapsed() < Duration::from_millis(500) {
        alive = unsafe { libc::kill(pid, 0) } == 0;
        if !alive {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    unsafe { libc::kill(pid, libc::SIGKILL) };
    assert_eq!(st.code(), Some(0));
    assert!(alive, "the caller's background job was killed");
}

/// Review round 3, F7: a panic after the freeze must not leave members stopped. Debug seam
/// SHEEPDOG_TEST_PANIC_AFTER_STOP=1 panics right after the first SIGSTOP pass.
#[test]
fn a_panic_after_the_freeze_does_not_leave_members_stopped() {
    let marker = format!("29.{}777002", std::process::id());
    let rec = std::env::temp_dir().join(format!("sd-panic-{}", std::process::id()));
    let _ = std::fs::remove_file(&rec);
    let st = Command::new(sheepdog())
        // --grace 0: the TERM grace would end the escapee before the freeze is reached
        .args(["run", "--grace", "0", "--", fixture(), "escape", &marker])
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

/// The largest grace, one day, is accepted in both spellings.
#[test]
fn a_one_day_grace_is_accepted() {
    for v in ["86400", "86400000ms"] {
        let st = Command::new(sheepdog()).args(["run", "--grace", v, "--", "true"]).stderr(Stdio::null()).status().unwrap();
        assert_eq!(st.code(), Some(0), "--grace {v} was refused");
    }
}

/// Review S2 A-P3: a duration that is not finite or too large is a usage error, not a panic.
#[test]
fn an_unrepresentable_grace_is_a_usage_error() {
    for v in ["inf", "NaN", "1e30", "-1", "86401", "86400001ms"] {
        let out = Command::new(sheepdog()).args(["run", "--grace", v, "--", "true"]).output().unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(125), "--grace {v}: {err}");
        assert!(err.contains("usage"), "--grace {v}: no usage line: {err}");
        assert!(!err.contains("panick"), "--grace {v} panicked: {err}");
    }
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

// ---- the relay (review round 4; macOS since the S2 review, A-P2-1) ------------------------
// When sheepdog starts with children it did not create, it forks a fresh supervisor and the
// first process becomes the relay (Linux: not their subreaper; macOS: a fresh uniqueid).

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
        let out = Command::new("ps").args(["-Ao", "pid=,ppid=,args="]).output().expect("ps");
        assert!(out.status.success(), "ps failed");
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| {
                let w: Vec<&str> = l.split_whitespace().collect();
                (w.len() >= 3 && w[1] == pid.to_string() && w[2].ends_with("sheepdog")).then(|| w[0].parse().ok()).flatten()
            })
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

    /// P3-1: the relay adds no polling delay to the exit (median of 9, within 50 ms).
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
        // the defect this guards: a relay that notices the supervisor's exit only at a
        // 250 ms (Linux: 1 s) tick. 50 ms leaves room for the fixture's own start-up.
        assert!(relayed <= direct + 50, "relay median {relayed} ms vs direct {direct} ms");
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

    /// The spawn-window cells (macOS). The root's first action leaves the `ran` file. Seams: the
    /// window's own (it writes the ready file on entry) and SHEEPDOG_TEST_CONT_FILE (written just
    /// before sheepdog resumes the suspended root), so a cell can prove its action came first.
    #[cfg(target_os = "macos")]
    struct Window {
        job: String,
        root: String,
        ready: std::path::PathBuf,
        ran: std::path::PathBuf,
        cont: std::path::PathBuf,
    }
    #[cfg(target_os = "macos")]
    impl Window {
        fn new(tag: u32) -> Self {
            let f = |k: &str| std::env::temp_dir().join(format!("sd-{k}-{}-w{tag}", std::process::id()));
            let w = Window { job: m(tag), root: m(tag + 1), ready: f("ready"), ran: f("ran"), cont: f("cont") };
            for p in [&w.ready, &w.ran, &w.cont] {
                let _ = std::fs::remove_file(p);
            }
            w
        }
        fn with(&self, root: &[&str], relayed: bool, seam: &str, ms: &str) -> Command {
            let mut c = start(relayed.then_some(self.job.as_str()), root);
            c.env(seam, ms).env("SHEEPDOG_TEST_READY_FILE", &self.ready).env("SHEEPDOG_TEST_CONT_FILE", &self.cont);
            c
        }
        /// A root that acts (writes `ran`), then runs as `/bin/sleep <root>`.
        fn command(&self, relayed: bool, seam: &str, ms: &str) -> Command {
            let script = format!("echo RAN > '{}'; exec /bin/sleep {}", self.ran.display(), self.root);
            self.with(&["/bin/sh", "-c", &script], relayed, seam, ms)
        }
        /// A root whose first instruction is its action (`touch <ran>`): no shell start-up for a
        /// kill right after a resume to hide behind.
        fn quick_command(&self, relayed: bool, seam: &str, ms: &str) -> Command {
            let ran = self.ran.display().to_string();
            self.with(&["/usr/bin/touch", &ran], relayed, seam, ms)
        }
        fn wait_ready(&self) {
            wait_until("sheepdog inside the window", || self.ready.exists());
        }
        /// The root, found while it is suspended (state T): its pid.
        fn suspended_root(&self) -> i32 {
            let mut pid = None;
            wait_until("the suspended root", || {
                let out = Command::new("ps").args(["-Ao", "pid=,stat=,args="]).output().expect("ps");
                pid = String::from_utf8_lossy(&out.stdout).lines().find_map(|l| {
                    let w: Vec<&str> = l.split_whitespace().collect();
                    let root = w.len() >= 3 && w[2] != sheepdog() && l.contains(&self.ran.display().to_string());
                    (root && w[1].starts_with('T')).then(|| w[0].parse().ok()).flatten()
                });
                pid.is_some()
            });
            pid.unwrap()
        }
        /// sheepdog has resumed the root (the CONT seam file exists).
        fn resumed(&self) -> bool {
            self.cont.exists()
        }
        fn cleanup(&self, pids: &[i32]) {
            for p in pids {
                unsafe { libc::kill(*p, libc::SIGKILL) };
            }
            for p in [&self.ready, &self.ran, &self.cont] {
                let _ = std::fs::remove_file(p);
            }
            kill_marked(&[&self.job, &self.root]);
        }
    }
    #[cfg(target_os = "macos")]
    fn alive(p: i32) -> bool {
        unsafe { libc::kill(p, 0) == 0 }
    }
    #[cfg(target_os = "macos")]
    fn state(p: i32) -> Option<char> {
        let out = Command::new("ps").args(["-o", "stat=", "-p", &p.to_string()]).output().ok()?;
        String::from_utf8_lossy(&out.stdout).trim().chars().next()
    }
    #[cfg(target_os = "macos")]
    fn wait_bounded(c: &mut std::process::Child, d: Duration) -> Option<std::process::ExitStatus> {
        let t = Instant::now();
        while t.elapsed() < d {
            if let Some(st) = c.try_wait().unwrap() {
                return Some(st);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        None
    }
    /// Wait (up to 11 s) until none of `pids` is alive.
    #[cfg(target_os = "macos")]
    fn until_gone(pids: &[i32]) {
        let deadline = Instant::now() + Duration::from_secs(11);
        while Instant::now() < deadline && pids.iter().any(|&p| alive(p)) {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    /// The caller's shape for a spawn-window cell: HUP, INT, QUIT and TERM at their defaults (a
    /// test run as a background job of a script has INT and QUIT ignored); TERM blocked if asked;
    /// a process group of its own only for the stop cells (killpg). The other cells stay in the
    /// test's group: a group of its own is orphaned when sheepdog dies, and the kernel then sends
    /// HUP and CONT to its stopped members, which would end a root left stopped for another reason.
    #[cfg(target_os = "macos")]
    fn caller(c: &mut Command, own_group: bool, block_term: bool) {
        unsafe {
            c.pre_exec(move || {
                for s in [libc::SIGHUP, libc::SIGINT, libc::SIGQUIT, libc::SIGTERM] {
                    libc::signal(s, libc::SIG_DFL);
                }
                if block_term {
                    let mut set: libc::sigset_t = std::mem::zeroed();
                    libc::sigemptyset(&mut set);
                    libc::sigaddset(&mut set, libc::SIGTERM);
                    libc::sigprocmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
                }
                if own_group {
                    for s in [libc::SIGTSTP, libc::SIGTTIN, libc::SIGTTOU] {
                        libc::signal(s, libc::SIG_DFL);
                    }
                    libc::setpgid(0, 0);
                }
                Ok(())
            });
        }
    }

    /// S2 review rounds 3-4 (macOS): a relay that dies before the root is spawned (during the
    /// supervisor's re-exec, debug seam) means the job must not start: the root's first action
    /// never happens and the supervisor exits. The caller's TERM at its default, blocked or
    /// ignored; and a caller that blocks SIGCONT and ignores TERM: then the root is not spawned
    /// suspended and no TERM can end it, so only the check before the spawn stops it (in the
    /// other shapes the check before the root's CONT would also stop it). That shape uses a root
    /// whose first instruction is its action.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_relay_dead_before_the_spawn_starts_no_root() {
        use std::os::unix::process::CommandExt;
        for (i, caller) in ["default", "blocked", "ignored", "cont-blocked, ignored"].into_iter().enumerate() {
            let w = Window::new(30 + 2 * i as u32);
            let mut c = if caller == "cont-blocked, ignored" {
                w.quick_command(true, "SHEEPDOG_TEST_SLEEP_BEFORE_REEXEC_MS", "600")
            } else {
                w.command(true, "SHEEPDOG_TEST_SLEEP_BEFORE_REEXEC_MS", "600")
            };
            let how = caller;
            unsafe {
                c.pre_exec(move || {
                    if how == "blocked" {
                        let mut set: libc::sigset_t = std::mem::zeroed();
                        libc::sigemptyset(&mut set);
                        libc::sigaddset(&mut set, libc::SIGTERM);
                        libc::sigprocmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
                    } else if how == "ignored" {
                        libc::signal(libc::SIGTERM, libc::SIG_IGN);
                    } else if how == "cont-blocked, ignored" {
                        libc::signal(libc::SIGTERM, libc::SIG_IGN);
                        let mut set: libc::sigset_t = std::mem::zeroed();
                        libc::sigemptyset(&mut set);
                        libc::sigaddset(&mut set, libc::SIGCONT);
                        libc::sigprocmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
                    }
                    Ok(())
                });
            }
            let mut relayed = c.spawn().unwrap();
            w.wait_ready();
            let sups = supervisors_of(relayed.id() as i32);
            unsafe { libc::kill(relayed.id() as i32, libc::SIGKILL) };
            let in_window = sleeps(&w.root) == 0 && !w.ran.exists() && !w.resumed();
            let _ = relayed.wait();
            until_gone(&sups);
            std::thread::sleep(Duration::from_millis(200));
            let (did_run, sup_after) = (w.ran.exists(), sups.iter().filter(|&&p| alive(p)).count());
            w.cleanup(&sups);
            assert!(in_window, "caller TERM {caller}: the kill missed the window (load): the root already existed");
            assert_eq!(sups.len(), 1, "caller TERM {caller}: expected one supervisor in the window, found {sups:?}");
            assert!(!did_run, "caller TERM {caller}: the root ran although its relay was already dead");
            assert_eq!(sup_after, 0, "caller TERM {caller}: the supervisor was left running");
        }
    }

    /// S2 review rounds 4-5 (macOS): a relay that dies while the root is still suspended (after
    /// the pre-spawn check, before the CONT; debug seam) stops the job there: the root is never
    /// resumed, so its first action never happens, and it is not left behind (stopped or not).
    #[cfg(target_os = "macos")]
    #[test]
    fn a_relay_dead_in_the_spawn_window_starts_no_root() {
        // with the caller blocking TERM the root inherits it blocked, so only a SIGKILL keeps it
        // from running once anything resumes it
        for (i, block_term) in [false, true].into_iter().enumerate() {
            let w = Window::new(40 + 20 * i as u32);
            let mut c = w.quick_command(true, "SHEEPDOG_TEST_SLEEP_AFTER_SPAWN_MS", "800");
            caller(&mut c, false, block_term);
            let mut relayed = c.spawn().unwrap();
            w.wait_ready();
            let sups = supervisors_of(relayed.id() as i32);
            let root = w.suspended_root();
            unsafe { libc::kill(relayed.id() as i32, libc::SIGKILL) };
            let _ = relayed.wait(); // the relay is gone before the window is judged
            let late = w.resumed();
            until_gone(&sups);
            until_gone(&[root]);
            let (did_run, root_left, sup_after) = (w.ran.exists(), alive(root), sups.iter().filter(|&&p| alive(p)).count());
            w.cleanup(&[&sups[..], &[root]].concat());
            assert!(!late, "caller blocks TERM={block_term}: the kill missed the window (load): sheepdog had already resumed the root");
            assert_eq!(sups.len(), 1, "caller blocks TERM={block_term}: expected one supervisor in the window, found {sups:?}");
            assert!(!did_run, "caller blocks TERM={block_term}: the root acted although its relay died before it was resumed");
            assert!(!root_left, "caller blocks TERM={block_term}: the suspended root was left behind");
            assert_eq!(sup_after, 0, "caller blocks TERM={block_term}: the supervisor was left running");
        }
    }

    /// S2 review rounds 4-5 (macOS): a TERM that arrives while the root is still suspended ends
    /// the job there: the root is never resumed, it is not left behind, and sheepdog dies of
    /// SIGTERM.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_term_in_the_spawn_window_starts_no_root() {
        for (i, block_term) in [false, true].into_iter().enumerate() {
            let w = Window::new(42 + 20 * i as u32);
            let mut c = w.quick_command(false, "SHEEPDOG_TEST_SLEEP_AFTER_SPAWN_MS", "800");
            caller(&mut c, false, block_term);
            let mut c = c.spawn().unwrap();
            w.wait_ready();
            let root = w.suspended_root();
            unsafe { libc::kill(c.id() as i32, libc::SIGTERM) };
            let late = w.resumed();
            let st = c.wait().unwrap();
            until_gone(&[root]);
            let (did_run, root_left) = (w.ran.exists(), alive(root));
            w.cleanup(&[root]);
            assert!(!late, "caller blocks TERM={block_term}: the TERM missed the window (load): sheepdog had already resumed the root");
            assert!(!did_run, "caller blocks TERM={block_term}: the root acted although TERM came before it was resumed");
            assert!(!root_left, "caller blocks TERM={block_term}: the suspended root was left behind");
            assert_eq!(st.signal(), Some(libc::SIGTERM), "caller blocks TERM={block_term}: sheepdog did not die of SIGTERM: {st:?}");
        }
    }

    /// S2 review rounds 3-5 (macOS): a signal that ends the supervisor between the root's
    /// suspended spawn and its CONT must not leave the root stopped (HUP, INT and QUIT all end
    /// sheepdog by default; the caller's dispositions are reset to make sure they do).
    #[cfg(target_os = "macos")]
    #[test]
    fn a_signal_in_the_spawn_window_leaves_no_stopped_root() {
        for (i, sig) in [libc::SIGHUP, libc::SIGINT, libc::SIGQUIT].into_iter().enumerate() {
            let w = Window::new(44 + 2 * i as u32);
            let mut c = w.command(false, "SHEEPDOG_TEST_SLEEP_AFTER_SPAWN_MS", "800");
            caller(&mut c, false, false);
            let mut c = c.spawn().unwrap();
            w.wait_ready();
            let root = w.suspended_root();
            unsafe { libc::kill(c.id() as i32, sig) };
            let late = w.resumed();
            let st = c.wait().unwrap();
            std::thread::sleep(Duration::from_millis(200));
            let stopped = alive(root) && state(root) == Some('T');
            w.cleanup(&[root]);
            assert!(!late, "signal {sig}: the signal missed the window (load): sheepdog had already resumed the root");
            assert_eq!(st.signal(), Some(sig), "signal {sig}: sheepdog did not die of it: {st:?}");
            assert!(!stopped, "signal {sig}: the root was left stopped after the supervisor died in the spawn window");
        }
    }

    /// S2 review rounds 4-5 (macOS): a ctrl-Z (TSTP to the job's group) in the spawn window stops
    /// the job: the root does not act while the job is stopped, and after the group's CONT it
    /// runs and sheepdog exits 0.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_stop_in_the_spawn_window_stops_the_job() {
        let w = Window::new(50);
        let script = format!("echo RAN > '{}'", w.ran.display());
        let mut c = w.with(&["/bin/sh", "-c", &script], false, "SHEEPDOG_TEST_SLEEP_AFTER_SPAWN_MS", "800");
        caller(&mut c, true, false);
        let mut c = c.spawn().unwrap();
        let pg = c.id() as i32;
        w.wait_ready();
        let root = w.suspended_root();
        unsafe { libc::killpg(pg, libc::SIGTSTP) };
        let late = w.resumed();
        wait_until("sheepdog stopped", || state(pg) == Some('T'));
        std::thread::sleep(Duration::from_millis(300)); // the job stays stopped: nothing may run
        let ran_while_stopped = w.ran.exists();
        unsafe { libc::killpg(pg, libc::SIGCONT) };
        let st = wait_bounded(&mut c, Duration::from_secs(10));
        let ran_after = w.ran.exists();
        if st.is_none() {
            unsafe { libc::killpg(pg, libc::SIGKILL) };
        }
        w.cleanup(&[root]);
        assert!(!late, "the stop missed the window (load): sheepdog had already resumed the root");
        assert!(!ran_while_stopped, "the root acted while the job was stopped");
        assert_eq!(st.and_then(|s| s.code()), Some(0), "sheepdog did not finish after the CONT");
        assert!(ran_after, "the root never ran after the CONT");
    }

    /// S2 review round 5 (macOS): a stop and the job's CONT that come before the root's identity
    /// is read (debug seam) must not let the root run unknown: a child it starts with the
    /// disclaim (its one fact is its original parent, the root) is still killed.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_stop_before_the_identity_read_loses_no_member() {
        let w = Window::new(52);
        let rec = std::env::temp_dir().join(format!("sd-rec-{}-w52", std::process::id()));
        let _ = std::fs::remove_file(&rec);
        let recs = rec.display().to_string();
        let mut c = w.with(&[fixture(), "puniq-only", &w.root, &recs], false, "SHEEPDOG_TEST_SLEEP_AFTER_SPAWN_MS", "3000");
        caller(&mut c, true, false);
        let mut c = c.spawn().unwrap();
        let pg = c.id() as i32;
        w.wait_ready();
        unsafe { libc::killpg(pg, libc::SIGTSTP) };
        wait_until("sheepdog stopped", || state(pg) == Some('T'));
        unsafe { libc::killpg(pg, libc::SIGCONT) };
        let st = wait_bounded(&mut c, Duration::from_secs(15));
        std::thread::sleep(Duration::from_millis(200));
        let d: Vec<(i32, u64)> = std::fs::read_to_string(&rec)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| {
                let mut x = l.split_whitespace();
                Some((x.next()?.parse().ok()?, x.next()?.parse().ok()?))
            })
            .collect();
        let left = d.iter().filter(|&&(p, id)| sheepdog::ident::same(p, id)).count();
        if st.is_none() {
            unsafe { libc::killpg(pg, libc::SIGKILL) };
        }
        let _ = std::fs::remove_file(&rec);
        w.cleanup(&d.iter().map(|x| x.0).collect::<Vec<_>>());
        assert_eq!(st.and_then(|s| s.code()), Some(0), "sheepdog did not finish");
        assert_eq!(d.len(), 1, "D was not created");
        assert_eq!(left, 0, "the root ran before its identity was read, and its disclaimed child survived");
    }

    /// S2 review round 5 (macOS): a stop in the window, then TERM and the job's CONT. The job's
    /// CONT resumes the root too, so it may run (a debug seam holds sheepdog after the stop is let
    /// through and before its check, so the root does run and starts a child). The check then
    /// finds TERM, and the whole job ends: the root and its child.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_term_after_a_stop_in_the_window_leaves_nothing() {
        let w = Window::new(54);
        let esc = m(56);
        let script = format!("/bin/sleep {esc} & exec /bin/sleep {}", w.root);
        let mut c = w.with(&["/bin/sh", "-c", &script], false, "SHEEPDOG_TEST_SLEEP_BEFORE_CONT_CHECK_MS", "2000");
        caller(&mut c, true, false);
        let mut c = c.spawn().unwrap();
        let pg = c.id() as i32;
        w.wait_ready();
        unsafe { libc::killpg(pg, libc::SIGTSTP) };
        wait_until("sheepdog stopped", || state(pg) == Some('T'));
        unsafe {
            libc::kill(pg, libc::SIGTERM);
            libc::killpg(pg, libc::SIGCONT);
        }
        wait_until("the root's child (the job's CONT resumed the root)", || sleeps(&esc) == 1);
        let st = wait_bounded(&mut c, Duration::from_secs(15));
        let resumed_by_sheepdog = w.resumed(); // read after sheepdog's check has run
        std::thread::sleep(Duration::from_millis(200));
        let left = (sleeps(&w.root), sleeps(&esc));
        if st.is_none() {
            unsafe { libc::killpg(pg, libc::SIGKILL) };
        }
        kill_marked(&[&esc]);
        w.cleanup(&[]);
        assert!(!resumed_by_sheepdog, "sheepdog resumed the root itself although TERM was pending");
        assert_eq!(st.and_then(|s| s.signal()), Some(libc::SIGTERM), "sheepdog did not die of SIGTERM");
        assert_eq!(left, (0, 0), "the job was left (root, the root's child)");
    }

    /// S2 review round 6 (macOS): the same early end, with another deadly signal (INT, HUP,
    /// QUIT) sent to sheepdog together with the TERM: sheepdog still kills the whole job and dies
    /// of TERM; the other signal must not end it before the kill.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_term_and_another_signal_after_a_stop_leave_nothing() {
        // both caller shapes that can end early on TERM: TERM at its default, and blocked (then
        // the root inherits it blocked and the kill waits out the grace)
        let cases = [false, true].into_iter().flat_map(|b| [libc::SIGINT, libc::SIGHUP, libc::SIGQUIT].map(move |s| (b, s)));
        for (i, (block_term, other)) in cases.enumerate() {
            let w = Window::new(64 + 3 * i as u32);
            let esc = m(66 + 3 * i as u32);
            let script = format!("/bin/sleep {esc} & exec /bin/sleep {}", w.root);
            let mut c = w.with(&["/bin/sh", "-c", &script], false, "SHEEPDOG_TEST_SLEEP_BEFORE_CONT_CHECK_MS", "2000");
            caller(&mut c, true, block_term);
            let mut c = c.spawn().unwrap();
            let pg = c.id() as i32;
            w.wait_ready();
            unsafe { libc::killpg(pg, libc::SIGTSTP) };
            wait_until("sheepdog stopped", || state(pg) == Some('T'));
            unsafe {
                libc::kill(pg, libc::SIGTERM);
                libc::kill(pg, other);
                libc::killpg(pg, libc::SIGCONT);
            }
            wait_until("the root's child (the job's CONT resumed the root)", || sleeps(&esc) == 1);
            let st = wait_bounded(&mut c, Duration::from_secs(15));
            let late = w.resumed(); // written only when sheepdog resumes the root: the window was missed
            std::thread::sleep(Duration::from_millis(200));
            let left = (sleeps(&w.root), sleeps(&esc));
            if st.is_none() {
                unsafe { libc::killpg(pg, libc::SIGKILL) };
            }
            kill_marked(&[&esc]);
            w.cleanup(&[]);
            assert!(!late, "caller blocks TERM={block_term}, signal {other}: the signals missed the window (load): sheepdog resumed the root");
            assert_eq!(left, (0, 0), "caller blocks TERM={block_term}, signal {other}: the job was left (root, the root's child)");
            assert_eq!(st.and_then(|s| s.signal()), Some(libc::SIGTERM), "caller blocks TERM={block_term}, signal {other}: sheepdog did not die of SIGTERM");
        }
    }

    /// S2 review round 7 (macOS): an early end caused by the relay's death, with another deadly
    /// signal (INT) pending at sheepdog: the whole job is still killed. A CONT to the root itself
    /// lets it run and start a child first (a debug seam holds sheepdog before its check).
    #[cfg(target_os = "macos")]
    #[test]
    fn a_relay_death_and_another_signal_leave_nothing() {
        let w = Window::new(84);
        let esc = m(86);
        let script = format!("echo x > '{}'; /bin/sleep {esc} & exec /bin/sleep {}", w.ran.display(), w.root);
        let mut c = w.with(&["/bin/sh", "-c", &script], true, "SHEEPDOG_TEST_SLEEP_BEFORE_CONT_CHECK_MS", "2500");
        caller(&mut c, false, false);
        let mut relayed = c.spawn().unwrap();
        w.wait_ready();
        let sups = supervisors_of(relayed.id() as i32);
        let root = w.suspended_root();
        unsafe { libc::kill(root, libc::SIGCONT) };
        wait_until("the root's child", || sleeps(&esc) == 1);
        unsafe { libc::kill(relayed.id() as i32, libc::SIGKILL) };
        let _ = relayed.wait();
        for &p in &sups {
            unsafe { libc::kill(p, libc::SIGINT) };
        }
        until_gone(&sups);
        let late = w.resumed(); // written only when sheepdog resumes the root: the window was missed
        std::thread::sleep(Duration::from_millis(200));
        let left = (sleeps(&w.root), sleeps(&esc));
        kill_marked(&[&esc]);
        w.cleanup(&[&sups[..], &[root]].concat());
        assert_eq!(sups.len(), 1, "expected one supervisor, found {sups:?}");
        assert!(!late, "the relay kill and the INT missed the window (load): sheepdog resumed the root");
        assert_eq!(left, (0, 0), "the job was left (root, the root's child)");
    }

    /// S2 review round 5 (macOS): a relay that dies after the root is resumed but before the
    /// wait registers for the relay's exit (debug seam) still ends the job: the registration
    /// fails with ESRCH, which counts as the relay's death.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_relay_dead_before_the_wait_still_ends_the_job() {
        let w = Window::new(58);
        let registered = std::env::temp_dir().join(format!("sd-reg-{}-w58", std::process::id()));
        let _ = std::fs::remove_file(&registered);
        let mut relayed = w
            .with(&["/bin/sleep", &w.root], true, "SHEEPDOG_TEST_SLEEP_BEFORE_RELAY_REG_MS", "800")
            .env("SHEEPDOG_TEST_RELAY_REG_FILE", &registered)
            .spawn()
            .unwrap();
        w.wait_ready();
        let sups = supervisors_of(relayed.id() as i32);
        unsafe { libc::kill(relayed.id() as i32, libc::SIGKILL) };
        let _ = relayed.wait();
        let late = registered.exists();
        let _ = std::fs::remove_file(&registered);
        until_gone(&sups);
        let deadline = Instant::now() + Duration::from_secs(11);
        while Instant::now() < deadline && sleeps(&w.root) > 0 {
            std::thread::sleep(Duration::from_millis(50));
        }
        let (left, sup_after) = (sleeps(&w.root), sups.iter().filter(|&&p| alive(p)).count());
        w.cleanup(&sups);
        assert!(!late, "the kill missed the window (load): the wait had already registered the relay's exit");
        assert_eq!(sups.len(), 1, "expected one supervisor, found {sups:?}");
        assert_eq!((left, sup_after), (0, 0), "a relay dead before the wait's registration leaked the job (root, supervisor)");
    }

    /// S2 review round 2: a dead relay still ends the job when the supervisor's wait has fallen
    /// back to polling (debug seam: the root's exit cannot be watched), where kqueue events are
    /// not read.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_dead_relay_still_ends_the_job_while_polling() {
        let (job, root, esc) = (m(13), m(14), m(15));
        let inner = format!("/bin/sleep {esc} & exec /bin/sleep {root}");
        let errf = std::env::temp_dir().join(format!("sd-relay-poll-{}", std::process::id()));
        let mut relayed = start(Some(&job), &["sh", "-c", &inner])
            .env("SHEEPDOG_TEST_KQ_EINVAL", "1")
            .stderr(std::fs::File::create(&errf).unwrap())
            .spawn()
            .unwrap();
        wait_until("the root and the escapee", || sleeps(&root) == 1 && sleeps(&esc) == 1);
        let sups = supervisors_of(relayed.id() as i32);
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
        let err = std::fs::read_to_string(&errf).unwrap_or_default();
        let _ = std::fs::remove_file(&errf);
        assert!(err.contains("polling instead"), "the seam did not make the wait poll: {err:?}");
        assert_eq!(after, (0, 0), "killing the relay leaked the job while polling (root, escapee)");
    }

    /// P3-4 / review round 5 P2-B: on the relay path the root gets the caller's signal mask.
    /// The root reads its own mask (no shell: dash resets the mask at startup), and the control
    /// must show the injected bits, or the cell measures nothing.
    #[test]
    fn the_relay_path_keeps_the_callers_mask() {
        let job = m(8);
        let read = |j: Option<&str>| -> String {
            let mut c = start(j, &[fixture(), "print-mask"]);
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
        let has = |s: &str, sig: i32| s.lines().any(|l| l.trim() == sig.to_string());
        assert!(has(&direct, libc::SIGUSR1) && has(&direct, libc::SIGWINCH), "control: the injected mask did not reach the root: {direct:?}");
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
