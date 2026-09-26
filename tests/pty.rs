//! Phase 1, step S4 (PHASE1.md §2 and S4): the pty harness, and INT/HUP forwarding.
//!
//! Safety rules of this file (the 2026-09-26 host incident was a test that sent `kill(-1)`):
//! - a pid is signalled only after its recorded identity is re-checked (`send`);
//! - a process group is signalled only if this test created it and its leader still has the
//!   recorded identity (`send_group`);
//! - nothing is ever sent to a pid of 1 or less.
//!
//! Readiness only (record files, the shell's report), never a fixed sleep before an action.

use sheepdog::ident::{identity, same};
use std::os::unix::io::RawFd;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

static SEQ: AtomicUsize = AtomicUsize::new(0);

fn sheepdog() -> &'static str {
    env!("CARGO_BIN_EXE_sheepdog")
}
fn fixture() -> &'static str {
    env!("CARGO_BIN_EXE_sd-fixture")
}

mod common;
use common::{send, send_group};

/// The negative numbers in a hint (the `kill -INT -<pgid>` targets), as values.
fn targets(text: &str) -> Vec<i64> {
    text.split(|c: char| c.is_whitespace() || "(),.".contains(c)).filter_map(|w| w.strip_prefix('-')?.parse::<i64>().ok()).collect()
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

/// The pids whose argv carries `marker` as a whole word (sheepdog's own argv included).
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

/// The sheepdog child of `relay` (the relay path was taken), with its identity.
fn supervisor_of(relay: u32) -> Option<(i32, u64)> {
    let out = Command::new("ps").args(["-Ao", "pid=,ppid=,args="]).output().expect("ps");
    String::from_utf8_lossy(&out.stdout).lines().find_map(|l| {
        let w: Vec<&str> = l.split_whitespace().collect();
        let p: i32 = w.first()?.parse().ok()?;
        (w.len() >= 3 && w[1] == relay.to_string() && w[2].ends_with("sheepdog")).then(|| Some((p, identity(p)?))).flatten()
    })
}

fn new_marker() -> String {
    format!("28.{}{:06}", std::process::id(), SEQ.fetch_add(1, Ordering::SeqCst))
}

/// One `sd-fixture counter` job: a root and one escapee, both counting INT and HUP.
struct Job {
    marker: String,
    rec: PathBuf,
}

impl Job {
    fn new() -> Self {
        let marker = new_marker();
        let rec = std::env::temp_dir().join(format!("sd-s4-{marker}"));
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
        // marker: only `sd-fixture counter <marker>` processes, identity read at the scan
        let out = Command::new("ps").args(["-Ao", "pid=,args="]).output();
        if let Ok(out) = out {
            for l in String::from_utf8_lossy(&out.stdout).lines() {
                let w: Vec<&str> = l.split_whitespace().collect();
                if w.len() >= 4 && w[1].ends_with("sd-fixture") && w[2] == "counter" && w[3] == self.marker {
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
        let report = std::env::temp_dir().join(format!("sd-pty-{}", new_marker()));
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

    /// `sd-fixture shell REPORT opts... prog args...` as the session leader.
    fn shell(opts: &[&str], cmd: &[String]) -> Pty {
        let report = std::env::temp_dir().join(format!("sd-pty-{}", new_marker()));
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
    let mut v = vec![sheepdog().to_string(), "run".into()];
    v.extend(flags.iter().map(|s| s.to_string()));
    v.push("--".into());
    v.extend(cmd.iter().cloned());
    v
}

/// sheepdog in its own process group, no terminal involved; INT/HUP/TERM at their defaults.
fn direct(flags: &[&str], cmd: &[String], env: &[(&str, &str)]) -> (Child, u64) {
    let mut c = Command::new(sheepdog());
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
    let id = identity(child.id() as i32).expect("sheepdog's identity");
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

/// End a direct sheepdog by TERM (identity-checked) and return its stderr. The job's members
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

/// PHASE1.md §2, the harness's control cell (no sheepdog): ctrl-Z stops the job and the
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
/// the tty's INT; the escapee (its own session) gets exactly one, from sheepdog.
#[test]
fn cell22a_ctrl_c_in_the_foreground_reaches_each_member_once() {
    let job = Job::new();
    let mut pty = Pty::shell(&["null-stdin"], &run_args(&[], &job.args()));
    pty.started();
    let ((esc, _), (root, _)) = job.ready();
    pty.write(b"\x03");
    let (r, e) = job.settle(root, esc, |r, e| r.0 >= 1 && e.0 >= 1);
    assert_eq!(r.0, 1, "the root must count exactly one INT (the tty's)");
    assert_eq!(e.0, 1, "the escapee must count exactly one INT (sheepdog's)");
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
/// then a group HUP. sheepdog is not the session leader here.
#[test]
fn cell22c_group_signals_without_a_terminal_reach_each_member_once() {
    let job = Job::new();
    let pidfile = std::env::temp_dir().join(format!("sd-s4-pid-{}", job.marker));
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
    // "<pid> <identity>": nosession reads the identity before it can reap sheepdog
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
    let (sd, sd_id) = pty.started();
    let ((esc, _), (root, _)) = job.ready();
    pty.close_master();
    let forwarded = pty.wait_line("the shell's HUP", Duration::from_secs(10), |l| l == "hup");
    let (r, e) = job.settle(root, esc, |r, e| r.1 >= 1 && e.1 >= 1);
    assert!(same(sd, sd_id), "HUP must not end the job");
    assert!(forwarded.is_some(), "control: the shell never got the terminal's HUP");
    assert_eq!((r.1, e.1), (1, 1), "each member must count exactly one HUP (root, escapee)");
}

/// Cell 22(f), the stated limit: INT to the sheepdog pid only reaches the escapees, not the
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

/// Cell 22'(g): sheepdog is the session leader of a pty and the master closes. The kernel
/// sends HUP to sheepdog only, so sheepdog forwards it to every member, the root included.
#[test]
fn cell22g_a_session_leader_forwards_hup_to_every_member() {
    let job = Job::new();
    let mut pty = Pty::leader(sheepdog(), &run_args(&[], &job.args())[1..]);
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
    assert!(relayed, "control: this is not the relay path (no sheepdog child of the relay)");
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
    let (c, id) = direct(&[], &job.args(), &[("SHEEPDOG_TEST_TICK_MS", "900")]);
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
        assert!(has_supervisor, "{flags:?}: control: this is not the relay path (no sheepdog child of the relay)");
        assert!(relay_alive, "{flags:?}: an INT ended the relay");
        assert_eq!((r.0, e.0), (0, 0), "{flags:?}: an INT to the relay's pid reached (root, escapee)");
    }
}

/// PHASE1.md §1.3: an INT counts "at any point before sheepdog exits", also one that comes
/// during the kill. The root dies of INT by itself (sheepdog got none), and while the kill waits
/// out the grace (the escapee ignores TERM), sheepdog gets an INT: it dies of INT. Control: the
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
            assert_eq!(st.and_then(|s| s.code()), Some(130), "control: without sheepdog's own INT it exits 130: {st:?}");
        }
    }
}

// ---- the hint (DevEx D9) and its flags ---------------------------------------------------

/// S4 (PLAN.md §3.1, D9): INT to the pid while the root keeps running prints one hint, once
/// per job, naming the real process group (debug seam: the 3 s delay is 300 ms).
#[test]
fn the_pid_only_hint_is_printed_once_with_the_group() {
    let job = Job::new();
    let (c, id) = direct(&[], &job.args(), &[("SHEEPDOG_TEST_HINT_MS", "300")]);
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
    let (c, id) = direct(&["--quiet"], &job.args(), &[("SHEEPDOG_TEST_HINT_MS", "300")]);
    job.ready();
    assert!(send(c.id() as i32, id, libc::SIGINT));
    std::thread::sleep(Duration::from_millis(900));
    let err = end(c, id, &job);
    assert!(err.trim().is_empty(), "--quiet printed: {err:?}");
}

/// S4 (PLAN.md §3.1): `--forward-int-to-root` also forwards a pid-only INT to the root, and
/// then no hint is printed (sheepdog did deliver it).
#[test]
fn forward_int_to_root_reaches_the_root_and_prints_no_hint() {
    let job = Job::new();
    let (c, id) = direct(&["--forward-int-to-root"], &job.args(), &[("SHEEPDOG_TEST_HINT_MS", "300")]);
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
    let mut cmd = vec!["/usr/bin/env".to_string(), "SHEEPDOG_TEST_HINT_MS=300".into()];
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
    assert!(shown.trim().is_empty(), "sheepdog printed on the terminal after a ctrl-C the root survived: {shown:?}");
}

/// S4 review (B-P2-1): death by signal needs the SAME signal. sheepdog consumed an INT, but the
/// root died of KILL (from outside): sheepdog exits 128+9, it does not die of a signal.
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
    assert_eq!(st.and_then(|s| s.signal()), None, "sheepdog died of a signal the root did not die of: {st:?}");
    assert_eq!(st.and_then(|s| s.code()), Some(128 + libc::SIGKILL), "exit code: {st:?}");
}

/// S4 review (B-P2-3): `--forward-int-to-root` is for INT only. A pid-only HUP still reaches only
/// the escapee, and the hint is printed.
#[test]
fn forward_int_to_root_does_not_forward_hup_to_the_root() {
    let job = Job::new();
    let (c, id) = direct(&["--forward-int-to-root"], &job.args(), &[("SHEEPDOG_TEST_HINT_MS", "300")]);
    let ((esc, _), (root, _)) = job.ready();
    assert!(send(c.id() as i32, id, libc::SIGHUP));
    let (r, e) = job.settle(root, esc, |_, e| e.1 >= 1);
    std::thread::sleep(Duration::from_millis(400));
    let err = end(c, id, &job);
    assert_eq!((r.1, e.1), (0, 1), "a pid-only HUP with --forward-int-to-root: (root, escapee)");
    assert_eq!(err.lines().filter(|l| !l.trim().is_empty()).count(), 1, "the hint must be printed once: {err:?}");
}

/// S4 review round 2 (A-P2-2): the hint names a process group only when it is sheepdog's own
/// (sheepdog or its relay leads it). Here a shell leads the group and runs sheepdog as a child:
/// that group is the caller's, and signalling it would reach the caller too. The hint is
/// printed once and names no group.
#[test]
fn the_hint_does_not_name_the_callers_group() {
    let job = Job::new();
    let mut c = Command::new("/bin/sh");
    let script = "\"$0\" run -- \"$@\"; exit 0";
    c.args(["-c", script, sheepdog()]).args(job.args()).env("SHEEPDOG_TEST_HINT_MS", "300").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
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
    let sd = wait_for("sheepdog under the shell", Duration::from_secs(15), || {
        let out = Command::new("ps").args(["-Ao", "pid=,ppid=,args="]).output().ok()?;
        String::from_utf8_lossy(&out.stdout).lines().find_map(|l| {
            let w: Vec<&str> = l.split_whitespace().collect();
            (w.len() >= 3 && w[1] == pg.to_string() && w[2].ends_with("sheepdog")).then(|| w[0].parse::<i32>().ok()).flatten()
        })
    });
    let sd_id = identity(sd).expect("sheepdog's identity");
    assert!(send(sd, sd_id, libc::SIGINT));
    std::thread::sleep(Duration::from_millis(900));
    // end the job by the root's exit, not by a signal to sheepdog: the shell would report a
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
    assert_ne!(sd, pg, "control: sheepdog must not lead its group here");
    assert_eq!(err.lines().filter(|l| !l.trim().is_empty()).count(), 1, "the hint must be printed once: {err:?}");
    assert!(targets(&err).is_empty(), "the hint named a group (the caller's is {pg}): {err:?}");
}

/// S4 review round 2 (A-P2-1): D9's own case, a harness in a terminal (a shell that handles INT
/// and leads the terminal's foreground group) runs sheepdog as a child and sends INT to its pid
/// only. sheepdog shares the foreground group, but it is not its own group, so the hint is
/// printed (on the terminal), and it names no group (the harness's).
#[test]
fn the_hint_reaches_a_harness_in_the_foreground() {
    let job = Job::new();
    let script = "trap : INT; \"$0\" run -- \"$@\"; exit 0";
    let mut args = vec!["SHEEPDOG_TEST_HINT_MS=300".to_string(), "/bin/sh".into(), "-c".into(), script.into(), sheepdog().into()];
    args.extend(job.args());
    let mut pty = Pty::leader("/usr/bin/env", &args);
    let sh = pty.child.id();
    job.ready();
    let (sd, sd_id) = wait_for("sheepdog under the harness", Duration::from_secs(15), || supervisor_of(sh));
    assert!(send(sd, sd_id, libc::SIGINT));
    // the hint is due at 300 ms: nothing before it (so the output is the hint, not some error)
    std::thread::sleep(Duration::from_millis(100));
    pty.drain();
    let early = String::from_utf8_lossy(&pty.out).to_string();
    std::thread::sleep(Duration::from_millis(800));
    pty.drain();
    let shown = String::from_utf8_lossy(&pty.out).to_string();
    send(sd, sd_id, libc::SIGTERM);
    assert!(early.trim().is_empty(), "output before the hint was due: {early:?}");
    assert!(!shown.trim().is_empty(), "no hint for a pid-only INT from a harness in the foreground");
    assert!(targets(&shown).is_empty(), "the hint named a group (the harness's is {sh}): {shown:?}");
}

/// S4 review round 2 (B-P2-1): the other side of the foreground rule. A background job in a
/// terminal (its own group, not the foreground one) that gets a pid-only INT prints the hint on
/// the terminal, naming its group.
#[test]
fn the_hint_is_printed_for_a_background_job() {
    let job = Job::new();
    let mut cmd = vec!["/usr/bin/env".to_string(), "SHEEPDOG_TEST_HINT_MS=300".into()];
    cmd.extend(run_args(&[], &job.args()));
    let mut pty = Pty::shell(&["bg"], &cmd);
    let (sd, sd_id) = pty.started();
    job.ready();
    assert!(send(sd, sd_id, libc::SIGINT));
    std::thread::sleep(Duration::from_millis(900));
    pty.drain();
    let shown = String::from_utf8_lossy(&pty.out).to_string();
    assert_eq!(targets(&shown), vec![sd as i64], "a background job must print the hint naming its group {sd}: {shown:?}");
}

/// Start `bg-then-exec` (or `bg-apart-then-exec`) with sheepdog running `job`, in a new process
/// group led by the relay, stderr piped, the hint due after 300 ms. Returns the relay.
fn relay_job(mode: &str, bg: &str, job: &Job) -> (Child, u64) {
    let mut c = Command::new(fixture());
    c.args([mode, bg]).args(run_args(&[], &job.args())).env("SHEEPDOG_TEST_HINT_MS", "300").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
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
/// sheepdog's own group, and a pid-only INT to the supervisor names it (not the supervisor's pid).
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
/// when the relay leads it. Here the caller's background job (started before sheepdog, with no
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
/// (`(job &)`) and then exec'd sheepdog, which therefore leads a group that holds the orphan.
#[test]
fn the_hint_names_no_group_that_holds_the_callers_orphan() {
    let job = Job::new();
    let bg = new_marker();
    let script = format!("(/bin/sleep {bg} >/dev/null 2>&1 &); exec \"$0\" run -- \"$@\"");
    let mut c = Command::new("/bin/sh");
    c.args(["-c", &script, sheepdog()]).args(job.args()).env("SHEEPDOG_TEST_HINT_MS", "300").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
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
    let id = identity(child.id() as i32).expect("sheepdog's identity");
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
    assert!(shared, "control: the orphan is not in sheepdog's group");
    assert_eq!(err.lines().filter(|l| !l.trim().is_empty()).count(), 1, "the hint must be printed once: {err:?}");
    assert!(targets(&err).is_empty(), "the hint named a group that holds the caller's orphan: {err:?}");
}

// ---- death by INT (PHASE1.md §1.3) ---------------------------------------------------------

/// ctrl-C ends a foreground job whose root dies of it (both arrive in one wake): sheepdog
/// dies of INT after the kill, as the root did, so the shell sees death by INT.
#[test]
fn ctrl_c_makes_sheepdog_die_of_int() {
    let m = new_marker();
    let mut pty = Pty::shell(&[], &run_args(&[], &["/bin/sleep".to_string(), m.clone()]));
    pty.started();
    wait_for("the root", Duration::from_secs(15), || (sleeps(&m).len() == 1).then_some(()));
    pty.write(b"\x03");
    let end = pty.outcome(Duration::from_secs(10));
    assert_eq!(end, Some(format!("signaled {}", libc::SIGINT)), "sheepdog must die of INT");
}

/// Control: a root that handles ctrl-C and exits 130 makes sheepdog exit 130, not die of INT
/// (death by INT only when the root died of the same signal).
#[test]
fn a_root_that_exits_130_on_ctrl_c_makes_sheepdog_exit_130() {
    let m = new_marker();
    let ready = std::env::temp_dir().join(format!("sd-s4-ready-{m}"));
    let _ = std::fs::remove_file(&ready);
    let mut pty = Pty::shell(&[], &run_args(&[], &[fixture().to_string(), "int-exit".into(), "130".into(), m.clone(), ready.display().to_string()]));
    pty.started();
    // the root's INT handler is installed (an INT before it would kill the root by default)
    wait_for("the root's handler", Duration::from_secs(15), || ready.exists().then_some(()));
    let _ = std::fs::remove_file(&ready);
    pty.write(b"\x03");
    let end = pty.outcome(Duration::from_secs(10));
    assert_eq!(end, Some("exited 130".to_string()), "sheepdog must exit as the root did");
}

/// A shell loop `while sheepdog run …; do :; done` stops on one ctrl-C: bash stops a loop only
/// when its child died of INT (wait-and-cooperative-exit).
#[test]
fn a_bash_loop_stops_on_one_ctrl_c() {
    let bash = ["/bin/bash", "/usr/bin/bash"].into_iter().find(|p| std::path::Path::new(p).exists()).expect("this cell needs bash");
    let m = new_marker();
    let script = "while :; do \"$0\" run -- /bin/sleep \"$1\"; /bin/sleep 0.1; done";
    let mut pty = Pty::shell(&[], &[bash.to_string(), "-c".into(), script.into(), sheepdog().into(), m.clone()]);
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
