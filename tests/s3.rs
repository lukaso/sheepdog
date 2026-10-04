//! Phase 1, step S3 (PHASE1.md): identity before every signal (Linux: pidfd, with the
//! ENOSYS/EPERM fallback; macOS: the uniqueid re-check) and the wrong-freeze rollback
//! (PLAN.md §3.3 steps 2 and 4). A decoy is a process outside any job that logs every catchable
//! signal it gets; a SIGSTOP shows as state T, a SIGKILL as its death. Every signal a test
//! sends goes through the identity-checked door in `common`.

mod common;

use common::send;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

static SEQ: AtomicUsize = AtomicUsize::new(0);

fn sheepr() -> &'static str {
    common::test_env();
    env!("CARGO_BIN_EXE_sheepr")
}
fn fixture() -> &'static str {
    common::test_env();
    env!("CARGO_BIN_EXE_sr-fixture")
}

fn state(pid: i32) -> Option<char> {
    let out = Command::new("ps").args(["-o", "stat=", "-p", &pid.to_string()]).output().ok()?;
    String::from_utf8_lossy(&out.stdout).trim().chars().next()
}
/// The debug signal log (SHEEPR_TEST_SIGNAL_LOG): one line per signal decision,
/// `pidfd <pid> <sig>`, `kill <pid> <sig>` (the fallback or macOS), `rollback <pid>`.
fn signal_log(path: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(path).map(|s| s.lines().map(String::from).collect()).unwrap_or_default()
}
fn log_path(tag: &str) -> std::path::PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!("sr-s3-siglog-{}-{tag}-{n}", std::process::id()));
    let _ = std::fs::remove_file(&p);
    p
}
/// Does `pidfd_open` work here? Not on macOS, and not in ./test-all's enosys leg (a seccomp
/// profile makes it fail with ENOSYS): there sheepr takes the kill fallback on every leg.
fn pidfd_works() -> bool {
    #[cfg(target_os = "linux")]
    {
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, libc::getpid(), 0) };
        if fd >= 0 {
            unsafe { libc::close(fd as i32) };
            return true;
        }
    }
    false
}

/// The forced-ENOSYS leg exists only where pidfd does.
fn enosys_legs() -> &'static [bool] {
    if cfg!(target_os = "linux") {
        &[false, true]
    } else {
        &[false]
    }
}

/// A decoy process, started by the test (never part of a job).
struct Decoy {
    pid: i32,
    /// its identity, read when its pid is found
    id: u64,
    file: std::path::PathBuf,
    child: std::process::Child,
}
impl Decoy {
    fn start(tag: &str) -> Self {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let file = std::env::temp_dir().join(format!("sr-s3-decoy-{}-{tag}-{n}", std::process::id()));
        let _ = std::fs::remove_file(&file);
        let _ = std::fs::remove_file(file.with_extension("log"));
        let child = Command::new(fixture()).arg("decoy").arg(&file).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        let t = Instant::now();
        let pid = loop {
            if let Some(p) = std::fs::read_to_string(&file).ok().and_then(|s| s.trim().parse().ok()) {
                break p;
            }
            assert!(t.elapsed() < Duration::from_secs(10), "the decoy never started");
            std::thread::sleep(Duration::from_millis(5));
        };
        let id = sheepr::ident::identity(pid).expect("the decoy's identity");
        Decoy { pid, id, file, child }
    }
    fn log_path(&self) -> std::path::PathBuf {
        std::path::PathBuf::from(format!("{}.log", self.file.display()))
    }
    fn signals(&self) -> Vec<String> {
        std::fs::read_to_string(self.log_path()).map(|s| s.lines().map(String::from).collect()).unwrap_or_default()
    }
    /// Not killed: the decoy is the test's child, so a killed decoy is a zombie until reaped
    /// (kill(pid, 0) would still succeed on it).
    fn not_killed(&mut self) -> Result<(), String> {
        match self.child.try_wait() {
            Ok(None) => Ok(()),
            other => Err(format!("{other:?}")),
        }
    }
}
impl Drop for Decoy {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.file);
        let _ = std::fs::remove_file(self.log_path());
    }
}

