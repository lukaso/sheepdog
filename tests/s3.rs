//! Phase 1, step S3 (PHASE1.md): identity before every signal (Linux: pidfd, with the
//! ENOSYS/EPERM fallback; macOS: the uniqueid re-check) and the wrong-freeze rollback
//! (PLAN.md §3.3 steps 2 and 4). A decoy is a process outside any job that logs every catchable
//! signal it gets; a SIGSTOP shows as state T, a SIGKILL as its death.

use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

static SEQ: AtomicUsize = AtomicUsize::new(0);

fn sheepdog() -> &'static str {
    env!("CARGO_BIN_EXE_sheepdog")
}
fn fixture() -> &'static str {
    env!("CARGO_BIN_EXE_sd-fixture")
}

fn state(pid: i32) -> Option<char> {
    let out = Command::new("ps").args(["-o", "stat=", "-p", &pid.to_string()]).output().ok()?;
    String::from_utf8_lossy(&out.stdout).trim().chars().next()
}
fn alive(pid: i32) -> bool {
    unsafe { libc::kill(pid, 0) == 0 }
}

/// A decoy process, started by the test (never part of a job).
struct Decoy {
    pid: i32,
    file: std::path::PathBuf,
    child: std::process::Child,
}
impl Decoy {
    fn start(tag: &str) -> Self {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let file = std::env::temp_dir().join(format!("sd-s3-decoy-{}-{tag}-{n}", std::process::id()));
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
        Decoy { pid, file, child }
    }
    fn log_path(&self) -> std::path::PathBuf {
        std::path::PathBuf::from(format!("{}.log", self.file.display()))
    }
    fn signals(&self) -> Vec<String> {
        std::fs::read_to_string(self.log_path()).map(|s| s.lines().map(String::from).collect()).unwrap_or_default()
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
        let rec = std::env::temp_dir().join(format!("sd-s3-{marker}"));
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
            if sheepdog::ident::same(p, id) {
                unsafe { libc::kill(p, libc::SIGKILL) };
            }
        }
        let _ = std::fs::remove_file(&self.rec);
    }
}

/// PLAN.md §3.3: every signal goes to the process the member was, never to one that has its pid
/// now. Debug seam SHEEPDOG_TEST_REUSE_PID sends each signal to the decoy's pid instead (the
/// member's pid "reused" by the decoy), keeping the member's identity: the decoy must get
/// nothing (no TERM or CONT in its log, not stopped, not killed). On Linux also through the
/// fallback path (debug seam SHEEPDOG_TEST_PIDFD_ENOSYS: pidfd_open reports ENOSYS).
#[test]
fn s3_a_reused_pid_gets_no_signal() {
    for enosys in [false, true] {
        let decoy = Decoy::start("reuse");
        // Linux's identity is the start time in clock ticks (10 ms): a decoy and a member started
        // in the same tick share it, and the check then rightly passes. A real reuse of the same
        // pid within one tick cannot happen, so the decoy starts a few ticks earlier (control below).
        std::thread::sleep(Duration::from_millis(40));
        let j = Job::new();
        let mut c = Command::new(sheepdog());
        c.args(["run", "--grace", "100ms", "--", fixture(), "escape", &j.marker])
            .arg(&j.rec)
            .env("SHEEPDOG_TEST_REUSE_PID", decoy.pid.to_string())
            .env("SHEEPDOG_TEST_DEADLINE_MS", "300")
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if enosys {
            c.env("SHEEPDOG_TEST_PIDFD_ENOSYS", "1");
        }
        let st = c.status().unwrap();
        std::thread::sleep(Duration::from_millis(100));
        let rec = j.recorded();
        assert_eq!(rec.len(), 1, "enosys={enosys}: the escapee was not created");
        assert_ne!(
            sheepdog::ident::identity(decoy.pid),
            Some(rec[0].1),
            "enosys={enosys}: control: the decoy has the member's identity, so the check cannot tell them apart"
        );
        assert!(alive(decoy.pid), "enosys={enosys}: the decoy was killed");
        assert_ne!(state(decoy.pid), Some('T'), "enosys={enosys}: the decoy was stopped");
        assert!(decoy.signals().is_empty(), "enosys={enosys}: the decoy got signals {:?}", decoy.signals());
        // control: the member was never signalled either, so the kill cannot end clean
        assert_eq!(st.code(), Some(125), "enosys={enosys}: the seam did not redirect the signals");
    }
}

/// PLAN.md §3.3 step 4: a process that fails the identity check after the freeze (its STOP may
/// have landed on a reused pid) gets SIGCONT only if it was not stopped before sheepdog's STOP.
/// Debug seam SHEEPDOG_TEST_WRONG_FREEZE puts the decoy into the freeze as such a process.
#[test]
fn s3_a_wrong_freeze_is_rolled_back_only_for_a_running_process() {
    for pre_stopped in [false, true] {
        let decoy = Decoy::start("freeze");
        if pre_stopped {
            unsafe { libc::kill(decoy.pid, libc::SIGSTOP) };
            let t = Instant::now();
            while state(decoy.pid) != Some('T') {
                assert!(t.elapsed() < Duration::from_secs(5), "the decoy did not stop");
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        let j = Job::new();
        let st = Command::new(sheepdog())
            .args(["run", "--grace", "0", "--", fixture(), "escape", &j.marker])
            .arg(&j.rec)
            .env("SHEEPDOG_TEST_WRONG_FREEZE", decoy.pid.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(st.code(), Some(0), "pre_stopped={pre_stopped}: the job did not end clean");
        assert!(alive(decoy.pid), "pre_stopped={pre_stopped}: the decoy was killed");
        let stopped = state(decoy.pid) == Some('T');
        if pre_stopped {
            assert!(stopped, "a decoy stopped before sheepdog's STOP was resumed by the rollback");
        } else {
            assert!(!stopped, "a running decoy caught in the freeze was left stopped (no rollback)");
            assert!(
                decoy.signals().iter().any(|s| s == &libc::SIGCONT.to_string()),
                "control: the decoy never got the freeze's rollback CONT, so the seam did not put it in the freeze"
            );
        }
    }
}
