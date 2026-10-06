//! Phase 1, step S4 (PHASE1.md §2 and S4): the pty harness, and INT/HUP forwarding.
//!
//! Safety rules of this file (the 2026-09-26 host incident was a test that sent `kill(-1)`):
//! - a pid is signalled only after its recorded identity is re-checked (`send`);
//! - a process group is signalled only if this test created it and its leader still has the
//!   recorded identity (`send_group`);
//! - nothing is ever sent to a pid of 1 or less.
//!
//! Readiness only (record files, the shell's report), never a fixed sleep before an action.

use sheepr::ident::{identity, same};
use std::os::unix::io::RawFd;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
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

mod common;
use common::{send, send_group};

/// The negative numbers in a hint (the `kill -INT -<pgid>` targets), as values.
fn targets(text: &str) -> Vec<i64> {
    text.split(|c: char| c.is_whitespace() || "(),.".contains(c)).filter_map(|w| w.strip_prefix('-')?.parse::<i64>().ok()).collect()
}

/// Poll `f` until it holds (true) or `limit` passes (false).
fn wait_for_opt(limit: Duration, mut f: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while !f() {
        if start.elapsed() > limit {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    true
}

fn wait_for<T>(what: &str, limit: Duration, mut f: impl FnMut() -> Option<T>) -> T {
    let start = Instant::now();
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(start.elapsed() < limit, "timed out waiting for: {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The pids whose argv carries `marker` as a whole word (sheepr's own argv included).
fn marked(marker: &str) -> Vec<i32> {
    let out = Command::new("ps").args(["-Ao", "pid=,args="]).output().expect("ps");
    assert!(out.status.success(), "ps failed");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| l.split_whitespace().skip(1).any(|w| w == marker))
        .filter_map(|l| l.split_whitespace().next()?.parse().ok())
        .collect()
}

/// `/bin/sleep <marker>` processes only.
fn sleeps(marker: &str) -> Vec<i32> {
    let out = Command::new("ps").args(["-Ao", "pid=,args="]).output().expect("ps");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let w: Vec<&str> = l.split_whitespace().collect();
            (w.len() >= 3 && w[1].ends_with("sleep") && w[2] == marker).then(|| w[0].parse().ok()).flatten()
        })
        .collect()
}

/// The sheepr child of `relay` (the relay path was taken), with its identity.
fn supervisor_of(relay: u32) -> Option<(i32, u64)> {
    let out = Command::new("ps").args(["-Ao", "pid=,ppid=,args="]).output().expect("ps");
    String::from_utf8_lossy(&out.stdout).lines().find_map(|l| {
        let w: Vec<&str> = l.split_whitespace().collect();
        let p: i32 = w.first()?.parse().ok()?;
        (w.len() >= 3 && w[1] == relay.to_string() && w[2].ends_with("sheepr")).then(|| Some((p, identity(p)?))).flatten()
    })
}

fn new_marker() -> String {
    format!("28.{}{:06}", std::process::id(), SEQ.fetch_add(1, Ordering::SeqCst))
}

/// One `sr-fixture counter` job: a root and one escapee, both counting INT and HUP.
struct Job {
    marker: String,
    rec: PathBuf,
}

impl Job {
    fn new() -> Self {
        let marker = new_marker();
        let rec = std::env::temp_dir().join(format!("sr-s4-{marker}"));
        let _ = std::fs::remove_file(&rec);
        let _ = std::fs::remove_file(Self::sig_path(&rec));
        eprintln!("job {marker}: {}", std::thread::current().name().unwrap_or("?"));
        Job { marker, rec }
    }
    fn sig_path(rec: &PathBuf) -> PathBuf {
        PathBuf::from(format!("{}.sig", rec.display()))
    }
    fn args(&self) -> Vec<String> {
        vec![fixture().into(), "counter".into(), self.marker.clone(), self.rec.display().to_string()]
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
    /// Wait until the root recorded the escapee and itself: (escapee, root).
    fn ready(&self) -> ((i32, u64), (i32, u64)) {
        wait_for("the counter job", Duration::from_secs(15), || {
            let r = self.recorded();
            (r.len() >= 2).then(|| (r[0], r[1]))
        })
    }
    /// (INT, HUP) counted by `pid`.
    fn counts(&self, pid: i32) -> (usize, usize) {
        let s = std::fs::read_to_string(Self::sig_path(&self.rec)).unwrap_or_default();
        let n = |name: &str| s.lines().filter(|l| *l == format!("{name} {pid}")).count();
        (n("INT"), n("HUP"))
    }
    /// Wait until `ok` holds for the counts (bounded, no panic), then let 500 ms (two scan
    /// ticks) pass, so a second delivery would show too. Returns (root, escapee) counts.
    fn settle(&self, root: i32, esc: i32, ok: impl Fn((usize, usize), (usize, usize)) -> bool) -> ((usize, usize), (usize, usize)) {
        let start = Instant::now();
        while !ok(self.counts(root), self.counts(esc)) && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(500));
        (self.counts(root), self.counts(esc))
    }
}

impl Job {
    /// SIGKILL every recorded member (identity-checked) and wait until they are gone.
    fn kill(&self) {
        let rec = self.recorded();
        for &(p, id) in &rec {
            if p > 1 {
                send(p, id, libc::SIGKILL);
            }
        }
        let start = Instant::now();
        while rec.iter().any(|&(p, id)| same(p, id)) && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        self.kill();
        // a job whose record is incomplete (a cell that failed before `ready`) is found by its
        // marker: only `sr-fixture counter <marker>` processes, identity read at the scan
        let out = Command::new("ps").args(["-Ao", "pid=,args="]).output();
        if let Ok(out) = out {
            for l in String::from_utf8_lossy(&out.stdout).lines() {
                let w: Vec<&str> = l.split_whitespace().collect();
                if w.len() >= 4 && w[1].ends_with("sr-fixture") && w[2] == "counter" && w[3] == self.marker {
                    if let Some((p, id)) = w[0].parse::<i32>().ok().and_then(|p| Some((p, identity(p)?))) {
                        if p > 1 {
                            send(p, id, libc::SIGKILL);
                        }
                    }
                }
            }
        }
        let _ = std::fs::remove_file(&self.rec);
        let _ = std::fs::remove_file(Self::sig_path(&self.rec));
    }
}

/// A pseudo-terminal with one session on it (PHASE1.md §2). The child is the session leader and
/// the pty is its controlling terminal. The master is drained on every wait and closed on drop.
struct Pty {
    master: Option<RawFd>,
    child: Child,
    child_id: u64,
    report: PathBuf,
    out: Vec<u8>,
    /// the job's leader (the shell's child), once reported: TERMed on drop
    job: Option<(i32, u64)>,
}

impl Pty {
    /// Start `prog args` as the session leader of a new pty.
    fn leader(prog: &str, args: &[String]) -> Pty {
        let (mut m, mut s) = (0, 0);
        let r = unsafe { libc::openpty(&mut m, &mut s, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) };
        assert_eq!(r, 0, "openpty failed: {}", std::io::Error::last_os_error());
        unsafe {
            libc::fcntl(m, libc::F_SETFD, libc::FD_CLOEXEC);
            libc::fcntl(s, libc::F_SETFD, libc::FD_CLOEXEC);
            libc::fcntl(m, libc::F_SETFL, libc::fcntl(m, libc::F_GETFL) | libc::O_NONBLOCK);
        }
        let report = std::env::temp_dir().join(format!("sr-pty-{}", new_marker()));
        let _ = std::fs::remove_file(&report);
        let mut c = Command::new(prog);
        c.args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        unsafe {
            c.pre_exec(move || {
                if libc::setsid() < 0 || libc::ioctl(s, libc::TIOCSCTTY as _, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                for fd in 0..3 {
                    libc::dup2(s, fd);
                }
                for sig in [libc::SIGINT, libc::SIGQUIT, libc::SIGTSTP, libc::SIGTTIN, libc::SIGTTOU, libc::SIGHUP, libc::SIGTERM] {
                    libc::signal(sig, libc::SIG_DFL);
                }
                let mut none: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut none);
                libc::sigprocmask(libc::SIG_SETMASK, &none, std::ptr::null_mut());
                Ok(())
            });
        }
        let child = c.spawn().expect("spawn under the pty");
        unsafe { libc::close(s) };
        let child_id = identity(child.id() as i32).expect("the session leader's identity");
        Pty { master: Some(m), child, child_id, report, out: Vec::new(), job: None }
    }

    /// `sr-fixture shell REPORT opts... prog args...` as the session leader.
    fn shell(opts: &[&str], cmd: &[String]) -> Pty {
        let report = std::env::temp_dir().join(format!("sr-pty-{}", new_marker()));
        let mut args = vec!["shell".to_string(), report.display().to_string()];
        args.extend(opts.iter().map(|s| s.to_string()));
        args.extend(cmd.iter().cloned());
        let mut p = Pty::leader(fixture(), &args);
        p.report = report;
        p
    }

    fn drain(&mut self) {
        if let Some(m) = self.master {
            let mut buf = [0u8; 4096];
            loop {
                let n = unsafe { libc::read(m, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
                if n <= 0 {
                    break;
                }
                self.out.extend_from_slice(&buf[..n as usize]);
            }
        }
    }

    fn write(&mut self, bytes: &[u8]) {
        let m = self.master.expect("master open");
        let n = unsafe { libc::write(m, bytes.as_ptr() as *const libc::c_void, bytes.len()) };
        assert_eq!(n, bytes.len() as isize, "write to the pty master failed");
    }

    fn close_master(&mut self) {
        if let Some(m) = self.master.take() {
            unsafe { libc::close(m) };
        }
    }

    fn lines(&self) -> Vec<String> {
        std::fs::read_to_string(&self.report).unwrap_or_default().lines().map(String::from).collect()
    }

    /// Wait for a report line that satisfies `f` (the shell's report), draining the master.
    fn wait_line(&mut self, what: &str, limit: Duration, f: impl Fn(&str) -> bool) -> Option<String> {
        let start = Instant::now();
        loop {
            self.drain();
            if let Some(l) = self.lines().into_iter().find(|l| f(l)) {
                return Some(l);
            }
            if start.elapsed() > limit {
                eprintln!("no report line for: {what}; report so far: {:?}", self.lines());
                return None;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// The job's leader (and group id), as the shell reported it; recorded with its identity.
    fn started(&mut self) -> (i32, u64) {
        let l = self.wait_line("started", Duration::from_secs(15), |l| l.starts_with("started ")).expect("the shell never started the job");
        // the shell reads the identity before it can reap the job, so it is the job's
        let mut w = l["started ".len()..].split_whitespace();
        let pid: i32 = w.next().and_then(|p| p.parse().ok()).expect("started <pid> <identity>");
        let id: u64 = w.next().and_then(|p| p.parse().ok()).expect("started <pid> <identity>");
        assert!(pid > 1 && id != 0, "the shell reported pid {pid}, identity {id}");
        self.job = Some((pid, id));
        (pid, id)
    }

    /// The shell's final report for the job: `exited N` or `signaled N`.
    fn outcome(&mut self, limit: Duration) -> Option<String> {
        self.wait_line("the job's end", limit, |l| l.starts_with("exited ") || l.starts_with("signaled "))
    }

    fn fg(&self) {
        std::fs::write(format!("{}.fg", self.report.display()), "").unwrap();
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        if let Some((p, id)) = self.job {
            send(p, id, libc::SIGTERM);
            // a stopped job keeps a TERM pending: continue it so the TERM acts
            send(p, id, libc::SIGCONT);
        }
        self.close_master();
        let start = Instant::now();
        while self.child.try_wait().ok().flatten().is_none() && start.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(10));
        }
        if self.child.try_wait().ok().flatten().is_none() {
            send(self.child.id() as i32, self.child_id, libc::SIGKILL);
            let _ = self.child.wait();
        }
        let _ = std::fs::remove_file(&self.report);
        let _ = std::fs::remove_file(format!("{}.fg", self.report.display()));
    }
}

fn run_args(flags: &[&str], cmd: &[String]) -> Vec<String> {
    let mut v = vec![sheepr().to_string(), "run".into()];
    v.extend(flags.iter().map(|s| s.to_string()));
    v.push("--".into());
    v.extend(cmd.iter().cloned());
    v
}

/// sheepr in its own process group, no terminal involved; INT/HUP/TERM at their defaults.
fn direct(flags: &[&str], cmd: &[String], env: &[(&str, &str)]) -> (Child, u64) {
    let mut c = Command::new(sheepr());
    c.arg("run").args(flags).arg("--").args(cmd).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    for (k, v) in env {
        c.env(k, v);
    }
    unsafe {
        c.pre_exec(|| {
            for sig in [libc::SIGINT, libc::SIGHUP, libc::SIGTERM, libc::SIGQUIT] {
                libc::signal(sig, libc::SIG_DFL);
            }
            Ok(())
        });
    }
    c.process_group(0);
    let child = c.spawn().unwrap();
    let id = identity(child.id() as i32).expect("sheepr's identity");
    (child, id)
}

fn wait_bounded(c: &mut Child, limit: Duration) -> Option<std::process::ExitStatus> {
    let start = Instant::now();
    loop {
        if let Some(st) = c.try_wait().unwrap() {
            return Some(st);
        }
        if start.elapsed() > limit {
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// End a direct sheepr by TERM (identity-checked) and return its stderr. The job's members
/// share that stderr pipe, so they are killed (by recorded identity) before it is read: a
/// member left running would keep the read open forever.
fn end(mut c: Child, id: u64, job: &Job) -> String {
    send(c.id() as i32, id, libc::SIGTERM);
    let st = wait_bounded(&mut c, Duration::from_secs(15));
    if st.is_none() {
        send(c.id() as i32, id, libc::SIGKILL);
        let _ = c.wait();
    }
    job.kill();
    let mut err = String::new();
    if let Some(mut e) = c.stderr.take() {
        use std::io::Read;
        let _ = e.read_to_string(&mut err);
    }
    err
}

// ---- the harness itself ------------------------------------------------------------------

/// PHASE1.md §2, the harness's control cell (no sheepr): ctrl-Z stops the job and the
/// shell sees it; after `fg`, ctrl-C ends it by INT. If this fails, no pty cell measures
/// anything.
#[test]
fn pty_control_ctrl_z_stops_and_ctrl_c_ends_a_plain_job() {
    let m = new_marker();
    let mut pty = Pty::shell(&[], &["/bin/sleep".to_string(), m.clone()]);
    pty.started();
    wait_for("the sleep", Duration::from_secs(15), || (sleeps(&m).len() == 1).then_some(()));
    pty.write(b"\x1a");
    let stopped = pty.wait_line("stopped", Duration::from_secs(10), |l| l.starts_with("stopped "));
    pty.fg();
    let continued = pty.wait_line("continued", Duration::from_secs(10), |l| l == "continued");
    pty.write(b"\x03");
    let end = pty.outcome(Duration::from_secs(10));
    for p in sleeps(&m) {
        if let Some(id) = identity(p) {
            send(p, id, libc::SIGKILL);
        }
    }
    assert_eq!(stopped, Some(format!("stopped {}", libc::SIGTSTP)), "ctrl-Z did not stop the job");
    assert!(continued.is_some(), "the shell did not continue the job");
    assert_eq!(end, Some(format!("signaled {}", libc::SIGINT)), "ctrl-C did not end the job by INT");
}

// ---- cell 22: each member counts exactly one INT (or HUP) ----------------------------------

/// Cell 22(a) and (e): a foreground job in a pty, stdin from /dev/null, ctrl-C. The root got
/// the tty's INT; the escapee (its own session) gets exactly one, from sheepr.
#[test]
fn cell22a_ctrl_c_in_the_foreground_reaches_each_member_once() {
    let job = Job::new();
    let mut pty = Pty::shell(&["null-stdin"], &run_args(&[], &job.args()));
    pty.started();
    let ((esc, _), (root, _)) = job.ready();
    pty.write(b"\x03");
    let (r, e) = job.settle(root, esc, |r, e| r.0 >= 1 && e.0 >= 1);
    assert_eq!(r.0, 1, "the root must count exactly one INT (the tty's)");
    assert_eq!(e.0, 1, "the escapee must count exactly one INT (sheepr's)");
}

/// Cell 22(b) and (e): a background job in a pty, `kill -INT -<pgid>`.
#[test]
fn cell22b_a_group_int_to_a_background_job_reaches_each_member_once() {
    let job = Job::new();
    let mut pty = Pty::shell(&["bg"], &run_args(&[], &job.args()));
    let (pg, pg_id) = pty.started();
    let ((esc, _), (root, _)) = job.ready();
    assert!(send_group(pg, pg_id, libc::SIGINT), "the group INT was not sent");
    let (r, e) = job.settle(root, esc, |r, e| r.0 >= 1 && e.0 >= 1);
    assert_eq!((r.0, e.0), (1, 1), "each member must count exactly one INT (root, escapee)");
}

/// Cell 22(c) and (e): no controlling terminal (as liveapp and CI run it), a group INT and
/// then a group HUP. sheepr is not the session leader here.
#[test]
fn cell22c_group_signals_without_a_terminal_reach_each_member_once() {
    let job = Job::new();
    let pidfile = std::env::temp_dir().join(format!("sr-s4-pid-{}", job.marker));
    let mut c = Command::new(fixture());
    c.args(["nosession", pidfile.to_str().unwrap()]).args(run_args(&[], &job.args())).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    unsafe {
        c.pre_exec(|| {
            for sig in [libc::SIGINT, libc::SIGHUP, libc::SIGTERM] {
                libc::signal(sig, libc::SIG_DFL);
            }
            Ok(())
        });
    }
    let mut outer = c.spawn().unwrap();
    // "<pid> <identity>": nosession reads the identity before it can reap sheepr
    let (pg, pg_id): (i32, u64) = wait_for("the pid file", Duration::from_secs(15), || {
        let s = std::fs::read_to_string(&pidfile).ok()?;
        let mut w = s.split_whitespace();
        Some((w.next()?.parse().ok()?, w.next()?.parse().ok()?))
    });
    let _ = std::fs::remove_file(&pidfile);
    let ((esc, _), (root, _)) = job.ready();
    assert!(send_group(pg, pg_id, libc::SIGINT));
    let (ri, ei) = job.settle(root, esc, |r, e| r.0 >= 1 && e.0 >= 1);
    assert!(send_group(pg, pg_id, libc::SIGHUP));
    let (rh, eh) = job.settle(root, esc, |r, e| r.1 >= 1 && e.1 >= 1);
    let alive = same(pg, pg_id);
    send(pg, pg_id, libc::SIGTERM);
    let _ = wait_bounded(&mut outer, Duration::from_secs(15));
    assert_eq!((ri.0, ei.0), (1, 1), "INT: each member must count exactly one (root, escapee)");
    assert_eq!((rh.1, eh.1), (1, 1), "HUP: each member must count exactly one (root, escapee)");
    assert!(alive, "HUP must not end the job");
}

/// Cell 22(d) and (e): the terminal closes. The kernel sends HUP to the session leader (the
/// shell) only; the shell sends it on to the job's group, as bash does. Each member counts one.
#[test]
fn cell22d_a_closed_terminal_hups_each_member_once() {
    let job = Job::new();
    let mut pty = Pty::shell(&[], &run_args(&[], &job.args()));
    let (sd, sr_id) = pty.started();
    let ((esc, _), (root, _)) = job.ready();
    pty.close_master();
    let forwarded = pty.wait_line("the shell's HUP", Duration::from_secs(10), |l| l == "hup");
    let (r, e) = job.settle(root, esc, |r, e| r.1 >= 1 && e.1 >= 1);
    assert!(same(sd, sr_id), "HUP must not end the job");
    assert!(forwarded.is_some(), "control: the shell never got the terminal's HUP");
    assert_eq!((r.1, e.1), (1, 1), "each member must count exactly one HUP (root, escapee)");
}

/// Cell 22(f), the stated limit: INT to the sheepr pid only reaches the escapees, not the
/// root.
#[test]
fn cell22f_int_to_the_pid_only_reaches_the_escapees() {
    let job = Job::new();
    let (c, id) = direct(&[], &job.args(), &[]);
    let ((esc, _), (root, _)) = job.ready();
    assert!(send(c.id() as i32, id, libc::SIGINT));
    let (r, e) = job.settle(root, esc, |_, e| e.0 >= 1);
    let alive = same(c.id() as i32, id);
    end(c, id, &job);
    assert!(alive, "INT must not end the job");
    assert_eq!((r.0, e.0), (0, 1), "INT to the pid: (root, escapee) counts");
}

/// Cell 22'(g): sheepr is the session leader of a pty and the master closes. The kernel
/// sends HUP to sheepr only, so sheepr forwards it to every member, the root included.
#[test]
fn cell22g_a_session_leader_forwards_hup_to_every_member() {
    let job = Job::new();
    let mut pty = Pty::leader(sheepr(), &run_args(&[], &job.args())[1..]);
    pty.job = Some((pty.child.id() as i32, pty.child_id));
    let ((esc, _), (root, _)) = job.ready();
    pty.close_master();
    let (r, e) = job.settle(root, esc, |r, e| r.1 >= 1 && e.1 >= 1);
    assert!(same(pty.child.id() as i32, pty.child_id), "HUP must not end the job");
    assert_eq!((r.1, e.1), (1, 1), "each member must count exactly one HUP (root, escapee)");
}

/// S4, the relay variant: ctrl-C reaches the relay, the supervisor and the root (one
/// process group). The relay never forwards INT, so the escapee counts one, not two.
#[test]
fn a_relayed_ctrl_c_reaches_the_escapee_once() {
    let job = Job::new();
    let bg = new_marker();
    let mut cmd = vec![fixture().to_string(), "bg-then-exec".into(), bg.clone()];
    cmd.extend(run_args(&[], &job.args()));
    let mut pty = Pty::shell(&[], &cmd);
    let (relay, _) = pty.started();
    let ((esc, _), (root, _)) = job.ready();
    let relayed = supervisor_of(relay as u32).is_some();
    pty.write(b"\x03");
    let (r, e) = job.settle(root, esc, |r, e| r.0 >= 1 && e.0 >= 1);
    assert!(relayed, "control: this is not the relay path (no sheepr child of the relay)");
    for p in sleeps(&bg) {
        if let Some(id) = identity(p) {
            send(p, id, libc::SIGKILL);
        }
    }
    assert_eq!((r.0, e.0), (1, 1), "each member must count exactly one INT (root, escapee)");
}

/// S4 (macOS consumption, PHASE1.md §1.1): two INTs to the pid, one after the other, are
/// exactly two forwards. A signal that is seen but not consumed would be forwarded again at
/// every wake.
#[test]
fn two_ints_are_two_forwards() {
    let job = Job::new();
    let (c, id) = direct(&[], &job.args(), &[]);
    let ((esc, _), (root, _)) = job.ready();
    assert!(send(c.id() as i32, id, libc::SIGINT));
    job.settle(root, esc, |_, e| e.0 >= 1);
    assert!(send(c.id() as i32, id, libc::SIGINT));
    std::thread::sleep(Duration::from_millis(300));
    let (r, e) = job.settle(root, esc, |_, e| e.0 >= 2);
    end(c, id, &job);
    assert_eq!((r.0, e.0), (0, 2), "two INTs to the pid: (root, escapee) counts");
}

/// S4 (§1.3, fresh membership at every signal event): an escapee started after the last
/// timed scan still gets the INT. The scan tick is widened to 900 ms (debug seam), and the
/// INT is sent as soon as the escapee exists, so only a rescan at the event can find it.
#[test]
fn an_escapee_newer_than_the_last_scan_gets_the_int() {
    let job = Job::new();
    let (c, id) = direct(&[], &job.args(), &[("SHEEPR_TEST_TICK_MS", "900")]);
    let ((esc, _), (root, _)) = job.ready();
    assert!(send(c.id() as i32, id, libc::SIGINT));
    let start = Instant::now();
    while job.counts(esc).0 == 0 && start.elapsed() < Duration::from_millis(600) {
        std::thread::sleep(Duration::from_millis(5));
    }
    let took = start.elapsed();
    let (_, e) = (job.counts(root), job.counts(esc));
    end(c, id, &job);
    assert_eq!(e.0, 1, "the escapee did not get the INT within {took:?} (a timed scan is 900 ms away)");
}

/// PLAN.md §3.1: the relay never forwards INT, so an INT sent only to the relay's pid reaches
/// no member (the stated pid-only limit, stricter on this path), also with
/// `--forward-int-to-root` (a stated limit: a relay that forwarded it could deliver a ctrl-C
/// twice to an escapee, S4 review round 2). A relay that forwarded INT would double every
/// terminal INT, which a ctrl-C cell cannot always see: the two INTs can merge into one pending
/// signal at the supervisor.
#[test]
fn an_int_to_the_relay_pid_reaches_no_member() {
    for flags in [&[][..], &["--forward-int-to-root"][..]] {
        let job = Job::new();
        let bg = new_marker();
        let mut c = Command::new(fixture());
        c.args(["bg-then-exec", &bg]).args(run_args(flags, &job.args())).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        unsafe {
            c.pre_exec(|| {
                for sig in [libc::SIGINT, libc::SIGHUP, libc::SIGTERM] {
                    libc::signal(sig, libc::SIG_DFL);
                }
                Ok(())
            });
        }
        c.process_group(0);
        let mut relay = c.spawn().unwrap();
        let rid = identity(relay.id() as i32).expect("the relay's identity");
        let ((esc, _), (root, _)) = job.ready();
        let has_supervisor = supervisor_of(relay.id()).is_some();
        assert!(send(relay.id() as i32, rid, libc::SIGINT));
        std::thread::sleep(Duration::from_millis(700));
        let (r, e) = (job.counts(root), job.counts(esc));
        let relay_alive = relay.try_wait().unwrap().is_none();
        send(relay.id() as i32, rid, libc::SIGTERM);
        if wait_bounded(&mut relay, Duration::from_secs(15)).is_none() {
            send(relay.id() as i32, rid, libc::SIGKILL);
            let _ = relay.wait();
        }
        for p in sleeps(&bg) {
            if let Some(id) = identity(p) {
                send(p, id, libc::SIGKILL);
            }
        }
        assert!(has_supervisor, "{flags:?}: control: this is not the relay path (no sheepr child of the relay)");
        assert!(relay_alive, "{flags:?}: an INT ended the relay");
        assert_eq!((r.0, e.0), (0, 0), "{flags:?}: an INT to the relay's pid reached (root, escapee)");
    }
}

/// PHASE1.md §1.3: an INT counts "at any point before sheepr exits", also one that comes
/// during the kill. The root dies of INT by itself (sheepr got none), and while the kill waits
/// out the grace (the escapee ignores TERM), sheepr gets an INT: it dies of INT. Control: the
/// same run without that INT exits 130.
#[test]
fn an_int_during_the_kill_still_counts() {
    for with_int in [false, true] {
        let job = Job::new();
        let term_log = PathBuf::from(format!("{}.term", job.rec.display()));
        let _ = std::fs::remove_file(&term_log);
        let script = format!("\"$0\" term-counter {} '{}'; kill -INT $$; exec /bin/sleep {}", job.marker, job.rec.display(), job.marker);
        let (mut c, id) = direct(&["--quiet", "--grace", "2s"], &["/bin/sh".into(), "-c".into(), script, fixture().into()], &[]);
        wait_for("the grace's TERM", Duration::from_secs(15), || {
            std::fs::read_to_string(&term_log).ok().filter(|s| s.contains("TERM ")).map(|_| ())
        });
        if with_int {
            assert!(send(c.id() as i32, id, libc::SIGINT), "the INT was not sent");
        }
        let st = wait_bounded(&mut c, Duration::from_secs(15));
        if st.is_none() {
            send(c.id() as i32, id, libc::SIGKILL);
            let _ = c.wait();
        }
        let _ = std::fs::remove_file(&term_log);
        use std::os::unix::process::ExitStatusExt;
        if with_int {
            assert_eq!(st.and_then(|s| s.signal()), Some(libc::SIGINT), "an INT during the kill did not count: {st:?}");
        } else {
            assert_eq!(st.and_then(|s| s.code()), Some(130), "control: without sheepr's own INT it exits 130: {st:?}");
        }
    }
}

// ---- the hint (DevEx D9) and its flags ---------------------------------------------------

/// S4 (PLAN.md §3.1, D9): INT to the pid while the root keeps running prints one hint, once
/// per job, naming the real process group (debug seam: the 3 s delay is 300 ms).
#[test]
fn the_pid_only_hint_is_printed_once_with_the_group() {
    let job = Job::new();
    let (c, id) = direct(&[], &job.args(), &[("SHEEPR_TEST_HINT_MS", "300"), ("SHEEPR_TEST_NO_REPORT", "1")]);
    job.ready();
    let pg = c.id() as i32;
    assert!(send(pg, id, libc::SIGINT));
    std::thread::sleep(Duration::from_millis(900));
    assert!(send(pg, id, libc::SIGINT));
    std::thread::sleep(Duration::from_millis(900));
    let err = end(c, id, &job);
    let lines: Vec<&str> = err.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 1, "expected exactly one hint line, got: {err:?}");
    assert_eq!(targets(lines[0]), vec![pg as i64], "the hint must name the process group {pg}: {err:?}");
}

/// S4: `--quiet` suppresses the hint.
#[test]
fn quiet_suppresses_the_hint() {
    let job = Job::new();
    let (c, id) = direct(&["--quiet"], &job.args(), &[("SHEEPR_TEST_HINT_MS", "300"), ("SHEEPR_TEST_NO_REPORT", "1")]);
    job.ready();
    assert!(send(c.id() as i32, id, libc::SIGINT));
    std::thread::sleep(Duration::from_millis(900));
    let err = end(c, id, &job);
    assert!(err.trim().is_empty(), "--quiet printed: {err:?}");
}

/// S4 (PLAN.md §3.1): `--forward-int-to-root` also forwards a pid-only INT to the root, and
/// then no hint is printed (sheepr did deliver it).
#[test]
fn forward_int_to_root_reaches_the_root_and_prints_no_hint() {
    let job = Job::new();
    let (c, id) = direct(&["--forward-int-to-root"], &job.args(), &[("SHEEPR_TEST_HINT_MS", "300"), ("SHEEPR_TEST_NO_REPORT", "1")]);
    let ((esc, _), (root, _)) = job.ready();
    assert!(send(c.id() as i32, id, libc::SIGINT));
    let (r, e) = job.settle(root, esc, |r, e| r.0 >= 1 && e.0 >= 1);
    std::thread::sleep(Duration::from_millis(400));
    let err = end(c, id, &job);
    assert_eq!((r.0, e.0), (1, 1), "(root, escapee) INT counts");
    assert!(err.trim().is_empty(), "a hint was printed although the root got the INT: {err:?}");
}

/// S4 review (A-P2-3): a ctrl-C from the terminal reaches the root, so a root that handles it
/// and keeps running (a REPL, an editor, a pager) must not trigger the pid-only hint. Nothing but
/// the terminal's echo of ^C may appear on the terminal (debug seam: the hint is due at 300 ms).
#[test]
fn no_hint_for_a_ctrl_c_the_root_survives() {
    let job = Job::new();
    let mut cmd = vec!["/usr/bin/env".to_string(), "SHEEPR_TEST_HINT_MS=300".into()];
    cmd.extend(run_args(&[], &job.args()));
    let mut pty = Pty::shell(&[], &cmd);
    pty.started();
    let ((esc, _), (root, _)) = job.ready();
    pty.write(b"\x03");
    let (r, e) = job.settle(root, esc, |r, e| r.0 >= 1 && e.0 >= 1);
    std::thread::sleep(Duration::from_millis(700));
    pty.drain();
    let shown = String::from_utf8_lossy(&pty.out).replace("^C", "");
    assert_eq!((r.0, e.0), (1, 1), "control: the ctrl-C reached (root, escapee)");
    assert!(shown.trim().is_empty(), "sheepr printed on the terminal after a ctrl-C the root survived: {shown:?}");
}

/// S4 review (B-P2-1): death by signal needs the SAME signal. sheepr consumed an INT, but the
/// root died of KILL (from outside): sheepr exits 128+9, it does not die of a signal.
#[test]
fn death_by_signal_needs_the_same_signal() {
    let job = Job::new();
    let (mut c, id) = direct(&["--quiet"], &job.args(), &[]);
    let ((esc, _), (root, root_id)) = job.ready();
    assert!(send(c.id() as i32, id, libc::SIGINT));
    job.settle(root, esc, |_, e| e.0 >= 1);
    assert!(send(root, root_id, libc::SIGKILL));
    let st = wait_bounded(&mut c, Duration::from_secs(15));
    if st.is_none() {
        send(c.id() as i32, id, libc::SIGKILL);
        let _ = c.wait();
    }
    use std::os::unix::process::ExitStatusExt;
    assert_eq!(st.and_then(|s| s.signal()), None, "sheepr died of a signal the root did not die of: {st:?}");
    assert_eq!(st.and_then(|s| s.code()), Some(128 + libc::SIGKILL), "exit code: {st:?}");
}

/// S4 review (B-P2-3): `--forward-int-to-root` is for INT only. A pid-only HUP still reaches only
/// the escapee, and the hint is printed.
#[test]
fn forward_int_to_root_does_not_forward_hup_to_the_root() {
    let job = Job::new();
    let (c, id) = direct(&["--forward-int-to-root"], &job.args(), &[("SHEEPR_TEST_HINT_MS", "300"), ("SHEEPR_TEST_NO_REPORT", "1")]);
    let ((esc, _), (root, _)) = job.ready();
    assert!(send(c.id() as i32, id, libc::SIGHUP));
    let (r, e) = job.settle(root, esc, |_, e| e.1 >= 1);
    std::thread::sleep(Duration::from_millis(400));
    let err = end(c, id, &job);
    assert_eq!((r.1, e.1), (0, 1), "a pid-only HUP with --forward-int-to-root: (root, escapee)");
    assert_eq!(err.lines().filter(|l| !l.trim().is_empty()).count(), 1, "the hint must be printed once: {err:?}");
}

/// S4 review round 2 (A-P2-2): the hint names a process group only when it is sheepr's own
/// (sheepr or its relay leads it). Here a shell leads the group and runs sheepr as a child:
/// that group is the caller's, and signalling it would reach the caller too. The hint is
/// printed once and names no group.
#[test]
fn the_hint_does_not_name_the_callers_group() {
    let job = Job::new();
    let mut c = Command::new("/bin/sh");
    let script = "\"$0\" run -- \"$@\"; exit 0";
    c.args(["-c", script, sheepr()]).args(job.args()).env("SHEEPR_TEST_HINT_MS", "300").env("SHEEPR_TEST_NO_REPORT", "1").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    unsafe {
        c.pre_exec(|| {
            for sig in [libc::SIGINT, libc::SIGHUP, libc::SIGTERM] {
                libc::signal(sig, libc::SIG_DFL);
            }
            Ok(())
        });
    }
    c.process_group(0);
    let mut sh = c.spawn().unwrap();
    let pg = sh.id() as i32;
    let (_, (root, root_id)) = job.ready();
    let sd = wait_for("sheepr under the shell", Duration::from_secs(15), || {
        let out = Command::new("ps").args(["-Ao", "pid=,ppid=,args="]).output().ok()?;
        String::from_utf8_lossy(&out.stdout).lines().find_map(|l| {
            let w: Vec<&str> = l.split_whitespace().collect();
            (w.len() >= 3 && w[1] == pg.to_string() && w[2].ends_with("sheepr")).then(|| w[0].parse::<i32>().ok()).flatten()
        })
    });
    let sr_id = identity(sd).expect("sheepr's identity");
    assert!(send(sd, sr_id, libc::SIGINT));
    std::thread::sleep(Duration::from_millis(900));
    // end the job by the root's exit, not by a signal to sheepr: the shell would report a
    // child that died of a signal on the same stderr
    send(root, root_id, libc::SIGKILL);
    let st = wait_bounded(&mut sh, Duration::from_secs(15));
    job.kill();
    if st.is_none() {
        let _ = sh.kill();
        let _ = sh.wait();
    }
    let mut err = String::new();
    if let Some(mut e) = sh.stderr.take() {
        use std::io::Read;
        let _ = e.read_to_string(&mut err);
    }
    assert_ne!(sd, pg, "control: sheepr must not lead its group here");
    assert_eq!(err.lines().filter(|l| !l.trim().is_empty()).count(), 1, "the hint must be printed once: {err:?}");
    assert!(targets(&err).is_empty(), "the hint named a group (the caller's is {pg}): {err:?}");
}

/// S4 review round 2 (A-P2-1): D9's own case, a harness in a terminal (a shell that handles INT
/// and leads the terminal's foreground group) runs sheepr as a child and sends INT to its pid
/// only. sheepr shares the foreground group, but it is not its own group, so the hint is
/// printed (on the terminal), and it names no group (the harness's).
#[test]
fn the_hint_reaches_a_harness_in_the_foreground() {
    let job = Job::new();
    let script = "trap : INT; \"$0\" run -- \"$@\"; exit 0";
    let mut args = vec!["SHEEPR_TEST_HINT_MS=1500".to_string(), "/bin/sh".into(), "-c".into(), script.into(), sheepr().into()];
    args.extend(job.args());
    let mut pty = Pty::leader("/usr/bin/env", &args);
    let sh = pty.child.id();
    job.ready();
    let (sd, sr_id) = wait_for("sheepr under the harness", Duration::from_secs(15), || supervisor_of(sh));
    // the hint is due at 1.5 s: wait for the first output (bounded) and note when it came, so
    // the output is the hint (it came when the hint was due) and not some earlier error. The
    // clock starts before the INT, so a delayed test thread cannot make the hint look early.
    let sent = Instant::now();
    assert!(send(sd, sr_id, libc::SIGINT));
    let mut first: Option<Duration> = None;
    while sent.elapsed() < Duration::from_secs(10) {
        pty.drain();
        if !String::from_utf8_lossy(&pty.out).trim().is_empty() {
            first = Some(sent.elapsed());
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    // then the whole line (it can reach the master in pieces): its group check needs the end
    let line_done = |out: &[u8]| out.iter().position(|b| !b.is_ascii_whitespace()).is_some_and(|i| out[i..].contains(&b'\n'));
    while first.is_some() && !line_done(&pty.out) && sent.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(20));
        pty.drain();
    }
    let shown = String::from_utf8_lossy(&pty.out).to_string();
    send(sd, sr_id, libc::SIGTERM);
    assert!(first.is_some(), "no hint for a pid-only INT from a harness in the foreground");
    // the clock started before the INT and sheepr sets the due time after it, so a correct
    // hint comes at or after the full 1.5 s
    assert!(first.is_some_and(|t| t >= Duration::from_millis(1500)), "output came {first:?} after the INT, before the hint was due: {shown:?}");
    assert!(targets(&shown).is_empty(), "the hint named a group (the harness's is {sh}): {shown:?}");
}

/// S4 review round 2 (B-P2-1): the other side of the foreground rule. A background job in a
/// terminal (its own group, not the foreground one) that gets a pid-only INT prints the hint on
/// the terminal, naming its group.
#[test]
fn the_hint_is_printed_for_a_background_job() {
    let job = Job::new();
    let mut cmd = vec!["/usr/bin/env".to_string(), "SHEEPR_TEST_HINT_MS=300".into()];
    cmd.extend(run_args(&[], &job.args()));
    let mut pty = Pty::shell(&["bg"], &cmd);
    let (sd, sr_id) = pty.started();
    job.ready();
    assert!(send(sd, sr_id, libc::SIGINT));
    std::thread::sleep(Duration::from_millis(900));
    pty.drain();
    let shown = String::from_utf8_lossy(&pty.out).to_string();
    assert_eq!(targets(&shown), vec![sd as i64], "a background job must print the hint naming its group {sd}: {shown:?}");
}

/// Start `bg-then-exec` (or `bg-apart-then-exec`) with sheepr running `job`, in a new process
/// group led by the relay, stderr piped, the hint due after 300 ms. Returns the relay.
fn relay_job(mode: &str, bg: &str, job: &Job) -> (Child, u64) {
    let mut c = Command::new(fixture());
    c.args([mode, bg]).args(run_args(&[], &job.args())).env("SHEEPR_TEST_HINT_MS", "300").env("SHEEPR_TEST_NO_REPORT", "1").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    unsafe {
        c.pre_exec(|| {
            for sig in [libc::SIGINT, libc::SIGHUP, libc::SIGTERM] {
                libc::signal(sig, libc::SIG_DFL);
            }
            Ok(())
        });
    }
    c.process_group(0);
    let relay = c.spawn().unwrap();
    let rid = identity(relay.id() as i32).expect("the relay's identity");
    (relay, rid)
}

/// A pid-only INT to the relay's supervisor, then the relay's stderr (the job ended by TERM).
fn relay_hint(mut relay: Child, rid: u64, bg: &str, job: &Job) -> (String, i32) {
    job.ready();
    let (sup, sup_id) = wait_for("the supervisor", Duration::from_secs(15), || supervisor_of(relay.id()));
    assert!(send(sup, sup_id, libc::SIGINT));
    std::thread::sleep(Duration::from_millis(900));
    send(relay.id() as i32, rid, libc::SIGTERM);
    if wait_bounded(&mut relay, Duration::from_secs(15)).is_none() {
        send(relay.id() as i32, rid, libc::SIGKILL);
        let _ = relay.wait();
    }
    job.kill();
    for p in sleeps(bg) {
        if let Some(id) = identity(p) {
            send(p, id, libc::SIGKILL);
        }
    }
    let mut err = String::new();
    if let Some(mut e) = relay.stderr.take() {
        use std::io::Read;
        let _ = e.read_to_string(&mut err);
    }
    (err, sup)
}

/// The pgid of `pid`, or -1.
fn pgid(pid: i32) -> i32 {
    unsafe { libc::getpgid(pid) }
}

/// S4 review round 2: on the relay path the relay leads the group. When nothing but the relay
/// and the job is in it (the caller's background job moved to a group of its own), it is
/// sheepr's own group, and a pid-only INT to the supervisor names it (not the supervisor's pid).
#[test]
fn the_hint_names_the_relays_group() {
    let job = Job::new();
    let bg = new_marker();
    let (relay, rid) = relay_job("bg-apart-then-exec", &bg, &job);
    let pg = relay.id() as i32;
    wait_for("the background job", Duration::from_secs(15), || (sleeps(&bg).len() == 1).then_some(()));
    let apart = sleeps(&bg).iter().all(|&p| pgid(p) != pg);
    let (err, sup) = relay_hint(relay, rid, &bg, &job);
    assert!(apart, "control: the caller's background job is still in the relay's group");
    assert_ne!(sup, pg, "control: the supervisor does not lead its group here");
    assert_eq!(targets(&err), vec![pg as i64], "the hint must name the relay's group {pg} (not the supervisor {sup}): {err:?}");
}

/// S4 review round 3 (P2-1): a group that also holds a process of the caller is not named, even
/// when the relay leads it. Here the caller's background job (started before sheepr, with no
/// job control) shares the relay's group: `kill -INT -<pgid>` would reach it.
#[test]
fn the_hint_names_no_group_that_holds_the_callers_job() {
    let job = Job::new();
    let bg = new_marker();
    let (relay, rid) = relay_job("bg-then-exec", &bg, &job);
    let pg = relay.id() as i32;
    wait_for("the background job", Duration::from_secs(15), || (sleeps(&bg).len() == 1).then_some(()));
    let shared = sleeps(&bg).iter().all(|&p| pgid(p) == pg);
    let (err, _) = relay_hint(relay, rid, &bg, &job);
    assert!(shared, "control: the caller's background job is not in the relay's group");
    assert_eq!(err.lines().filter(|l| !l.trim().is_empty()).count(), 1, "the hint must be printed once: {err:?}");
    assert!(targets(&err).is_empty(), "the hint named a group that holds the caller's job: {err:?}");
}

/// S4 review round 3 (P2-1): the same without a relay. The caller left an orphan in its group
/// (`(job &)`) and then exec'd sheepr, which therefore leads a group that holds the orphan.
#[test]
fn the_hint_names_no_group_that_holds_the_callers_orphan() {
    let job = Job::new();
    let bg = new_marker();
    let script = format!("(/bin/sleep {bg} >/dev/null 2>&1 &); exec \"$0\" run -- \"$@\"");
    let mut c = Command::new("/bin/sh");
    c.args(["-c", &script, sheepr()]).args(job.args()).env("SHEEPR_TEST_HINT_MS", "300").env("SHEEPR_TEST_NO_REPORT", "1").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    unsafe {
        c.pre_exec(|| {
            for sig in [libc::SIGINT, libc::SIGHUP, libc::SIGTERM] {
                libc::signal(sig, libc::SIG_DFL);
            }
            Ok(())
        });
    }
    c.process_group(0);
    let child = c.spawn().unwrap();
    let id = identity(child.id() as i32).expect("sheepr's identity");
    let pg = child.id() as i32;
    job.ready();
    wait_for("the orphan", Duration::from_secs(15), || (sleeps(&bg).len() == 1).then_some(()));
    let shared = sleeps(&bg).iter().all(|&p| pgid(p) == pg);
    assert!(send(pg, id, libc::SIGINT));
    std::thread::sleep(Duration::from_millis(900));
    let err = end(child, id, &job);
    for p in sleeps(&bg) {
        if let Some(i) = identity(p) {
            send(p, i, libc::SIGKILL);
        }
    }
    assert!(shared, "control: the orphan is not in sheepr's group");
    assert_eq!(err.lines().filter(|l| !l.trim().is_empty()).count(), 1, "the hint must be printed once: {err:?}");
    assert!(targets(&err).is_empty(), "the hint named a group that holds the caller's orphan: {err:?}");
}

// ---- death by INT (PHASE1.md §1.3) ---------------------------------------------------------

/// ctrl-C ends a foreground job whose root dies of it (both arrive in one wake): sheepr
/// dies of INT after the kill, as the root did, so the shell sees death by INT.
#[test]
fn ctrl_c_makes_sheepr_die_of_int() {
    let m = new_marker();
    let mut pty = Pty::shell(&[], &run_args(&[], &["/bin/sleep".to_string(), m.clone()]));
    pty.started();
    wait_for("the root", Duration::from_secs(15), || (sleeps(&m).len() == 1).then_some(()));
    pty.write(b"\x03");
    let end = pty.outcome(Duration::from_secs(10));
    assert_eq!(end, Some(format!("signaled {}", libc::SIGINT)), "sheepr must die of INT");
}

/// Control: a root that handles ctrl-C and exits 130 makes sheepr exit 130, not die of INT
/// (death by INT only when the root died of the same signal).
#[test]
fn a_root_that_exits_130_on_ctrl_c_makes_sheepr_exit_130() {
    let m = new_marker();
    let ready = std::env::temp_dir().join(format!("sr-s4-ready-{m}"));
    let _ = std::fs::remove_file(&ready);
    let mut pty = Pty::shell(&[], &run_args(&[], &[fixture().to_string(), "int-exit".into(), "130".into(), m.clone(), ready.display().to_string()]));
    pty.started();
    // the root's INT handler is installed (an INT before it would kill the root by default)
    wait_for("the root's handler", Duration::from_secs(15), || ready.exists().then_some(()));
    let _ = std::fs::remove_file(&ready);
    pty.write(b"\x03");
    let end = pty.outcome(Duration::from_secs(10));
    assert_eq!(end, Some("exited 130".to_string()), "sheepr must exit as the root did");
}

/// A shell loop `while sheepr run …; do :; done` stops on one ctrl-C: bash stops a loop only
/// when its child died of INT (wait-and-cooperative-exit).
#[test]
fn a_bash_loop_stops_on_one_ctrl_c() {
    let bash = ["/bin/bash", "/usr/bin/bash"].into_iter().find(|p| std::path::Path::new(p).exists()).expect("this cell needs bash");
    let m = new_marker();
    let script = "while :; do \"$0\" run -- /bin/sleep \"$1\"; /bin/sleep 0.1; done";
    let mut pty = Pty::shell(&[], &[bash.to_string(), "-c".into(), script.into(), sheepr().into(), m.clone()]);
    pty.started();
    wait_for("the root", Duration::from_secs(15), || (sleeps(&m).len() == 1).then_some(()));
    pty.write(b"\x03");
    let end = pty.outcome(Duration::from_secs(5));
    drop(pty);
    std::thread::sleep(Duration::from_millis(300));
    for p in marked(&m) {
        if let Some(id) = identity(p) {
            send(p, id, libc::SIGTERM);
        }
    }
    assert_eq!(end, Some(format!("signaled {}", libc::SIGINT)), "the loop did not stop on one ctrl-C");
}

// ---- job control (PHASE1.md §1.4, S5) -------------------------------------------------------

/// One `sr-fixture ticker` job: a root and a breeding escapee tree that ticks into `<R>.tick`.
struct Ticker {
    marker: String,
    rec: PathBuf,
}

impl Ticker {
    fn new() -> Self {
        let marker = new_marker();
        let rec = std::env::temp_dir().join(format!("sr-s5-{marker}"));
        for ext in ["", ".tick", ".root"] {
            let _ = std::fs::remove_file(format!("{}{ext}", rec.display()));
        }
        eprintln!("ticker {marker}: {}", std::thread::current().name().unwrap_or("?"));
        Ticker { marker, rec }
    }
    fn args(&self, opts: &[&str]) -> Vec<String> {
        let mut v = vec![fixture().to_string(), "ticker".into(), self.marker.clone(), self.rec.display().to_string()];
        v.extend(opts.iter().map(|s| s.to_string()));
        v
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
    /// The root, once its handler is set and it has named itself.
    fn ready(&self) -> (i32, u64) {
        let f = format!("{}.root", self.rec.display());
        wait_for("the ticker root", Duration::from_secs(15), || {
            let s = std::fs::read_to_string(&f).ok()?;
            let mut w = s.split_whitespace();
            Some((w.next()?.parse().ok()?, w.next()?.parse().ok()?))
        })
    }
    fn ticks(&self) -> usize {
        std::fs::read_to_string(format!("{}.tick", self.rec.display())).map(|s| s.lines().count()).unwrap_or(0)
    }
    /// Ticks in a 400 ms window.
    fn advance(&self) -> usize {
        let t0 = self.ticks();
        std::thread::sleep(Duration::from_millis(400));
        self.ticks() - t0
    }
    /// Wait until the escapee tree ticks (bounded).
    fn ticking(&self) {
        wait_for("the escapee's ticks", Duration::from_secs(15), || (self.ticks() >= 2).then_some(()));
    }
}

impl Drop for Ticker {
    fn drop(&mut self) {
        for (p, id) in self.recorded() {
            if p > 1 {
                send(p, id, libc::SIGKILL);
            }
        }
        // any ticker process not recorded (a cell that failed early), by its argv and identity
        if let Ok(out) = Command::new("ps").args(["-Ao", "pid=,args="]).output() {
            for l in String::from_utf8_lossy(&out.stdout).lines() {
                let w: Vec<&str> = l.split_whitespace().collect();
                if w.len() >= 4 && w[1].ends_with("sr-fixture") && w[2] == "ticker" && w[3] == self.marker {
                    if let Some((p, id)) = w[0].parse::<i32>().ok().and_then(|p| Some((p, identity(p)?))) {
                        if p > 1 {
                            send(p, id, libc::SIGKILL);
                        }
                    }
                }
            }
        }
        for ext in ["", ".tick", ".root"] {
            let _ = std::fs::remove_file(format!("{}{ext}", self.rec.display()));
        }
    }
}

/// The process state letter (`T` stopped), or None if gone.
fn state(pid: i32) -> Option<char> {
    let out = Command::new("ps").args(["-o", "stat=", "-p", &pid.to_string()]).output().ok()?;
    String::from_utf8_lossy(&out.stdout).trim().chars().next()
}

/// The child of `parent` whose argv ends a word with `prog` (e.g. "sleep"), with its identity.
fn child_of(parent: i32, prog: &str) -> Option<(i32, u64)> {
    let out = Command::new("ps").args(["-Ao", "pid=,ppid=,args="]).output().ok()?;
    String::from_utf8_lossy(&out.stdout).lines().find_map(|l| {
        let w: Vec<&str> = l.split_whitespace().collect();
        let p: i32 = w.first()?.parse().ok()?;
        (w.len() >= 3 && w[1] == parent.to_string() && w[2].ends_with(prog)).then(|| common::found(p)).flatten()
    })
}

/// `fg`: let the fixture shell continue the job, and wait until it has.
fn fg(pty: &mut Pty, n: usize) {
    pty.fg();
    wait_for("the shell's fg", Duration::from_secs(10), || (pty.lines().iter().filter(|l| *l == "continued").count() >= n).then_some(()));
}

fn stopped_lines(pty: &Pty) -> usize {
    pty.lines().iter().filter(|l| l.starts_with("stopped ")).count()
}

/// Cell 21(a): ctrl-Z in a pty stops the whole job, a breeding escapee tree included (it ticks
/// and forks new tickers). While stopped it does not advance; after `fg` it ticks again.
#[test]
fn cell21a_ctrl_z_stops_a_breeding_escapee() {
    let t = Ticker::new();
    let mut pty = Pty::shell(&[], &run_args(&["--quiet"], &t.args(&[])));
    pty.started();
    t.ready();
    t.ticking();
    pty.write(b"\x1a");
    let stopped = pty.wait_line("stopped", Duration::from_secs(10), |l| l.starts_with("stopped "));
    std::thread::sleep(Duration::from_millis(200));
    let while_stopped = t.advance();
    fg(&mut pty, 1);
    let after = t.advance();
    assert_eq!(stopped, Some(format!("stopped {}", libc::SIGTSTP)), "the shell did not see the job stop");
    assert_eq!(while_stopped, 0, "the escapee tree ticked while the job was stopped");
    assert!(after > 0, "control: the escapee tree did not tick again after fg");
}

/// Cell 21(b): an escapee the user had stopped before the ctrl-Z stays stopped after `fg`
/// (sheepr continues only the members it stopped itself).
#[test]
fn cell21b_an_escapee_stopped_by_the_user_stays_stopped() {
    let t = Ticker::new();
    let mut pty = Pty::shell(&[], &run_args(&["--quiet"], &t.args(&[])));
    pty.started();
    t.ready();
    t.ticking();
    // one ticking process of the escapee tree, stopped by "the user": a recorded pair (its
    // identity read when it was made), that also ticks
    let tick = std::fs::read_to_string(format!("{}.tick", t.rec.display())).unwrap_or_default();
    let ticking: Vec<i32> = tick.lines().filter_map(|l| l.trim().parse().ok()).collect();
    let (p, id) = t.recorded().into_iter().find(|(p, _)| ticking.contains(p)).expect("a recorded, ticking escapee");
    assert!(send(p, id, libc::SIGSTOP));
    wait_for("the user's stop", Duration::from_secs(5), || (state(p) == Some('T')).then_some(()));
    pty.write(b"\x1a");
    pty.wait_line("stopped", Duration::from_secs(10), |l| l.starts_with("stopped "));
    fg(&mut pty, 1);
    // the resume is finished once the others tick again (bounded); only then read the state
    let t0 = t.ticks();
    let resumed = wait_for_opt(Duration::from_secs(10), || t.ticks() > t0 + 2);
    let st = state(p);
    assert!(resumed, "control: the rest of the job did not resume after fg");
    assert_eq!(st, Some('T'), "an escapee the user had stopped was continued by fg");
}

/// Cell 21(c): a same-group root with a 300 ms TSTP handler (a pager restoring the terminal)
/// finishes it before the job counts as stopped: its marker exists when the shell sees the stop.
#[test]
fn cell21c_a_slow_tstp_handler_finishes_before_the_job_stops() {
    let t = Ticker::new();
    let marker = std::env::temp_dir().join(format!("sr-s5-slow-{}", t.marker));
    let _ = std::fs::remove_file(&marker);
    let mut pty = Pty::shell(&[], &run_args(&["--quiet"], &t.args(&["slow-tstp", marker.to_str().unwrap()])));
    pty.started();
    t.ready();
    pty.write(b"\x1a");
    let stopped = pty.wait_line("stopped", Duration::from_secs(10), |l| l.starts_with("stopped "));
    let done = marker.exists();
    fg(&mut pty, 1);
    let _ = std::fs::remove_file(&marker);
    assert!(stopped.is_some(), "the job did not stop");
    assert!(done, "the job counted as stopped before the root's TSTP handler finished");
}

/// Cell 21(d): the root's TSTP handler sends TSTP to its whole group again. That second stop
/// belongs to the same ctrl-Z: after `fg` the job keeps running (it does not stop again).
#[test]
fn cell21d_a_regrouped_tstp_is_one_stop() {
    let t = Ticker::new();
    let mut pty = Pty::shell(&[], &run_args(&["--quiet"], &t.args(&["regroup-tstp"])));
    pty.started();
    t.ready();
    t.ticking();
    pty.write(b"\x1a");
    pty.wait_line("stopped", Duration::from_secs(10), |l| l.starts_with("stopped "));
    fg(&mut pty, 1);
    std::thread::sleep(Duration::from_millis(1000));
    let stops = stopped_lines(&pty);
    let after = t.advance();
    assert_eq!(stops, 1, "the job stopped again after fg: {:?}", pty.lines());
    assert!(after > 0, "the job does not run after fg");
}

/// Cell 21(e), rewritten for phase 1 (no --timeout): an orphaned process group (sheepr leads
/// its own session, no terminal) gets a TSTP. The root does not stop (the kernel discards its
/// TSTP), so after its wait sheepr SIGSTOPs it; the kernel discards sheepr's own stop, no
/// CONT ever comes, and sheepr still continues the root: the job ends when the 3 s root
/// exits, no hang. Sheepr's signal log is the evidence that the stop path ran (the root was
/// stopped and continued).
#[test]
fn cell21e_a_tstp_in_an_orphaned_group_does_not_hang() {
    let log = std::env::temp_dir().join(format!("sr-s5-21e-{}", new_marker()));
    let _ = std::fs::remove_file(&log);
    let mut c = Command::new(sheepr());
    c.args(["run", "--quiet", "--", "/bin/sleep", "3"]).env("SHEEPR_TEST_SIGNAL_LOG", &log).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    unsafe {
        c.pre_exec(|| {
            for sig in [libc::SIGINT, libc::SIGHUP, libc::SIGTERM, libc::SIGTSTP, libc::SIGCONT] {
                libc::signal(sig, libc::SIG_DFL);
            }
            libc::setsid();
            Ok(())
        });
    }
    let mut c = c.spawn().unwrap();
    let id = identity(c.id() as i32).expect("sheepr's identity");
    // ready: the root runs (so sheepr watches TSTP and has a member to stop)
    let (root, root_id) = wait_for("the root", Duration::from_secs(15), || child_of(c.id() as i32, "sleep"));
    assert!(send_group(c.id() as i32, id, libc::SIGTSTP));
    let st = wait_bounded(&mut c, Duration::from_secs(10));
    if st.is_none() {
        common::send_child(&mut c, libc::SIGKILL);
        let _ = c.wait();
        send(root, root_id, libc::SIGKILL);
    }
    let lines = std::fs::read_to_string(&log).unwrap_or_default();
    let _ = std::fs::remove_file(&log);
    let sent = |sig: i32| lines.lines().any(|l| l.split_whitespace().collect::<Vec<_>>() == ["kill", &root.to_string(), &sig.to_string()] || l.split_whitespace().collect::<Vec<_>>() == ["pidfd", &root.to_string(), &sig.to_string()]);
    assert!(sent(libc::SIGSTOP) && sent(libc::SIGCONT), "control: the stop path did not run (the root was not stopped and continued): {lines:?}");
    assert_eq!(st.and_then(|s| s.code()), Some(0), "the orphaned job hung or failed after a TSTP: {st:?}");
}

/// Cell 21'(f): `sh -c 'sheepr run …; true'` in a pty (sheepr is not the group leader), ctrl-Z:
/// the escapee tree stops and the prompt returns (the shell sees the job stop).
#[test]
fn cell21f_ctrl_z_under_a_shell_wrapper_stops_the_escapees() {
    let t = Ticker::new();
    let mut cmd = vec!["/bin/sh".to_string(), "-c".into(), "\"$0\" run --quiet -- \"$@\"; true".into(), sheepr().into()];
    cmd.extend(t.args(&[]));
    let mut pty = Pty::shell(&[], &cmd);
    pty.started();
    t.ready();
    t.ticking();
    pty.write(b"\x1a");
    let stopped = pty.wait_line("stopped", Duration::from_secs(10), |l| l.starts_with("stopped "));
    std::thread::sleep(Duration::from_millis(1300));
    let while_stopped = t.advance();
    fg(&mut pty, 1);
    let after = t.advance();
    assert!(stopped.is_some(), "the prompt did not return (the shell never saw the job stop)");
    assert_eq!(while_stopped, 0, "the escapee tree ticked while the job was stopped");
    assert!(after > 0, "control: the escapee tree did not tick again after fg");
}

/// The shape of cell 21'(g): a session leader (`nosession`) starts `sh` in a group of its own,
/// and `sh` runs sheepr. While `sh` lives the group is not orphaned (sh's parent is in the same
/// session, in another group); once `sh` is gone, sheepr is re-parented outside the session
/// and the group is orphaned. The root ignores TSTP, so sheepr waits for it (widened to 3 s by
/// the debug seam). Returns (nosession, sh, sheepr, root marker); the group has had its TSTP
/// and `sh` is seen stopped.
fn orphan_shape() -> (Child, (i32, u64), (i32, u64), String) {
    let m5 = format!("5.{}{:06}", std::process::id(), SEQ.fetch_add(1, Ordering::SeqCst));
    let pidfile = std::env::temp_dir().join(format!("sr-s5-21g-{m5}"));
    let _ = std::fs::remove_file(&pidfile);
    let script = format!("\"$0\" run --quiet -- /bin/sh -c \"trap '' TSTP; /bin/sleep {m5}\"; true");
    let mut c = Command::new(fixture());
    c.args(["nosession", pidfile.to_str().unwrap(), "/bin/sh", "-c", &script, sheepr()]).env("SHEEPR_TEST_STOP_WAIT_MS", "3000").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    unsafe {
        c.pre_exec(|| {
            for sig in [libc::SIGINT, libc::SIGHUP, libc::SIGTERM, libc::SIGTSTP, libc::SIGCONT] {
                libc::signal(sig, libc::SIG_DFL);
            }
            Ok(())
        });
    }
    let leader = c.spawn().unwrap();
    let sh: (i32, u64) = wait_for("the shell's pid file", Duration::from_secs(15), || {
        let s = std::fs::read_to_string(&pidfile).ok()?;
        let mut w = s.split_whitespace();
        Some((w.next()?.parse().ok()?, w.next()?.parse().ok()?))
    });
    let _ = std::fs::remove_file(&pidfile);
    let sd = wait_for("sheepr under the shell", Duration::from_secs(15), || supervisor_of(sh.0 as u32));
    wait_for("the root", Duration::from_secs(15), || (sleeps(&m5).len() == 1).then_some(()));
    assert!(send_group(sh.0, sh.1, libc::SIGTSTP));
    // the group got it and is not orphaned: sh (default TSTP) stops
    wait_for("the shell's stop", Duration::from_secs(10), || (state(sh.0) == Some('T')).then_some(()));
    (leader, sh, sd, m5)
}

fn orphan_cleanup(mut leader: Child, sh: (i32, u64), sd: (i32, u64), m5: &str) {
    if same(sd.0, sd.1) {
        send(sd.0, sd.1, libc::SIGCONT);
        send(sd.0, sd.1, libc::SIGTERM);
        std::thread::sleep(Duration::from_millis(300));
        send(sd.0, sd.1, libc::SIGKILL);
    }
    send(sh.0, sh.1, libc::SIGKILL);
    common::kill_marked(&[m5]);
    if wait_bounded(&mut leader, Duration::from_secs(5)).is_none() {
        common::send_child(&mut leader, libc::SIGKILL);
        let _ = leader.wait();
    }
}

/// Control for 21'(g): the same shape, the parent is NOT killed: the group is not orphaned, so
/// the stop is kept and sheepr is seen stopped (proving 21'(g) starts from a group that is
/// not orphaned).
#[test]
fn cell21g_control_a_live_parent_keeps_the_stop() {
    let (leader, sh, sd, m5) = orphan_shape();
    let seen = wait_for_opt(Duration::from_secs(8), || state(sd.0) == Some('T'));
    orphan_cleanup(leader, sh, sd, &m5);
    assert!(seen, "sheepr was never seen stopped: the group was orphaned before the parent was killed");
}

/// Cell 21'(g), rewritten: the parent is killed while sheepr waits for the members to stop.
/// The group becomes orphaned during the stop, sheepr's own stop is discarded, and the job
/// ends when its ~5 s root exits: no hang. (Re-parented to a process in the SAME session, as to
/// a container's init, the group would not be orphaned: stated in PLAN.)
#[test]
fn cell21g_a_parent_killed_during_the_stop_does_not_hang() {
    let (leader, sh, sd, m5) = orphan_shape();
    assert!(send(sh.0, sh.1, libc::SIGKILL), "the parent shell was not killed");
    let ended = wait_for_opt(Duration::from_secs(15), || !same(sd.0, sd.1));
    orphan_cleanup(leader, sh, sd, &m5);
    assert!(ended, "the job hung after its parent was killed during the stop");
}

/// Cell 21'(h): ctrl-Z again right after `fg` stops the job again, and after the second `fg`
/// the escapees that sheepr stopped run again. (The second ctrl-Z comes after sheepr took
/// the CONT; a stop that returns with no CONT at all is 21(e)'s case.)
#[test]
fn cell21h_ctrl_z_right_after_fg_stops_again_and_fg_resumes() {
    let t = Ticker::new();
    let mut pty = Pty::shell(&[], &run_args(&["--quiet"], &t.args(&[])));
    pty.started();
    t.ready();
    t.ticking();
    pty.write(b"\x1a");
    pty.wait_line("stopped", Duration::from_secs(10), |l| l.starts_with("stopped "));
    pty.fg();
    wait_for("the shell's fg", Duration::from_secs(10), || pty.lines().iter().any(|l| l == "continued").then_some(()));
    pty.write(b"\x1a");
    let second = wait_for("the second stop", Duration::from_secs(10), || (stopped_lines(&pty) >= 2).then_some(()));
    let _ = second;
    std::thread::sleep(Duration::from_millis(1300));
    let while_stopped = t.advance();
    fg(&mut pty, 2);
    let after = t.advance();
    assert_eq!(while_stopped, 0, "the escapee tree ticked during the second stop");
    assert!(after > 0, "the escapees sheepr stopped were not continued after the second fg");
}

/// S5 (PLAN.md §3.1 TTIN): `sheepr run -- <reader> &` in a pty: the root reads the terminal
/// in the background, the job stops with "Stopped (tty input)", and the escapees stop too.
#[test]
fn ttin_stops_the_job_and_its_escapees() {
    let t = Ticker::new();
    let mut pty = Pty::shell(&["bg"], &run_args(&["--quiet"], &t.args(&["read"])));
    pty.started();
    t.ready();
    let stopped = pty.wait_line("stopped", Duration::from_secs(10), |l| l.starts_with("stopped "));
    std::thread::sleep(Duration::from_millis(1300));
    let while_stopped = t.advance();
    fg(&mut pty, 1);
    let after = t.advance();
    assert!(after > 0, "control: the escapee tree does not tick after fg");
    assert_eq!(stopped, Some(format!("stopped {}", libc::SIGTTIN)), "the job did not stop for tty input");
    assert_eq!(while_stopped, 0, "the escapee tree ticked while the job was stopped");
}

/// S5 (TTOU): a background root that writes to a `tostop` terminal: "Stopped (tty output)", and
/// the escapees stop too.
#[test]
fn ttou_stops_the_job_and_its_escapees() {
    let t = Ticker::new();
    let mut pty = Pty::shell(&["bg", "tostop"], &run_args(&["--quiet"], &t.args(&["write"])));
    pty.started();
    t.ready();
    let stopped = pty.wait_line("stopped", Duration::from_secs(10), |l| l.starts_with("stopped "));
    std::thread::sleep(Duration::from_millis(1300));
    let while_stopped = t.advance();
    fg(&mut pty, 1);
    let after = t.advance();
    assert!(after > 0, "control: the escapee tree does not tick after fg");
    assert_eq!(stopped, Some(format!("stopped {}", libc::SIGTTOU)), "the job did not stop for tty output");
    assert_eq!(while_stopped, 0, "the escapee tree ticked while the job was stopped");
}

/// The regroup-tstp fixture sends TSTP to its whole group, so it refuses to run in a group its
/// parent does not lead (here: the test runner's own group, which one TSTP would stop).
#[test]
fn the_regroup_fixture_refuses_a_group_it_did_not_make() {
    // (1) in the test runner's group; (2) in a group its parent leads, the parent not being
    // sheepr (as when a test binary leads its own group)
    for leader in [false, true] {
        let t = Ticker::new();
        let mut c = if leader {
            let mut c = Command::new("/bin/sh");
            c.args(["-c", "\"$0\" \"$@\"; exit $?"]).args(t.args(&["regroup-tstp"]));
            c.process_group(0);
            c
        } else {
            let mut c = Command::new(fixture());
            c.args(&t.args(&["regroup-tstp"])[1..]);
            c
        };
        let mut c = c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        let st = wait_bounded(&mut c, Duration::from_secs(5));
        if st.is_none() {
            common::send_child(&mut c, libc::SIGKILL);
            let _ = c.wait();
        }
        assert_eq!(st.and_then(|s| s.code()), Some(5), "leader parent={leader}: the fixture did not refuse: {st:?}");
        assert!(t.recorded().is_empty(), "leader parent={leader}: the fixture started processes before refusing");
    }
}

/// A marker that is also a short sleep: `<secs>.<digits>`, so a leaked `/bin/sleep` ends soon.
fn short_marker(secs: u32) -> String {
    format!("{secs}.{}{:06}", std::process::id(), SEQ.fetch_add(1, Ordering::SeqCst))
}

fn end_direct(mut c: Child, id: u64, m: &str) {
    if common::alive((c.id() as i32, id)) {
        common::send_child(&mut c, libc::SIGCONT);
        common::send_child(&mut c, libc::SIGTERM);
        if wait_bounded(&mut c, Duration::from_secs(10)).is_none() {
            common::send_child(&mut c, libc::SIGKILL);
        }
    }
    let _ = c.wait();
    common::kill_marked(&[m]);
}

/// S5 review (A-P1-1): ctrl-Z on the relay path. The shell's job is the relay: it must stop
/// (the relay keeps the caller's stop signals), so the prompt returns; the escapees stop, and
/// after `fg` they run again.
#[test]
fn ctrl_z_on_the_relay_path_stops_the_job() {
    let t = Ticker::new();
    let bg = new_marker();
    let mut cmd = vec![fixture().to_string(), "bg-then-exec".into(), bg.clone()];
    cmd.extend(run_args(&["--quiet"], &t.args(&[])));
    let mut pty = Pty::shell(&[], &cmd);
    let (relay, _) = pty.started();
    t.ready();
    t.ticking();
    let sup = supervisor_of(relay as u32);
    pty.write(b"\x1a");
    let stopped = pty.wait_line("stopped", Duration::from_secs(10), |l| l.starts_with("stopped "));
    let while_stopped = if stopped.is_some() {
        std::thread::sleep(Duration::from_millis(1300));
        t.advance()
    } else {
        usize::MAX
    };
    if stopped.is_some() {
        fg(&mut pty, 1);
    }
    let after = t.advance();
    if let Some((p, id)) = sup {
        send(p, id, libc::SIGCONT);
    }
    for p in sleeps(&bg) {
        if let Some(id) = identity(p) {
            send(p, id, libc::SIGKILL);
        }
    }
    assert!(sup.is_some(), "control: not the relay path");
    assert!(stopped.is_some(), "the shell never saw the job stop on the relay path: {:?}", pty.lines());
    assert_eq!(while_stopped, 0, "the escapee tree ticked while the job was stopped");
    assert!(after > 0, "the escapee tree did not tick again after fg");
}

/// A direct sheepr whose root is a ticker that ignores TSTP (the member wait runs its full,
/// widened 3 s), with a signal log. Returns once the escapee tree ticks.
fn ignoring_ticker(t: &Ticker, log: &PathBuf) -> (Child, u64, (i32, u64)) {
    let mut cmd = vec!["/bin/sh".to_string(), "-c".into(), "trap '' TSTP; exec \"$0\" \"$@\"".into()];
    cmd.extend(t.args(&[]));
    let l = log.display().to_string();
    let (c, id) = direct(&["--quiet"], &cmd, &[("SHEEPR_TEST_STOP_WAIT_MS", "3000"), ("SHEEPR_TEST_SIGNAL_LOG", &l)]);
    let root = t.ready();
    t.ticking();
    (c, id, root)
}

/// Does sheepr's debug log hold the line `line` (its own trace of a state it reached)?
fn log_has(log: &PathBuf, line: &str) -> bool {
    std::fs::read_to_string(log).unwrap_or_default().lines().any(|l| l == line)
}

/// Signals sheepr sent, from its signal log: (pid, signal).
fn sent_signals(log: &PathBuf) -> Vec<(i32, i32)> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let w: Vec<&str> = l.split_whitespace().collect();
            (w.len() == 3 && (w[0] == "kill" || w[0] == "pidfd")).then(|| Some((w[1].parse().ok()?, w[2].parse().ok()?))).flatten()
        })
        .collect()
}

/// S5 review (A-P1-2, round 2 P3-2/P3-4): TSTP and then CONT to the group while sheepr waits
/// for its TSTP-ignoring root. The job was continued: sheepr does not stop itself, never
/// stops the root, and continues the escapees it had stopped (evidence: its signal log).
#[test]
fn a_cont_during_the_stop_wait_cancels_the_stop() {
    let t = Ticker::new();
    let log = std::env::temp_dir().join(format!("sr-s5-contlog-{}", t.marker));
    let _ = std::fs::remove_file(&log);
    let (c, id, (root, _)) = ignoring_ticker(&t, &log);
    let pg = c.id() as i32;
    assert!(send_group(pg, id, libc::SIGTSTP));
    let stopped_esc = wait_for_opt(Duration::from_secs(5), || sent_signals(&log).iter().any(|&(p, s)| p != root && s == libc::SIGSTOP));
    assert!(send_group(pg, id, libc::SIGCONT));
    // done when every escapee sheepr stopped has been continued (bounded)
    let resumed = wait_for_opt(Duration::from_secs(6), || {
        let v = sent_signals(&log);
        let stops: Vec<i32> = v.iter().filter(|&&(_, s)| s == libc::SIGSTOP).map(|&(p, _)| p).collect();
        !stops.is_empty() && stops.iter().all(|p| v.contains(&(*p, libc::SIGCONT)))
    });
    std::thread::sleep(Duration::from_millis(200));
    let st = state(pg);
    let root_stopped = sent_signals(&log).contains(&(root, libc::SIGSTOP));
    let ticks = t.advance();
    end_direct(c, id, &t.marker);
    let _ = std::fs::remove_file(&log);
    assert!(stopped_esc, "control: sheepr never stopped an escapee (the stop did not run)");
    assert!(resumed, "the escapees sheepr stopped were not all continued");
    assert_ne!(st, Some('T'), "sheepr stopped itself although the job was continued");
    assert!(!root_stopped, "sheepr stopped the root after the job was continued");
    assert!(ticks > 0, "the job does not run after the CONT");
}

/// S5 review (A-P2-2, round 2 P3-4): a TERM that arrives while sheepr waits for its
/// TSTP-ignoring root ends the job at once (TERM first in the fixed order): sheepr dies of
/// TERM, and it never stopped the root.
#[test]
fn a_term_during_the_stop_wait_ends_the_job() {
    let t = Ticker::new();
    let log = std::env::temp_dir().join(format!("sr-s5-termlog-{}", t.marker));
    let _ = std::fs::remove_file(&log);
    let (mut c, id, (root, _)) = ignoring_ticker(&t, &log);
    let pg = c.id() as i32;
    assert!(send_group(pg, id, libc::SIGTSTP));
    let stopped_esc = wait_for_opt(Duration::from_secs(5), || sent_signals(&log).iter().any(|&(p, s)| p != root && s == libc::SIGSTOP));
    assert!(common::send_child(&mut c, libc::SIGTERM));
    let st = wait_bounded(&mut c, Duration::from_secs(4));
    let root_stopped = sent_signals(&log).contains(&(root, libc::SIGSTOP));
    if st.is_none() {
        common::send_child(&mut c, libc::SIGCONT);
        common::send_child(&mut c, libc::SIGKILL);
        let _ = c.wait();
    }
    let _ = std::fs::remove_file(&log);
    use std::os::unix::process::ExitStatusExt;
    assert!(stopped_esc, "control: sheepr never stopped an escapee (the stop did not run)");
    assert_eq!(st.and_then(|s| s.signal()), Some(libc::SIGTERM), "a TERM during the stop wait did not end the job: {st:?}");
    assert!(!root_stopped, "sheepr stopped the root although a TERM had come");
}

/// S5 review (A-P2-2): the root exits while sheepr waits for it to stop: the job is over, so
/// sheepr exits as the root did instead of stopping itself.
#[test]
fn a_root_exit_during_the_stop_wait_ends_the_job() {
    // the root's sleep is its marker: running means the trap is set (it comes first)
    let m = short_marker(1);
    let script = format!("trap '' TSTP; /bin/sleep {m}");
    let (mut c, id) = direct(&["--quiet"], &["/bin/sh".into(), "-c".into(), script], &[("SHEEPR_TEST_STOP_WAIT_MS", "3000")]);
    let pg = c.id() as i32;
    wait_for("the root", Duration::from_secs(15), || (sleeps(&m).len() == 1).then_some(()));
    assert!(send_group(pg, id, libc::SIGTSTP));
    let st = wait_bounded(&mut c, Duration::from_secs(2));
    if st.is_none() {
        end_direct(c, id, &m);
    }
    assert_eq!(st.and_then(|s| s.code()), Some(0), "sheepr did not end with its root: {st:?}");
}

/// S5 review (A-P2-1): a member born during the stop wait (the root's TSTP handler forks a
/// ticker 200 ms in, and keeps running) must be stopped too: nothing ticks while the job is
/// stopped.
#[test]
fn a_member_born_during_the_stop_wait_is_stopped() {
    let t = Ticker::new();
    let mut pty = Pty::shell(&[], &run_args(&["--quiet"], &t.args(&["fork-on-tstp"])));
    pty.started();
    let (root, _) = t.ready();
    t.ticking();
    pty.write(b"\x1a");
    let stopped = pty.wait_line("stopped", Duration::from_secs(10), |l| l.starts_with("stopped "));
    std::thread::sleep(Duration::from_millis(300));
    let while_stopped = t.advance();
    // the handler's child: a recorded process whose parent is the root
    let born = t.recorded().iter().any(|&(p, _)| {
        let out = Command::new("ps").args(["-o", "ppid=", "-p", &p.to_string()]).output();
        out.map(|o| String::from_utf8_lossy(&o.stdout).trim() == root.to_string()).unwrap_or(false)
    });
    fg(&mut pty, 1);
    let after = t.advance();
    assert!(stopped.is_some(), "the job did not stop");
    assert!(born, "control: the root's TSTP handler did not fork its ticker (no recorded child of the root)");
    assert_eq!(while_stopped, 0, "a member born during the stop wait kept running while the job was stopped");
    assert!(after > 0, "control: nothing ticked after fg");
}

/// Start `bg-then-exec <bg> sheepr run -- <cmd>` in a new process group (no terminal), the
/// member wait widened to 3 s. Returns the relay (the caller's child) and its identity.
fn relay_direct(bg: &str, cmd: &[String], log: Option<&PathBuf>) -> (Child, u64) {
    relay_direct_env(bg, cmd, log, &[])
}

fn relay_direct_env(bg: &str, cmd: &[String], log: Option<&PathBuf>, env: &[(&str, &str)]) -> (Child, u64) {
    let mut c = Command::new(fixture());
    for (k, v) in env {
        c.env(k, v);
    }
    c.args(["bg-then-exec", bg]).args(run_args(&["--quiet"], cmd)).env("SHEEPR_TEST_STOP_WAIT_MS", "3000").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    if let Some(l) = log {
        c.env("SHEEPR_TEST_SIGNAL_LOG", l);
    }
    unsafe {
        c.pre_exec(|| {
            for sig in [libc::SIGINT, libc::SIGHUP, libc::SIGTERM, libc::SIGTSTP, libc::SIGCONT] {
                libc::signal(sig, libc::SIG_DFL);
            }
            Ok(())
        });
    }
    c.process_group(0);
    let relay = c.spawn().unwrap();
    let id = identity(relay.id() as i32).expect("the relay's identity");
    (relay, id)
}

fn end_relay(mut relay: Child, bg: &str, markers: &[&str]) {
    if relay.try_wait().ok().flatten().is_none() {
        common::send_child(&mut relay, libc::SIGCONT);
        common::send_child(&mut relay, libc::SIGTERM);
        if wait_bounded(&mut relay, Duration::from_secs(10)).is_none() {
            common::send_child(&mut relay, libc::SIGKILL);
        }
    }
    let _ = relay.wait();
    common::kill_marked(&[bg]);
    common::kill_marked(markers);
}

/// S5 review round 2 (P2-1a): cell 21(c) on the relay path. The shell waits on the relay, so the
/// relay must not stop before sheepr has stopped the job: the root's slow TSTP handler has
/// finished when the shell sees the stop.
#[test]
fn a_slow_tstp_handler_finishes_before_the_relayed_job_stops() {
    let t = Ticker::new();
    let bg = new_marker();
    let marker = std::env::temp_dir().join(format!("sr-s5-rslow-{}", t.marker));
    let _ = std::fs::remove_file(&marker);
    let mut cmd = vec![fixture().to_string(), "bg-then-exec".into(), bg.clone()];
    cmd.extend(run_args(&["--quiet"], &t.args(&["slow-tstp", marker.to_str().unwrap()])));
    let mut pty = Pty::shell(&[], &cmd);
    let (relay, _) = pty.started();
    t.ready();
    let relayed = supervisor_of(relay as u32).is_some();
    pty.write(b"\x1a");
    let stopped = pty.wait_line("stopped", Duration::from_secs(10), |l| l.starts_with("stopped "));
    let done = marker.exists();
    if stopped.is_some() {
        fg(&mut pty, 1);
    }
    let _ = std::fs::remove_file(&marker);
    common::kill_marked(&[&bg]);
    assert!(relayed, "control: not the relay path");
    assert!(stopped.is_some(), "the relayed job did not stop");
    assert!(done, "the relay counted the job as stopped before the root's TSTP handler finished");
}

/// S5 review round 2 (P2-1b): relay path, the root exits while sheepr waits for it to stop:
/// the caller sees the job's exit (the relay dies as the supervisor did), not a stop.
#[test]
fn a_relayed_job_that_ends_during_the_stop_wait_exits() {
    let bg = new_marker();
    let m = short_marker(1);
    let (relay, id) = relay_direct(&bg, &["/bin/sh".into(), "-c".into(), format!("trap '' TSTP; /bin/sleep {m}")], None);
    let mut relay = relay;
    wait_for("the root", Duration::from_secs(15), || (sleeps(&m).len() == 1).then_some(()));
    assert!(send_group(relay.id() as i32, id, libc::SIGTSTP));
    let st = wait_bounded(&mut relay, Duration::from_secs(4));
    let relay_state = if st.is_none() { state(relay.id() as i32) } else { None };
    end_relay(relay, &bg, &[&m]);
    assert_eq!(st.and_then(|s| s.code()), Some(0), "the caller did not see the job's exit (relay state {relay_state:?})");
}

/// S5 review round 2 (P2-1c): relay path, a TSTP to the relay's pid only (the stated pid-only
/// limit on this path): nothing stops. Never "the relay stopped while the job runs".
#[test]
fn a_tstp_to_the_relay_pid_stops_nothing() {
    let t = Ticker::new();
    let bg = new_marker();
    let (relay, id) = relay_direct(&bg, &t.args(&[]), None);
    t.ready();
    t.ticking();
    let (sup, _) = wait_for("the supervisor", Duration::from_secs(15), || supervisor_of(relay.id()));
    assert!(send(relay.id() as i32, id, libc::SIGTSTP));
    std::thread::sleep(Duration::from_millis(1300));
    let (rs, ss) = (state(relay.id() as i32), state(sup));
    let ticks = t.advance();
    end_relay(relay, &bg, &[]);
    assert_ne!(rs, Some('T'), "a pid-only TSTP stopped the relay");
    assert_ne!(ss, Some('T'), "a pid-only TSTP to the relay stopped the supervisor");
    assert!(ticks > 0, "the job stopped ticking");
}

/// S5 review round 3 (P2-1): relay path, the supervisor alone is stopped and continued (signals
/// to its pid: a debugger's shape). The relay mirrors the stop and must follow the continue, so
/// the caller gets the job's exit when the root ends. Both STOP/CONT and TSTP/CONT.
#[test]
fn a_supervisor_stopped_and_continued_alone_does_not_strand_the_relay() {
    for stop in [libc::SIGSTOP, libc::SIGTSTP] {
        let bg = new_marker();
        // the root outlives the 3 s member wait by a wide margin, whatever the runner's pid
        let m = short_marker(6);
        let (mut relay, _) = relay_direct(&bg, &["/bin/sleep".into(), m.clone()], None);
        wait_for("the root", Duration::from_secs(15), || (sleeps(&m).len() == 1).then_some(()));
        let (sup, sup_id) = wait_for("the supervisor", Duration::from_secs(15), || supervisor_of(relay.id()));
        assert!(send(sup, sup_id, stop));
        // mirrored (the relay follows a stopped supervisor within a tick)
        let mirrored = wait_for_opt(Duration::from_secs(6), || state(relay.id() as i32) == Some('T'));
        assert!(send(sup, sup_id, libc::SIGCONT));
        let st = wait_bounded(&mut relay, Duration::from_secs(14));
        let rs = if st.is_none() { state(relay.id() as i32) } else { None };
        end_relay(relay, &bg, &[&m]);
        assert!(mirrored, "stop {stop}: control: the relay did not mirror the supervisor's stop");
        assert_eq!(st.and_then(|s| s.code()), Some(0), "stop {stop}: the caller did not get the job's exit (relay state {rs:?})");
    }
}

/// S5 review round 3 (P2-2): relay path, the job is stopped (a group TSTP), then TERM and CONT to
/// the relay's pid (what coreutils `timeout` does): the job ends, the relay dies of TERM, and
/// nothing is left.
#[test]
fn term_and_cont_to_a_stopped_relay_end_the_job() {
    let t = Ticker::new();
    let bg = new_marker();
    let (mut relay, id) = relay_direct(&bg, &t.args(&[]), None);
    t.ready();
    t.ticking();
    let (sup, _) = wait_for("the supervisor", Duration::from_secs(15), || supervisor_of(relay.id()));
    assert!(send_group(relay.id() as i32, id, libc::SIGTSTP));
    let stopped = wait_for_opt(Duration::from_secs(8), || state(relay.id() as i32) == Some('T') && state(sup) == Some('T'));
    assert!(common::send_child(&mut relay, libc::SIGTERM));
    assert!(common::send_child(&mut relay, libc::SIGCONT));
    let st = wait_bounded(&mut relay, Duration::from_secs(8));
    std::thread::sleep(Duration::from_millis(300));
    let left = t.recorded().iter().filter(|&&(p, id)| same(p, id)).count();
    end_relay(relay, &bg, &[]);
    use std::os::unix::process::ExitStatusExt;
    assert!(stopped, "control: the relayed job did not stop");
    assert_eq!(st.and_then(|s| s.signal()), Some(libc::SIGTERM), "TERM and CONT did not end the stopped relayed job: {st:?}");
    assert_eq!(left, 0, "members were left after the job ended");
}

/// S5 review round 3 (P3-1): the decision "the job was continued" is taken once. A CONT during
/// the member wait, then a new TSTP in the gap after that decision (widened by a debug seam): the
/// new TSTP is a new stop (not swallowed by the called-off one), and when sheepr stops, its
/// TSTP-ignoring root is stopped too (never a member left running while sheepr stops).
#[test]
fn a_stop_after_the_continue_decision_leaves_no_member_running() {
    let t = Ticker::new();
    let log = std::env::temp_dir().join(format!("sr-s5-racelog-{}", t.marker));
    let ready = std::env::temp_dir().join(format!("sr-s5-raceready-{}", t.marker));
    let _ = std::fs::remove_file(&log);
    let _ = std::fs::remove_file(&ready);
    let mut cmd = vec!["/bin/sh".to_string(), "-c".into(), "trap '' TSTP; exec \"$0\" \"$@\"".into()];
    cmd.extend(t.args(&[]));
    let (l, r) = (log.display().to_string(), ready.display().to_string());
    let (c, id) = direct(&["--quiet"], &cmd, &[("SHEEPR_TEST_STOP_WAIT_MS", "1000"), ("SHEEPR_TEST_SIGNAL_LOG", &l), ("SHEEPR_TEST_SLEEP_AFTER_CONT_DECISION_MS", "800"), ("SHEEPR_TEST_READY_FILE", &r)]);
    let (root, _) = t.ready();
    t.ticking();
    let pg = c.id() as i32;
    assert!(send_group(pg, id, libc::SIGTSTP));
    let began = wait_for_opt(Duration::from_secs(5), || sent_signals(&log).iter().any(|&(p, s)| p != root && s == libc::SIGSTOP));
    assert!(send_group(pg, id, libc::SIGCONT));
    // the decision has been taken (the seam's ready file): the next TSTP lands in the gap after it
    let in_gap = wait_for_opt(Duration::from_secs(5), || ready.exists());
    let root_stops_before = sent_signals(&log).iter().filter(|&&(p, s)| p == root && s == libc::SIGSTOP).count();
    let saw_cont = log_has(&log, "decision continued");
    assert!(send_group(pg, id, libc::SIGTSTP));
    // the new stop: sheepr and its TSTP-ignoring root end up stopped (polled, bounded)
    let both = wait_for_opt(Duration::from_secs(8), || state(pg) == Some('T') && state(root) == Some('T'));
    let (sr_state, root_state) = (state(pg), state(root));
    end_direct(c, id, &t.marker);
    let _ = std::fs::remove_file(&log);
    let _ = std::fs::remove_file(&ready);
    assert!(began, "control: the first stop did not run");
    assert!(in_gap, "control: the continue decision was never reached");
    assert_eq!(root_stops_before, 0, "control: the first (called-off) stop stopped the root");
    assert!(saw_cont, "control: the decision did not see the CONT (the stop was not called off)");
    assert!(both, "after the second TSTP: sheepr {sr_state:?}, root {root_state:?} (both must be stopped)");
}

/// S5 review round 4 (P2-A): a pid-only STOP and CONT to the supervisor while it is inside its
/// stop sequence (a pid-only TSTP started it; the root ignores TSTP, so it waits). The relay
/// mirrors the STOP; the CONT, consumed inside the stop, must continue the relay too.
#[test]
fn a_cont_inside_the_stop_does_not_strand_the_relay() {
    let bg = new_marker();
    let m = short_marker(8);
    let log = std::env::temp_dir().join(format!("sr-s5-inside-{m}"));
    let _ = std::fs::remove_file(&log);
    let (relay, _) = relay_direct(&bg, &["/bin/sh".into(), "-c".into(), format!("trap '' TSTP; /bin/sleep {m}")], Some(&log));
    wait_for("the root", Duration::from_secs(15), || (sleeps(&m).len() == 1).then_some(()));
    let (sup, sup_id) = wait_for("the supervisor", Duration::from_secs(15), || supervisor_of(relay.id()));
    assert!(send(sup, sup_id, libc::SIGTSTP)); // the supervisor now waits (up to 3 s) for the root
    let waiting = wait_for_opt(Duration::from_secs(5), || log_has(&log, "stop-wait"));
    assert!(send(sup, sup_id, libc::SIGSTOP));
    let mirrored = wait_for_opt(Duration::from_secs(3), || state(relay.id() as i32) == Some('T'));
    assert!(send(sup, sup_id, libc::SIGCONT));
    let followed = wait_for_opt(Duration::from_secs(3), || state(relay.id() as i32) != Some('T'));
    let rs = state(relay.id() as i32);
    end_relay(relay, &bg, &[&m]);
    let _ = std::fs::remove_file(&log);
    assert!(waiting, "control: the supervisor never began its member wait");
    assert!(mirrored, "control: the relay did not mirror the supervisor's STOP");
    assert!(followed, "the relay stayed stopped after the supervisor was continued inside its stop (relay {rs:?})");
}

/// S5 review round 4 (P2-B): TERM and CONT to the relay while the supervisor has stopped but the
/// relay has not yet mirrored it (a debug seam holds the relay there): the relay forwards the
/// TERM (and wakes the supervisor) instead of stopping with it pending, and the job ends.
#[test]
fn term_and_cont_to_the_relay_before_it_mirrors_end_the_job() {
    let t = Ticker::new();
    let bg = new_marker();
    let ready = std::env::temp_dir().join(format!("sr-s5-mirror-{}", t.marker));
    let _ = std::fs::remove_file(&ready);
    let r = ready.display().to_string();
    let (mut relay, id) = relay_direct_env(&bg, &t.args(&[]), None, &[("SHEEPR_TEST_SLEEP_RELAY_BEFORE_MIRROR_MS", "1500"), ("SHEEPR_TEST_READY_FILE", &r)]);
    t.ready();
    t.ticking();
    assert!(send_group(relay.id() as i32, id, libc::SIGTSTP));
    // the supervisor has stopped the job and itself; the relay sits in the seam, not yet stopped
    let in_window = wait_for_opt(Duration::from_secs(8), || ready.exists());
    let rs = state(relay.id() as i32);
    assert!(common::send_child(&mut relay, libc::SIGTERM));
    assert!(common::send_child(&mut relay, libc::SIGCONT));
    let st = wait_bounded(&mut relay, Duration::from_secs(8));
    std::thread::sleep(Duration::from_millis(300));
    let left = t.recorded().iter().filter(|&&(p, id)| same(p, id)).count();
    let _ = std::fs::remove_file(&ready);
    end_relay(relay, &bg, &[]);
    use std::os::unix::process::ExitStatusExt;
    assert!(in_window, "control: the relay never reached the mirror (the job did not stop)");
    assert_ne!(rs, Some('T'), "control: the relay had already mirrored the stop");
    assert_eq!(st.and_then(|s| s.signal()), Some(libc::SIGTERM), "TERM and CONT before the mirror did not end the job: {st:?}");
    assert_eq!(left, 0, "members were left after the job ended");
}

/// S5 review round 5 (P3-1): the supervisor is continued alone (a CONT to its pid) while the relay
/// is about to mirror its stop: past its check that the supervisor is stopped, before its raise
/// (a debug seam holds the relay there; since the phase-1 review the relay checks first). The relay must not end up
/// stopped while the supervisor runs: the job runs on and nothing waits on a stopped relay.
#[test]
fn a_supervisor_continued_while_the_relay_mirrors_does_not_strand_it() {
    let t = Ticker::new();
    let bg = new_marker();
    let ready = std::env::temp_dir().join(format!("sr-s5-mirror2-{}", t.marker));
    let _ = std::fs::remove_file(&ready);
    let r = ready.display().to_string();
    let log = std::env::temp_dir().join(format!("sr-s5-mirrorlog-{}", t.marker));
    let _ = std::fs::remove_file(&log);
    let (relay, id) = relay_direct_env(&bg, &t.args(&[]), Some(&log), &[("SHEEPR_TEST_SLEEP_RELAY_BEFORE_RAISE_MS", "1500"), ("SHEEPR_TEST_READY_FILE", &r)]);
    t.ready();
    t.ticking();
    let (sup, sup_id) = wait_for("the supervisor", Duration::from_secs(15), || supervisor_of(relay.id()));
    assert!(send_group(relay.id() as i32, id, libc::SIGTSTP));
    let in_window = wait_for_opt(Duration::from_secs(8), || ready.exists());
    let sup_stopped = state(sup) == Some('T');
    assert!(send(sup, sup_id, libc::SIGCONT));
    // the relay leaves the seam 1.5 s after the ready file and mirrors the stop it read; the
    // supervisor, running, reads the relay in T and continues it (its trace says so): the relay is
    // not left in T
    let mirrored = wait_for_opt(Duration::from_secs(5), || log_has(&log, "relay-continued"));
    let freed = wait_for_opt(Duration::from_secs(5), || state(relay.id() as i32) != Some('T'));
    let (rs, ss) = (state(relay.id() as i32), state(sup));
    let ticks = t.advance();
    let _ = std::fs::remove_file(&ready);
    let _ = std::fs::remove_file(&log);
    end_relay(relay, &bg, &[]);
    assert!(in_window, "control: the relay never reached the mirror");
    assert!(sup_stopped, "control: the supervisor was not stopped when the relay reached the mirror");
    assert!(mirrored, "control: the supervisor never saw the relay stopped (it did not mirror the stop)");
    assert!(freed, "the relay stayed stopped while the supervisor runs");
    assert_ne!(rs, Some('T'), "the relay is stopped while the supervisor runs");
    assert_ne!(ss, Some('T'), "the supervisor is stopped");
    assert!(ticks > 0, "the job does not run");
}

/// S5 review round 6 (P3-1): a caller that ignores TERM. A TERM to the relay's pid during the
/// member wait must change nothing (TERM stays ignored for the whole job): the stop goes ahead,
/// the relay and the supervisor end up stopped, and the stop is never called off. Controls: the
/// same run without the TERM; and with TERM at its default, where the same TERM at the same
/// moment ends the job (so the moment is one where a TERM acts). Only Linux can be red here: it
/// delivers a blocked, ignored TERM on the relay's signalfd; macOS discards an ignored signal
/// when it is sent.
#[test]
fn an_ignored_term_to_the_relay_does_not_cancel_a_stop() {
    for (with_term, ignored) in [(false, true), (true, true), (true, false)] {
        let bg = new_marker();
        let m = short_marker(6);
        let log = std::env::temp_dir().join(format!("sr-s5-ignterm-{m}"));
        let _ = std::fs::remove_file(&log);
        let mut c = Command::new(fixture());
        c.args(["bg-then-exec", &bg])
            .args(run_args(&["--quiet"], &["/bin/sh".into(), "-c".into(), format!("trap '' TSTP; /bin/sleep {m}")]))
            .env("SHEEPR_TEST_STOP_WAIT_MS", "3000")
            .env("SHEEPR_TEST_SIGNAL_LOG", &log)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        unsafe {
            c.pre_exec(move || {
                for sig in [libc::SIGINT, libc::SIGHUP, libc::SIGTSTP, libc::SIGCONT] {
                    libc::signal(sig, libc::SIG_DFL);
                }
                libc::signal(libc::SIGTERM, if ignored { libc::SIG_IGN } else { libc::SIG_DFL });
                Ok(())
            });
        }
        c.process_group(0);
        let mut relay = c.spawn().unwrap();
        let rid = identity(relay.id() as i32).expect("the relay's identity");
        wait_for("the root", Duration::from_secs(15), || (sleeps(&m).len() == 1).then_some(()));
        let (sup, sup_id) = wait_for("the supervisor", Duration::from_secs(15), || supervisor_of(relay.id()));
        assert!(send_group(relay.id() as i32, rid, libc::SIGTSTP));
        let waiting = wait_for_opt(Duration::from_secs(5), || log_has(&log, "stop-wait"));
        if with_term {
            assert!(common::send_child(&mut relay, libc::SIGTERM));
        }
        let ended = if ignored { None } else { wait_bounded(&mut relay, Duration::from_secs(6)) };
        let both = ignored && wait_for_opt(Duration::from_secs(6), || state(relay.id() as i32) == Some('T') && state(sup) == Some('T'));
        let called_off = log_has(&log, "decision continued");
        // cleanup: TERM is ignored, so KILL (the relay, then the supervisor by identity)
        common::send_child(&mut relay, libc::SIGKILL);
        let _ = relay.wait();
        send(sup, sup_id, libc::SIGKILL);
        common::kill_marked(&[&bg, &m]);
        let _ = std::fs::remove_file(&log);
        assert!(waiting, "term={with_term}: control: the supervisor never began its member wait");
        if ignored {
            assert!(both, "term={with_term}: the job did not stop (relay and supervisor in T)");
            assert!(!called_off, "term={with_term}: an ignored TERM called the stop off");
        } else {
            use std::os::unix::process::ExitStatusExt;
            assert_eq!(ended.and_then(|s| s.signal()), Some(libc::SIGTERM), "control: a watched TERM at that moment did not end the job: {ended:?}");
        }
    }
}

// ---- cell 6 (S7): a stopped child whose parent dies ----------------------------------------

/// Cell 6: a child that handles TERM stops itself, then its parent exits. Two measured facts
/// shape the cell: a TERM at its default action ends even a stopped process (macOS 27), and when
/// a stopped process's group becomes orphaned the kernel sends it HUP and CONT. So the route leaks
/// only for a child that handles TERM, while its group stays non-orphaned, as in a terminal job.
/// The job here is `sh -c 'sheepr run ...; wait for HOLD'`: the outer shell (its parent, the
/// pty's shell, is in another group of the same session) keeps the group non-orphaned even after
/// sheepr exits, so only sheepr's own signals can end the child. Legs: 0, the naive TERM
/// leaves it stopped and alive (the control); 1, sheepr ends it before it exits; 2,
/// `--leave-strays`: it survives sheepr (the control of leg 1).
#[test]
fn cell6_a_stopped_orphan_is_killed_and_term_alone_leaks() {
    for leg in 0..3 {
        let leave = leg == 2;
        let m = new_marker();
        let go = std::env::temp_dir().join(format!("sr-cell6-{m}"));
        let hold = std::env::temp_dir().join(format!("sr-cell6-hold-{m}"));
        let _ = std::fs::remove_file(&go);
        let _ = std::fs::remove_file(&hold);
        // the child: trap TERM, stop itself, then (if ever continued) loop for 30 s at most
        let child = "trap \"exit 0\" TERM; kill -STOP $$; i=0; while [ $i -lt 300 ]; do /bin/sleep 0.1; i=$((i+1)); done";
        let outer = format!(
            "/bin/sh -c '/bin/sh -c '\\''{child}'\\'' {m} & exit 0'; while [ ! -e {go} ]; do /bin/sleep 0.01; done",
            go = go.display()
        );
        let flags: &[&str] = if leave { &["--quiet", "--leave-strays"] } else { &["--quiet"] };
        let mut job = vec!["/bin/sh".to_string(), "-c".to_string(), format!("\"$@\"; while [ ! -e {} ]; do /bin/sleep 0.01; done", hold.display()), "job".to_string()];
        job.extend(run_args(flags, &["/bin/sh".to_string(), "-c".to_string(), outer]));
        let mut pty = Pty::shell(&[], &job);
        pty.started();
        // the orphan: the child (its argv ends with the marker, its $0), stopped, and its
        // parent is no longer a shell (the inner shell exited)
        let (p, id) = wait_for("the stopped orphan", Duration::from_secs(15), || {
            marked(&m).into_iter().find_map(|p| {
                if state(p) != Some('T') {
                    return None;
                }
                let ppid: i32 = String::from_utf8_lossy(&Command::new("ps").args(["-o", "ppid=", "-p", &p.to_string()]).output().ok()?.stdout).trim().parse().ok()?;
                let comm = String::from_utf8_lossy(&Command::new("ps").args(["-o", "comm=", "-p", &ppid.to_string()]).output().ok()?.stdout).trim().to_string();
                (!comm.rsplit('/').next().unwrap_or("").ends_with("sh")).then(|| (p, identity(p)))
            })
        });
        let id = id.expect("the orphan's identity");
        // the naive TERM, in leg 0 only: a TERM left pending would end the child as soon as
        // anything continues it
        let after_term = if leg == 0 {
            assert!(send(p, id, libc::SIGTERM));
            std::thread::sleep(Duration::from_millis(300));
            (same(p, id), state(p))
        } else {
            (true, Some('T'))
        };
        // let the root exit, then wait for sheepr to be gone (the outer shell still holds the
        // group), and look at the child before anything else can act on it
        std::fs::File::create(&go).unwrap();
        let sr_gone = wait_for_opt(Duration::from_secs(20), || {
            let out = Command::new("ps").args(["-Ao", "pid=,args="]).output().unwrap();
            !String::from_utf8_lossy(&out.stdout).lines().any(|l| {
                let w: Vec<&str> = l.split_whitespace().collect();
                w.len() > 1 && w[1].ends_with("sheepr") && w.iter().any(|x| *x == m)
            })
        });
        std::thread::sleep(Duration::from_millis(200));
        let survived = same(p, id);
        send(p, id, libc::SIGKILL);
        std::fs::File::create(&hold).unwrap();
        let end = pty.outcome(Duration::from_secs(20));
        let _ = std::fs::remove_file(&go);
        let _ = std::fs::remove_file(&hold);
        assert_eq!(after_term, (true, Some('T')), "control: TERM ended the stopped orphan (the naive kill did not leak)");
        assert!(sr_gone, "sheepr did not end");
        assert!(end.is_some(), "the job did not end");
        match leg {
            1 => assert!(!survived, "the stopped orphan survived sheepr"),
            2 => assert!(survived, "control: with --leave-strays the stopped orphan should survive sheepr"),
            _ => {}
        }
    }
}

// ---- S1 (macOS consumption), built in the phase-1 review --------------------------------------

/// PHASE1 S1 (macOS): sheepr clears a pending signal with the SIG_IGN/SIG_DFL toggle, never
/// with `sigwait`. A CONT sent while a TSTP is pending discards that TSTP, so a `sigwait` for it
/// would block for good and a later TERM could never end the job. A debug seam holds sheepr
/// right after it saw the TSTP pending (it creates the ready file there): the test sends CONT,
/// then TERM, and sheepr must die of the TERM within a bound.
#[cfg(target_os = "macos")]
#[test]
fn a_cont_while_a_tstp_is_pending_does_not_block_a_later_term() {
    let m = short_marker(20);
    let ready = std::env::temp_dir().join(format!("sr-s1-consume-{m}"));
    let _ = std::fs::remove_file(&ready);
    let r = ready.display().to_string();
    let (mut c, id) = direct(&["--quiet"], &["/bin/sleep".into(), m.clone()], &[("SHEEPR_TEST_SLEEP_IN_CONSUME_MS", "1500"), ("SHEEPR_TEST_READY_FILE", &r)]);
    wait_for("the root", Duration::from_secs(15), || (sleeps(&m).len() == 1).then_some(()));
    assert!(send(c.id() as i32, id, libc::SIGTSTP));
    let in_window = wait_for_opt(Duration::from_secs(10), || ready.exists());
    assert!(send(c.id() as i32, id, libc::SIGCONT));
    assert!(send(c.id() as i32, id, libc::SIGTERM));
    let st = wait_bounded(&mut c, Duration::from_secs(8));
    if st.is_none() {
        common::send_child(&mut c, libc::SIGKILL);
        let _ = c.wait();
    }
    let left = sleeps(&m);
    for p in &left {
        if let Some(pid_id) = identity(*p) {
            send(*p, pid_id, libc::SIGKILL);
        }
    }
    let _ = std::fs::remove_file(&ready);
    use std::os::unix::process::ExitStatusExt;
    assert!(in_window, "control: sheepr never saw the TSTP pending (the seam was not reached)");
    assert_eq!(st.and_then(|s| s.signal()), Some(libc::SIGTERM), "the TERM after the CONT did not end the job: {st:?}");
    assert_eq!(left, Vec::<i32>::new(), "the job survived");
}

// ---- phase-1 review: every signal that ends by default ------------------------------------

/// Phase-1 review (P1): ctrl-\ sends QUIT to the foreground group. The root dies of it; sheepr
/// must still run its kill (the escapee is gone), and then die of QUIT as the root did (the same
/// rule as INT). Before the fix sheepr died of QUIT at once and the escapee lived.
#[test]
fn ctrl_backslash_ends_the_job_and_leaves_no_escapee() {
    let job = Job::new();
    let mut pty = Pty::shell(&[], &run_args(&[], &job.args()));
    pty.started();
    let ((esc, esc_id), (root, root_id)) = job.ready();
    pty.write(b"\x1c");
    let end = pty.outcome(Duration::from_secs(10));
    let esc_gone = wait_for_opt(Duration::from_secs(5), || !same(esc, esc_id));
    let root_alive = same(root, root_id);
    assert!(esc_gone, "sheepr ended ({end:?}) and left the escapee {esc} running");
    assert!(!root_alive, "control: the root survived ctrl-\\");
    assert_eq!(end, Some(format!("signaled {}", libc::SIGQUIT)), "sheepr did not die of QUIT as the root did");
}

/// A signal whose default action ends a process, sent to sheepr's pid while it holds right
/// after its first freeze (debug seam): nothing may be left stopped or alive, and sheepr exits
/// as the root did (it exited 0; the signal did not end it).
fn a_signal_in_the_freeze_leaves_nothing(sig: libc::c_int) {
    let m = short_marker(20);
    let dir = std::env::temp_dir().join(format!("sr-p1-freeze-{m}"));
    std::fs::create_dir_all(&dir).unwrap();
    let (ready, release) = (dir.join("ready"), dir.join("release"));
    let (r, rel) = (ready.display().to_string(), release.display().to_string());
    // the member ignores HUP: if sheepr dies in the freeze, the kernel's HUP+CONT to the
    // orphaned group must not end it for sheepr (the member is then seen alive)
    let script = format!("trap '' HUP; /bin/sleep {m} & exit 0");
    let (mut c, id) = direct(&["--quiet", "--grace", "0"], &["/bin/sh".into(), "-c".into(), script], &[("SHEEPR_TEST_HOLD_AFTER_FREEZE", &rel), ("SHEEPR_TEST_READY_FILE", &r)]);
    let in_window = wait_for_opt(Duration::from_secs(15), || ready.exists());
    assert!(send(c.id() as i32, id, sig));
    std::fs::write(&release, "x").unwrap();
    let st = wait_bounded(&mut c, Duration::from_secs(20));
    if st.is_none() {
        common::send_child(&mut c, libc::SIGKILL);
        let _ = c.wait();
    }
    let left: Vec<(i32, Option<char>)> = sleeps(&m).into_iter().map(|p| (p, state(p))).collect();
    for (p, _) in &left {
        if let Some(pid_id) = identity(*p) {
            send(*p, pid_id, libc::SIGKILL);
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(in_window, "control: the kill never reached its freeze");
    assert_eq!(left, vec![], "signal {sig}: members were left (stopped or alive)");
    assert_eq!(st.and_then(|s| s.code()), Some(0), "signal {sig}: sheepr did not exit as the root did: {st:?}");
}

/// Phase-1 review (P1): QUIT between the freeze and the KILL.
#[test]
fn a_quit_in_the_freeze_leaves_nothing() {
    a_signal_in_the_freeze_leaves_nothing(libc::SIGQUIT);
}

/// Phase-1 review (P2): USR1 (a signal sheepr never uses) between the freeze and the KILL.
#[test]
fn a_usr1_in_the_freeze_leaves_nothing() {
    a_signal_in_the_freeze_leaves_nothing(libc::SIGUSR1);
}

/// Phase-1 review (P2): sheepr's stderr is a pipe nobody reads. Its deadline report then gets
/// EPIPE, never SIGPIPE: the exit is 125 (the deadline), not death by SIGPIPE (141).
#[test]
fn a_closed_stderr_does_not_turn_a_missed_deadline_into_sigpipe() {
    let m = short_marker(20);
    let script = format!("/bin/sleep {m} & exit 0");
    let mut c = Command::new(sheepr());
    c.args(["run", "--quiet", "--grace", "0", "--", "/bin/sh", "-c", &script])
        .env("SHEEPR_TEST_NEVER_EMPTY", "1")
        .env("SHEEPR_TEST_DEADLINE_MS", "300")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut c = c.spawn().unwrap();
    drop(c.stderr.take()); // the read end closes: every write gets EPIPE (or SIGPIPE)
    let st = wait_bounded(&mut c, Duration::from_secs(20));
    for p in sleeps(&m) {
        if let Some(pid_id) = identity(p) {
            send(p, pid_id, libc::SIGKILL);
        }
    }
    use std::os::unix::process::ExitStatusExt;
    assert_eq!(st.map(|s| (s.code(), s.signal())), Some((Some(125), None)), "a missed deadline with a closed stderr: {st:?}");
}

/// Phase-1 review (P3): the supervisor is continued (a group CONT, as `fg` sends) while the relay
/// sits between reading the supervisor's stop and mirroring it, and the root exits at once. The
/// supervisor ends the job; the relay (the pid the caller waits on) must end too, not stop for
/// good with nothing left to continue it.
#[test]
fn the_relay_is_not_stranded_by_a_stop_after_the_job_ended() {
    let bg = new_marker();
    let m = short_marker(7);
    let dir = std::env::temp_dir().join(format!("sr-p1-relay-{m}"));
    std::fs::create_dir_all(&dir).unwrap();
    let (ready, go, started) = (dir.join("ready"), dir.join("go"), dir.join("started"));
    let r = ready.display().to_string();
    let root = format!(": > '{}'; while [ ! -e '{}' ]; do /bin/sleep 0.02; done", started.display(), go.display());
    let (mut relay, id) = relay_direct_env(&bg, &["/bin/sh".into(), "-c".into(), root], None, &[("SHEEPR_TEST_SLEEP_RELAY_BEFORE_MIRROR_MS", "1500"), ("SHEEPR_TEST_READY_FILE", &r)]);
    wait_for("the root", Duration::from_secs(15), || started.exists().then_some(()));
    let (sup, sup_id) = wait_for("the supervisor", Duration::from_secs(15), || supervisor_of(relay.id()));
    assert!(send_group(relay.id() as i32, id, libc::SIGTSTP));
    let in_window = wait_for_opt(Duration::from_secs(8), || ready.exists());
    let sup_stopped = state(sup) == Some('T');
    std::fs::write(&go, "x").unwrap();
    assert!(send_group(relay.id() as i32, id, libc::SIGCONT));
    let sup_gone = wait_for_opt(Duration::from_secs(8), || !same(sup, sup_id));
    // the relay leaves the seam 1.5 s after the ready file: give it that and a margin, bounded
    let exited = wait_for_opt(Duration::from_secs(6), || matches!(relay.try_wait(), Ok(Some(_))));
    let rs = state(relay.id() as i32);
    let _ = std::fs::remove_dir_all(&dir);
    end_relay(relay, &bg, &[]);
    assert!(in_window, "control: the relay never reached the mirror");
    assert!(sup_stopped, "control: the supervisor was not stopped at the mirror");
    assert!(sup_gone, "control: the supervisor did not end after the root exited");
    assert!(exited, "the job ended but the relay (the pid the caller waits on) is left in state {rs:?}");
}

/// Phase-1 review round 2 (P3, macOS): a STOP and a quick CONT of the supervisor interrupt its
/// kevent (EINTR). That is not a failure: sheepr must not fall back to polling (its signal log
/// records a fallback as `polling`). The error was read after the membership scan, whose own
/// calls had replaced it.
#[cfg(target_os = "macos")]
#[test]
fn a_stop_and_cont_of_the_supervisor_is_not_a_kqueue_failure() {
    let m = short_marker(20);
    let log = std::env::temp_dir().join(format!("sr-p1-kq-{m}"));
    let _ = std::fs::remove_file(&log);
    let l = log.display().to_string();
    let (mut c, id) = direct(&["--quiet"], &["/bin/sleep".into(), m.clone()], &[("SHEEPR_TEST_SIGNAL_LOG", &l)]);
    wait_for("the root", Duration::from_secs(15), || (sleeps(&m).len() == 1).then_some(()));
    for _ in 0..5 {
        assert!(send(c.id() as i32, id, libc::SIGSTOP));
        std::thread::sleep(Duration::from_millis(20));
        assert!(send(c.id() as i32, id, libc::SIGCONT));
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(send(c.id() as i32, id, libc::SIGTERM));
    let st = wait_bounded(&mut c, Duration::from_secs(10));
    for p in sleeps(&m) {
        if let Some(pid_id) = identity(p) {
            send(p, pid_id, libc::SIGKILL);
        }
    }
    let polled = log_has(&log, "polling");
    let _ = std::fs::remove_file(&log);
    use std::os::unix::process::ExitStatusExt;
    assert_eq!(st.and_then(|s| s.signal()), Some(libc::SIGTERM), "control: TERM did not end the job: {st:?}");
    assert!(!polled, "a STOP and CONT of the supervisor made it fall back to polling");
}

/// The signals whose default action ends a process and that are not faults, by rule (never a
/// hand-copied list): the classic signals 1 to 31 and, on Linux, the real-time ones the C library
/// hands out (SIGRTMIN to SIGRTMAX), minus KILL and STOP, the faults (SEGV, BUS, ILL, FPE, TRAP,
/// SYS, ABRT), the job-control signals and CHLD, and the ones whose default action is to ignore.
/// Linux's 32 to SIGRTMIN-1 belong to the C library (glibc and musl use them internally and
/// refuse to let a program block them): stated in PHASE1, not covered.
fn signals_that_end_by_default() -> Vec<libc::c_int> {
    let not = [
        libc::SIGKILL, libc::SIGSTOP, libc::SIGSEGV, libc::SIGBUS, libc::SIGILL, libc::SIGFPE, libc::SIGTRAP, libc::SIGSYS, libc::SIGABRT,
        libc::SIGTSTP, libc::SIGTTIN, libc::SIGTTOU, libc::SIGCONT, libc::SIGCHLD, libc::SIGURG, libc::SIGWINCH,
        #[cfg(target_os = "macos")]
        libc::SIGINFO,
    ];
    #[cfg(target_os = "linux")]
    let rt = libc::SIGRTMIN()..=libc::SIGRTMAX();
    #[cfg(target_os = "macos")]
    let rt = 1..=0;
    // measured, not assumed: keep a signal only if it ends a throwaway child at its default
    // action; each kept signal is its own control (the cell checks the result against the rule)
    (1..=31)
        .chain(rt)
        .filter(|s| !not.contains(s))
        .filter(|&sig| {
            let mut c = Command::new("/bin/sleep").arg("5").spawn().unwrap();
            common::send_child(&mut c, sig);
            let st = wait_bounded(&mut c, Duration::from_millis(500));
            if st.is_none() {
                common::send_child(&mut c, libc::SIGKILL);
                let _ = c.wait();
            }
            use std::os::unix::process::ExitStatusExt;
            st.and_then(|s| s.signal()) == Some(sig)
        })
        .collect()
}

/// Phase-1 review round 2 (P2): no such signal, sent to sheepr's pid, ends it: it keeps its
/// job (TERM then ends it, the escapee included). Before the rule, a hand-copied list left
/// SIGEMT (macOS) and PWR, IO, STKFLT and the real-time signals (Linux) able to end sheepr
/// without its kill.
#[test]
fn no_signal_that_ends_by_default_ends_sheepr_without_its_kill() {
    let mut ended = Vec::new();
    let set = signals_that_end_by_default();
    // the measured set is exactly the rule minus the signals this OS discards by default; a signal
    // dropped for another reason (inherited as ignored, e.g. HUP under nohup) is red, never a
    // quietly smaller cell
    #[cfg(target_os = "macos")]
    let discarded = [libc::SIGIO];
    #[cfg(target_os = "linux")]
    let discarded: [libc::c_int; 0] = [];
    let not = [
        libc::SIGKILL, libc::SIGSTOP, libc::SIGSEGV, libc::SIGBUS, libc::SIGILL, libc::SIGFPE, libc::SIGTRAP, libc::SIGSYS, libc::SIGABRT,
        libc::SIGTSTP, libc::SIGTTIN, libc::SIGTTOU, libc::SIGCONT, libc::SIGCHLD, libc::SIGURG, libc::SIGWINCH,
        #[cfg(target_os = "macos")]
        libc::SIGINFO,
    ];
    #[cfg(target_os = "linux")]
    let rt = libc::SIGRTMIN()..=libc::SIGRTMAX();
    #[cfg(target_os = "macos")]
    let rt = 1..=0;
    let rule: Vec<libc::c_int> = (1..=31).chain(rt).filter(|s| !not.contains(s) && !discarded.contains(s)).collect();
    assert_eq!(set, rule, "control: the measured set is not the rule minus this OS's default-discarded signals");
    for sig in set {
        if [libc::SIGTERM].contains(&sig) {
            continue; // TERM ends the job by design (with its kill)
        }
        let job = Job::new();
        let (mut c, id) = direct(&["--quiet"], &job.args(), &[]);
        let ((esc, esc_id), _) = job.ready();
        assert!(send(c.id() as i32, id, sig));
        std::thread::sleep(Duration::from_millis(150));
        let alive = matches!(c.try_wait(), Ok(None));
        if alive {
            common::send_child(&mut c, libc::SIGTERM);
        }
        let _ = wait_bounded(&mut c, Duration::from_secs(10));
        let esc_gone = wait_for_opt(Duration::from_secs(5), || !same(esc, esc_id));
        if !alive || !esc_gone {
            ended.push((sig, alive, esc_gone));
        }
    }
    assert_eq!(ended, vec![], "(signal, sheepr survived it, the escapee is gone)");
}

/// P3: `--timeout` counts running time only. A 2.5 s ctrl-Z inside a 2 s timeout (0.5 s after
/// the start): the job is ended by the timeout (124) only after it ran 2 s, so no sooner than
/// 0.5 + 2.5 + 1.5 = 4.5 s after the start (asserted: 4 s); counting the stop would end it at
/// the `fg`, about 3 s.
#[test]
fn a_ctrl_z_does_not_count_against_the_timeout() {
    let start = std::time::Instant::now();
    let mut pty = Pty::shell(&[], &run_args(&["--quiet", "--timeout", "2s"], &["/bin/sleep".to_string(), "30".to_string()]));
    pty.started();
    std::thread::sleep(Duration::from_millis(500));
    pty.write(b"\x1a");
    let stopped = pty.wait_line("stopped", Duration::from_secs(10), |l| l.starts_with("stopped "));
    std::thread::sleep(Duration::from_millis(2500));
    fg(&mut pty, 1);
    let end = pty.outcome(Duration::from_secs(20));
    let took = start.elapsed();
    assert!(stopped.is_some(), "the job did not stop");
    assert_eq!(end, Some("exited 124".to_string()), "the timeout ends the job");
    assert!(took >= Duration::from_secs(4), "ended after {took:?}: the stopped time was counted");
    assert!(took < Duration::from_secs(15), "took {took:?}");
}

/// Cell 21(e) with `--timeout`: in an orphaned group (sheepr leads its own session, no
/// terminal) the kernel discards sheepr's own stop, so a TSTP costs no running time: the 2 s
/// timeout fires at about 2 s (trigger_at under 2.8 s), not 1 s later for the stop's wait.
#[test]
fn cell21e_a_discarded_stop_counts_all_time() {
    let dir = std::env::temp_dir().join(format!("sr-p3-21e-{}", new_marker()));
    std::fs::create_dir_all(&dir).unwrap();
    let status = std::fs::File::create(dir.join("status")).unwrap();
    let sfd = std::os::fd::AsRawFd::as_raw_fd(&status);
    let mut c = Command::new(sheepr());
    c.args(["run", "--quiet", "--timeout", "2s", "--status-fd", "3", "--", "/bin/sleep", "5"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    unsafe {
        c.pre_exec(move || {
            for sig in [libc::SIGINT, libc::SIGHUP, libc::SIGTERM, libc::SIGTSTP, libc::SIGCONT] {
                libc::signal(sig, libc::SIG_DFL);
            }
            // dup2(3, 3) would keep the fd close-on-exec: then clear the flag instead
            if sfd == 3 {
                libc::fcntl(3, libc::F_SETFD, 0);
            } else {
                libc::dup2(sfd, 3);
            }
            libc::setsid();
            Ok(())
        });
    }
    let mut c = c.spawn().unwrap();
    let id = identity(c.id() as i32).expect("sheepr's identity");
    let (root, root_id) = wait_for("the root", Duration::from_secs(15), || child_of(c.id() as i32, "sleep"));
    std::thread::sleep(Duration::from_millis(300));
    assert!(send_group(c.id() as i32, id, libc::SIGTSTP));
    let st = wait_bounded(&mut c, Duration::from_secs(15));
    if st.is_none() {
        common::send_child(&mut c, libc::SIGKILL);
        let _ = c.wait();
    }
    send(root, root_id, libc::SIGKILL);
    let text = std::fs::read_to_string(dir.join("status")).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&dir);
    let j = common::json::parse(text.trim_end()).expect("a status line");
    assert_eq!(st.and_then(|s| s.code()), Some(124), "{text}");
    assert_eq!(j.get("trigger").and_then(common::json::Json::str), Some("timeout"));
    let at = j.get("trigger_at").and_then(common::json::Json::num).unwrap_or(0.0);
    assert!((2000.0..2800.0).contains(&at), "the timeout fired at {at} ms: the discarded stop was not counted as running time");
}