/// A job whose escapee records itself; cleanup by recorded identity.
struct Job {
    marker: String,
    rec: std::path::PathBuf,
}
impl Job {
    fn new() -> Self {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let marker = format!("21.{}{:06}", std::process::id(), 500 + n);
        let rec = std::env::temp_dir().join(format!("sr-s3-{marker}"));
        let _ = std::fs::remove_file(&rec);
        Job { marker, rec }
    }
    fn recorded(&self) -> Vec<(i32, u64)> {
        std::fs::read_to_string(&self.rec)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| {
                let mut w = l.split_whitespace();
                Some((w.next()?.parse().ok()?, w.next()?.parse().ok()?))
            })
            .collect()
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        for (p, id) in self.recorded() {
            send(p, id, libc::SIGKILL);
        }
        let _ = std::fs::remove_file(&self.rec);
    }
}

/// PLAN.md §3.3: every signal goes to the process the member was, never to one that has its pid
/// now. Debug seam SHEEPR_TEST_REUSE_PID sends each signal to the decoy's pid instead (the
/// member's pid "reused" by the decoy), keeping the member's identity: the decoy must get
/// nothing (no TERM or CONT in its log, not stopped, not killed). On Linux also through the
/// fallback path (debug seam SHEEPR_TEST_PIDFD_ENOSYS: pidfd_open reports ENOSYS).
#[test]
fn s3_a_reused_pid_gets_no_signal() {
    for &enosys in enosys_legs() {
        let mut decoy = Decoy::start("reuse");
        let siglog = log_path("reuse");
        // Linux's identity is the start time in clock ticks (10 ms): a decoy and a member started
        // in the same tick share it, and the check then rightly passes. A real reuse of the same
        // pid within one tick cannot happen, so the decoy starts a few ticks earlier (control below).
        std::thread::sleep(Duration::from_millis(40));
        let j = Job::new();
        let mut c = Command::new(sheepr());
        c.args(["run", "--grace", "100ms", "--", fixture(), "escape", &j.marker])
            .arg(&j.rec)
            .env("SHEEPR_TEST_REUSE_PID", decoy.pid.to_string())
            .env("SHEEPR_TEST_DEADLINE_MS", "300")
            .env("SHEEPR_TEST_SIGNAL_LOG", &siglog)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if enosys {
            c.env("SHEEPR_TEST_PIDFD_ENOSYS", "1");
        }
        let st = c.status().unwrap();
        std::thread::sleep(Duration::from_millis(100));
        let rec = j.recorded();
        assert_eq!(rec.len(), 1, "enosys={enosys}: the escapee was not created");
        assert_ne!(
            sheepr::ident::identity(decoy.pid),
            Some(rec[0].1),
            "enosys={enosys}: control: the decoy has the member's identity, so the check cannot tell them apart"
        );
        let log = signal_log(&siglog);
        let _ = std::fs::remove_file(&siglog);
        if let Err(e) = decoy.not_killed() {
            panic!("enosys={enosys}: the decoy was killed: {e}");
        }
        assert_ne!(state(decoy.pid), Some('T'), "enosys={enosys}: the decoy was stopped");
        assert!(decoy.signals().is_empty(), "enosys={enosys}: the decoy got signals {:?}", decoy.signals());
        // control: the member was never signalled either, so the kill cannot end clean
        assert_eq!(st.code(), Some(125), "enosys={enosys}: the seam did not redirect the signals");
        // control: the path the cell claims to cover was the one taken
        let path = if pidfd_works() && !enosys { "pidfd " } else { "kill " };
        assert!(log.iter().any(|l| l.starts_with(path)), "enosys={enosys}: the {path}path was never taken: {log:?}");
    }
}

/// PLAN.md §3.3 step 4: the process a STOP landed on by mistake (a pid reused between the check
/// and the kill; debug seam SHEEPR_TEST_WRONG_FREEZE) is resumed, whether or not it was
/// stopped before: its prior state cannot be known, and leaving it stopped for good is the
/// worse error. A running decoy and a pre-stopped one both end running.
#[test]
fn s3_a_wrong_freeze_is_rolled_back() {
    for pre_stopped in [false, true] {
        let mut decoy = Decoy::start("freeze");
        let siglog = log_path("freeze");
        if pre_stopped {
            assert!(send(decoy.pid, decoy.id, libc::SIGSTOP), "the decoy is gone");
            let t = Instant::now();
            while state(decoy.pid) != Some('T') {
                assert!(t.elapsed() < Duration::from_secs(5), "the decoy did not stop");
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        let j = Job::new();
        let st = Command::new(sheepr())
            .args(["run", "--grace", "0", "--", fixture(), "escape", &j.marker])
            .arg(&j.rec)
            .env("SHEEPR_TEST_WRONG_FREEZE", decoy.pid.to_string())
            .env("SHEEPR_TEST_SIGNAL_LOG", &siglog)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(st.code(), Some(0), "pre_stopped={pre_stopped}: the job did not end clean");
        let log = signal_log(&siglog);
        let _ = std::fs::remove_file(&siglog);
        if let Err(e) = decoy.not_killed() {
            panic!("pre_stopped={pre_stopped}: the decoy was killed: {e}");
        }
        let stopped = state(decoy.pid) == Some('T');
        {
            assert!(!stopped, "pre_stopped={pre_stopped}: the decoy our STOP landed on was left stopped (no rollback): {log:?}");
            let recorded = format!("record {}", decoy.pid);
            assert!(log.iter().any(|l| l == &recorded), "the STOP that landed on the decoy was not recorded for the rollback: {log:?}");
            let rolled = format!("rollback {}", decoy.pid);
            assert!(log.iter().any(|l| l == &rolled), "the rollback never considered the decoy: {log:?}");
            assert!(
                decoy.signals().iter().any(|s| s == &libc::SIGCONT.to_string()),
                "control: the decoy never got the freeze's rollback CONT, so the seam did not put it in the freeze"
            );
        }
    }
}

/// PLAN.md §3.3 step 4: the rollback resumes only the very process our STOP landed on (its
/// identity is read right after the STOP). A process that took the pid later cannot have got
/// it: the member got the STOP, died, and its pid went to a stranger that someone else
/// stopped. Debug seam SHEEPR_TEST_FREEZE_PID_REUSED records the wrong freeze's STOP as
/// landing on another process than the decoy (the seam's own STOP stands in for the other
/// actor's): the decoy must stay stopped.
#[test]
fn s3_a_process_started_after_our_stop_is_not_rolled_back() {
    let mut decoy = Decoy::start("late");
    let siglog = log_path("late");
    let j = Job::new();
    let st = Command::new(sheepr())
        .args(["run", "--grace", "0", "--", fixture(), "escape", &j.marker])
        .arg(&j.rec)
        .env("SHEEPR_TEST_WRONG_FREEZE", decoy.pid.to_string())
        .env("SHEEPR_TEST_FREEZE_PID_REUSED", "1")
        .env("SHEEPR_TEST_SIGNAL_LOG", &siglog)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    std::thread::sleep(Duration::from_millis(100));
    let log = signal_log(&siglog);
    let _ = std::fs::remove_file(&siglog);
    assert_eq!(st.code(), Some(0));
    if let Err(e) = decoy.not_killed() {
        panic!("the decoy was killed: {e}");
    }
    let frozen = format!("kill {} {}", decoy.pid, libc::SIGSTOP);
    assert!(log.iter().any(|l| l == &frozen), "control: the decoy never reached the freeze: {log:?}");
    assert_eq!(state(decoy.pid), Some('T'), "a process that started after our STOP was resumed by the rollback: {log:?}");
}

/// PLAN.md §3.3 step 4: the rollback resumes only a process that fails the identity check; a
/// job member sheepr froze stays frozen until its KILL (a resumed member could fork). With
/// `--grace 0` sheepr sends a member no CONT at all. The rollback considers only STOPs sent by
/// `kill` (macOS; Linux's fallback, forced here by SHEEPR_TEST_PIDFD_ENOSYS); on Linux's pidfd
/// path the leg checks only that a pinned STOP is never recorded.
#[test]
fn s3_frozen_members_are_not_rolled_back() {
    for &enosys in enosys_legs() {
        let j = Job::new();
        let siglog = log_path("members");
        let mut c = Command::new(sheepr());
        c.args(["run", "--grace", "0", "--", fixture(), "escape", &j.marker])
            .arg(&j.rec)
            .env("SHEEPR_TEST_SIGNAL_LOG", &siglog)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if enosys {
            c.env("SHEEPR_TEST_PIDFD_ENOSYS", "1");
        }
        let st = c.status().unwrap();
        let log = signal_log(&siglog);
        let _ = std::fs::remove_file(&siglog);
        let rec = j.recorded();
        assert_eq!(st.code(), Some(0), "enosys={enosys}");
        assert_eq!(rec.len(), 1, "enosys={enosys}: the escapee was not created");
        let unpinned = !pidfd_works() || enosys;
        let path = if unpinned { "kill" } else { "pidfd" };
        let stop = format!("{path} {} {}", rec[0].0, libc::SIGSTOP);
        assert!(log.iter().any(|l| l == &stop), "enosys={enosys}: control: the escapee's STOP did not go by {path}: {log:?}");
        // a STOP that reached its member is never recorded, so the rollback cannot touch it
        let recorded: Vec<&String> = log.iter().filter(|l| l.starts_with("record ")).collect();
        assert!(recorded.is_empty(), "enosys={enosys}: STOPs that reached members were recorded for the rollback: {recorded:?}");
        let rolled: Vec<&String> = log.iter().filter(|l| l.starts_with("rollback ")).collect();
        assert!(rolled.is_empty(), "enosys={enosys}: the rollback resumed frozen members: {rolled:?}");
    }
}

/// Linux: pidfd_open works but pidfd_send_signal fails (a seccomp filter; debug seam
/// SHEEPR_TEST_PIDFD_SEND_ENOSYS): the signal falls back to the check and kill, so the job
/// still ends clean and the escapee is dead.
#[cfg(target_os = "linux")]
#[test]
fn s3_a_failed_pidfd_send_falls_back() {
    if !pidfd_works() {
        eprintln!("skipped: pidfd_open does not work here, so no pidfd send can fail");
        return;
    }
    let j = Job::new();
    let siglog = log_path("send");
    // --grace 0: a TERM in the grace would end the escapee before any STOP or KILL is needed
    let st = Command::new(sheepr())
        .args(["run", "--grace", "0", "--", fixture(), "escape", &j.marker])
        .arg(&j.rec)
        .env("SHEEPR_TEST_PIDFD_SEND_ENOSYS", "1")
        .env("SHEEPR_TEST_SIGNAL_LOG", &siglog)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    let log = signal_log(&siglog);
    let _ = std::fs::remove_file(&siglog);
    let rec = j.recorded();
    assert_eq!(rec.len(), 1, "the escapee was not created");
    assert!(log.iter().any(|l| l.starts_with("pidfd ")), "control: the pidfd path was not taken: {log:?}");
    let fallback_kill = format!("kill {} {}", rec[0].0, libc::SIGKILL);
    assert!(log.iter().any(|l| l == &fallback_kill), "the escapee's KILL did not go through the fallback: {log:?}");
    assert_eq!(st.code(), Some(0), "the kill did not end clean after a failed pidfd send: {log:?}");
    assert!(!sheepr::ident::same(rec[0].0, rec[0].1), "the escapee survived");
}

/// The test door (`common::send`) signals only the recorded process: a live pid whose identity
/// is not the recorded one gets nothing, and pid 1 or less is refused. Signal 0 only.
#[test]
fn s3_the_test_door_refuses_a_stale_identity() {
    let me = unsafe { libc::getpid() };
    let id = sheepr::ident::identity(me).expect("own identity");
    assert!(send(me, id, 0), "control: the recorded identity is accepted");
    assert!(!send(me, id.wrapping_add(1), 0), "a live pid with another identity was signalled");
    assert!(std::panic::catch_unwind(|| send(1, id, 0)).is_err(), "pid 1 was not refused");
}

/// The other two doors (signal 0 only): a group signal needs the recorded leader. The child door
/// is checked only while the child lives: after the reap its pid no longer exists, so even an
/// unchecked kill fails (ESRCH), and only a reused pid could show the difference.
#[test]
fn s3_the_group_and_child_doors_refuse_stale_targets() {
    use std::os::unix::process::CommandExt;
    let mut c = Command::new("/bin/sleep").arg("29.4242").process_group(0).spawn().unwrap();
    let pg = c.id() as i32;
    let id = sheepr::ident::identity(pg).expect("the leader's identity");
    let right = common::send_group(pg, id, 0);
    let wrong = common::send_group(pg, id.wrapping_add(1), 0);
    let live = common::send_child(&mut c, 0);
    let _ = c.kill();
    let _ = c.wait();
    assert!(right, "control: the recorded leader's group is accepted");
    assert!(!wrong, "a group whose leader has another identity was signalled");
    assert!(live, "control: a live child is accepted");
}
