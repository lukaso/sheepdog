//! Phase 1, step S6 (PHASE1.md): `sheepdog kill <pid>`, the proved part. Target checks, the
//! proved set (the ppid closure; macOS also `puniq`), `--dry-run`, exit codes 0 / 1 / 2 / 125,
//! and a supervisor in the kill set ended first.
//!
//! Safety: every cell whose target is not a tree it built (pid 1, the test process, sheepdog
//! itself, another user's process, a pid that may have been reused) runs sheepdog with the debug
//! seam SHEEPDOG_TEST_INERT=1: sheepdog then sends no signal at all and logs each one it would
//! have sent. A control cell shows that the seam logs them. So a broken target check shows as a
//! logged signal, never as a signal. Readiness only; cleanup only by recorded identity or the
//! iteration's marker.

mod common;

use common::{found, scan, send, send_child};
use sheepdog::ident::same;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

static SEQ: AtomicUsize = AtomicUsize::new(0);

fn sheepdog() -> &'static str {
    env!("CARGO_BIN_EXE_sheepdog")
}
fn fixture() -> &'static str {
    env!("CARGO_BIN_EXE_sd-fixture")
}

/// One tree's marker and record file; dropping it kills what is left of the tree.
struct Job {
    marker: String,
    rec: PathBuf,
}

impl Job {
    fn new() -> Self {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let marker = format!("26.{}{:06}", std::process::id(), n);
        let rec = std::env::temp_dir().join(format!("sd-s6-{marker}"));
        for ext in ["", ".tick", ".root", ".log", ".out", ".err"] {
            let _ = std::fs::remove_file(format!("{}{ext}", rec.display()));
        }
        Job { marker, rec }
    }
    fn rec(&self) -> String {
        self.rec.display().to_string()
    }
    fn file(&self, ext: &str) -> PathBuf {
        PathBuf::from(format!("{}{ext}", self.rec()))
    }
    fn recorded(&self) -> Vec<(i32, u64)> {
        read_pairs(&self.rec)
    }
    /// Processes whose argv carries the marker, other than a sheepdog.
    fn marked(&self) -> Vec<(i32, u64)> {
        scan(&self.marker, |w| w.len() >= 2 && !w[1].ends_with("sheepdog")).expect("ps failed")
    }
    /// Recorded processes still alive, plus marked ones.
    fn alive(&self) -> Vec<(i32, u64)> {
        let mut v: Vec<(i32, u64)> = self.recorded().into_iter().filter(|&(p, id)| same(p, id)).collect();
        for p in self.marked() {
            if !v.iter().any(|q| q.0 == p.0) {
                v.push(p);
            }
        }
        v
    }
    fn wait_recorded(&self, n: usize) {
        wait_for(&format!("{n} recorded processes"), Duration::from_secs(15), || self.recorded().len() >= n);
    }
    /// The signal log sheepdog wrote (SHEEPDOG_TEST_SIGNAL_LOG), one entry per line.
    fn log(&self) -> Vec<String> {
        std::fs::read_to_string(self.file(".log")).unwrap_or_default().lines().map(String::from).collect()
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        for (p, id) in self.recorded().into_iter().chain(read_pairs(&self.file(".root"))) {
            send(p, id, libc::SIGKILL);
        }
        for (p, id) in self.marked() {
            send(p, id, libc::SIGKILL);
        }
        for ext in ["", ".tick", ".root", ".log", ".out", ".err"] {
            let _ = std::fs::remove_file(self.file(ext));
        }
    }
}

fn read_pairs(f: &PathBuf) -> Vec<(i32, u64)> {
    std::fs::read_to_string(f)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            Some((w.next()?.parse().ok()?, w.next()?.parse().ok()?))
        })
        .collect()
}

fn wait_for(what: &str, limit: Duration, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + limit;
    while !f() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Wait for a child we spawned, at most `limit`; SIGKILL it (while unreaped) if it overruns.
fn wait_bounded(c: &mut Child, limit: Duration) -> Option<ExitStatus> {
    let end = Instant::now() + limit;
    loop {
        if let Ok(Some(st)) = c.try_wait() {
            return Some(st);
        }
        if Instant::now() > end {
            send_child(c, libc::SIGKILL);
            let _ = c.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The result of one `sheepdog kill` run.
struct Kill {
    code: Option<i32>,
    out: String,
    err: String,
}

impl Kill {
    /// Every whole-word number in stdout's first column (the pids `--dry-run` lists).
    fn listed(&self) -> Vec<i32> {
        self.out.lines().filter_map(|l| l.split_whitespace().next()?.parse().ok()).collect()
    }
    /// Every number that appears as a word in stderr.
    fn err_numbers(&self) -> Vec<i32> {
        self.err.split(|c: char| !c.is_ascii_digit()).filter_map(|w| w.parse().ok()).collect()
    }
}

/// Start `sheepdog kill ARGS` with `env` (plus the signal log in the job's `.log`).
fn spawn_kill(j: &Job, args: &[&str], env: &[(&str, &str)]) -> Child {
    let mut c = Command::new(sheepdog());
    c.arg("kill").args(args).env("SHEEPDOG_TEST_SIGNAL_LOG", j.file(".log"));
    for (k, v) in env {
        c.env(k, v);
    }
    c.stdin(Stdio::null())
        .stdout(std::fs::File::create(j.file(".out")).unwrap())
        .stderr(std::fs::File::create(j.file(".err")).unwrap())
        .spawn()
        .unwrap()
}

/// Run `sheepdog kill ARGS` with `env`, bounded.
fn kill(j: &Job, args: &[&str], env: &[(&str, &str)]) -> Kill {
    finish(j, spawn_kill(j, args, env))
}

/// Wait (bounded) for a `sheepdog kill` started by `spawn_kill`.
fn finish(j: &Job, mut c: Child) -> Kill {
    let (out, err) = (j.file(".out"), j.file(".err"));
    let st = wait_bounded(&mut c, Duration::from_secs(40)).expect("sheepdog kill did not end within 40 s");
    Kill {
        code: st.code(),
        out: std::fs::read_to_string(&out).unwrap_or_default(),
        err: std::fs::read_to_string(&err).unwrap_or_default(),
    }
}

const INERT: (&str, &str) = ("SHEEPDOG_TEST_INERT", "1");

/// The inert seam exists only in debug builds; a cell aimed at a process it must never signal
/// refuses to run against a build without it.
fn assert_inert_seam_is_live() {
    assert!(cfg!(debug_assertions), "SHEEPDOG_TEST_INERT needs a debug build: refusing to aim sheepdog at processes it must not signal");
}

fn spawn_fixture(args: &[&str]) -> Child {
    Command::new(fixture()).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap()
}

/// Cell 27: a live tree 20 deep is killed whole.
#[test]
fn s6_kill_of_a_live_tree_20_deep_leaves_no_survivor() {
    let j = Job::new();
    let mut c = spawn_fixture(&["deep", &j.marker, &j.rec(), "20"]);
    j.wait_recorded(20);
    let k = kill(&j, &["--grace", "0", &c.id().to_string()], &[]);
    let _ = wait_bounded(&mut c, Duration::from_secs(5));
    assert_eq!(k.code, Some(0), "stderr: {}", k.err);
    assert_eq!(j.alive(), vec![], "survivors");
}

/// Cell 28(a): a grandchild in another session (`setsid`) whose parent lives is proved by the
/// ppid chain and killed.
#[test]
fn s6_a_setsid_grandchild_whose_parent_lives_is_killed() {
    let j = Job::new();
    let mut c = spawn_fixture(&["setsid-kid", &j.marker, &j.rec()]);
    j.wait_recorded(3);
    let k = kill(&j, &["--grace", "0", &c.id().to_string()], &[]);
    let _ = wait_bounded(&mut c, Duration::from_secs(5));
    assert_eq!(k.code, Some(0), "stderr: {}", k.err);
    assert_eq!(j.alive(), vec![], "survivors");
}

/// macOS `puniq` (PHASE1.md S6, amended cell 28(b)): a member that sheepdog has seen forks an
/// escapee in a new session on TERM and exits at once. The escapee's parent is gone before any
/// scan can see it as a child; its original parent's uniqueid, a member seen alive, proves it.
/// (Linux has no such fact: there the escapee survives, a stated limit.)
#[cfg(target_os = "macos")]
#[test]
fn s6_a_member_that_daemonizes_on_term_is_killed_by_its_puniq() {
    let j = Job::new();
    let mut c = spawn_fixture(&["fork-on-term", &j.marker, &j.rec()]);
    j.wait_recorded(2);
    let k = kill(&j, &["--grace", "1s", &c.id().to_string()], &[]);
    let _ = wait_bounded(&mut c, Duration::from_secs(5));
    assert_eq!(j.recorded().len(), 3, "control: the member did not daemonize on TERM");
    assert_eq!(k.code, Some(0), "stderr: {}", k.err);
    assert_eq!(j.alive(), vec![], "survivors");
}

/// `--dry-run` lists the proved set and signals nothing.
#[test]
fn s6_dry_run_lists_the_tree_and_signals_nothing() {
    let j = Job::new();
    let mut c = spawn_fixture(&["deep", &j.marker, &j.rec(), "5"]);
    j.wait_recorded(5);
    let k = kill(&j, &["--dry-run", &c.id().to_string()], &[]);
    let alive = j.alive().len();
    let log = j.log();
    let listed = k.listed();
    send_child(&mut c, libc::SIGKILL);
    let _ = c.wait();
    assert_eq!(k.code, Some(0), "stderr: {}", k.err);
    assert_eq!(log, Vec::<String>::new(), "a dry run sent signals");
    assert_eq!(alive, 5, "a dry run killed processes");
    let mut want: Vec<i32> = j.recorded().into_iter().map(|(p, _)| p).collect();
    want.sort();
    let mut listed = listed;
    listed.sort();
    assert_eq!(listed, want, "the dry run did not list exactly the tree: {}", k.out);
}

/// Exit 0 when the target is already gone (the goal is met).
#[test]
fn s6_a_target_that_is_already_gone_exits_0() {
    assert_inert_seam_is_live();
    let j = Job::new();
    let mut c = Command::new("/bin/sleep").arg("0").spawn().unwrap();
    let pid = c.id();
    c.wait().unwrap();
    let k = kill(&j, &["--grace", "0", &pid.to_string()], &[INERT]);
    assert_eq!(k.code, Some(0), "stderr: {}", k.err);
}

/// Usage errors exit 2 and signal nothing.
#[test]
fn s6_usage_errors_exit_2() {
    assert_inert_seam_is_live();
    let j = Job::new();
    for args in [&[][..], &["abc"], &["0"], &["-5"], &["--bogus", "123"], &["123", "456"], &["--grace"], &["--grace", "x", "123"]] {
        let k = kill(&j, args, &[INERT]);
        assert_eq!(k.code, Some(2), "kill {args:?}: stderr {}", k.err);
    }
    assert_eq!(j.log(), Vec::<String>::new(), "a usage error sent signals");
}

/// Exit 125 when members are alive at the deadline (a seam makes every scan non-empty).
#[test]
fn s6_a_missed_deadline_exits_125() {
    let j = Job::new();
    let mut c = spawn_fixture(&["deep", &j.marker, &j.rec(), "2"]);
    j.wait_recorded(2);
    let k = kill(&j, &["--grace", "0", &c.id().to_string()], &[("SHEEPDOG_TEST_NEVER_EMPTY", "1"), ("SHEEPDOG_TEST_DEADLINE_MS", "300")]);
    let _ = wait_bounded(&mut c, Duration::from_secs(5));
    assert_eq!(j.alive(), vec![], "control: the kill did not run (a usage error also exits 125)");
    assert_eq!(k.code, Some(125), "stderr: {}", k.err);
}

/// Control for the refusal cells: under SHEEPDOG_TEST_INERT sheepdog sends nothing, and logs
/// every signal it would have sent. So an empty log in a refusal cell means that no signal was
/// attempted.
#[test]
fn s6_control_the_inert_seam_logs_the_signals_it_withholds() {
    assert_inert_seam_is_live();
    let j = Job::new();
    let mut c = spawn_fixture(&["deep", &j.marker, &j.rec(), "3"]);
    j.wait_recorded(3);
    let k = kill(&j, &["--grace", "0", &c.id().to_string()], &[INERT, ("SHEEPDOG_TEST_DEADLINE_MS", "300")]);
    let alive = j.alive().len();
    let log = j.log();
    send_child(&mut c, libc::SIGKILL);
    let _ = c.wait();
    assert_eq!(alive, 3, "the inert seam let a signal through");
    let target = c.id().to_string();
    assert!(log.iter().any(|l| l.split_whitespace().any(|w| w == target)), "the inert seam logged no signal to the target: {log:?}");
    assert_eq!(k.code, Some(125), "stderr: {}", k.err);
}

/// A refused target: exit 1, and no signal attempted (the inert seam's log is empty).
fn refused(j: &Job, pid: i32) {
    let _ = refused_with(j, pid, &[]);
}

fn refused_with(j: &Job, pid: i32, env: &[(&str, &str)]) -> Kill {
    assert_inert_seam_is_live();
    let _ = std::fs::remove_file(j.file(".log"));
    let mut e = vec![INERT, ("SHEEPDOG_TEST_DEADLINE_MS", "300")];
    e.extend_from_slice(env);
    let k = kill(j, &["--grace", "0", &pid.to_string()], &e);
    assert_eq!(k.code, Some(1), "pid {pid} was not refused: stderr {}", k.err);
    assert_eq!(j.log(), Vec::<String>::new(), "signals were attempted for the refused pid {pid}");
    k
}

#[test]
fn s6_pid_1_is_refused() {
    refused(&Job::new(), 1);
}

/// sheepdog's parent (here the test process; in a shell, `sheepdog kill $$`) is an ancestor.
#[test]
fn s6_the_callers_process_is_refused() {
    refused(&Job::new(), std::process::id() as i32);
}

/// sheepdog itself (`exec sheepdog kill $$`).
#[test]
fn s6_sheepdog_itself_is_refused() {
    assert_inert_seam_is_live();
    let j = Job::new();
    let st = Command::new("/bin/sh")
        .args(["-c", "exec \"$0\" kill --grace 0 $$", sheepdog()])
        .env("SHEEPDOG_TEST_INERT", "1")
        .env("SHEEPDOG_TEST_DEADLINE_MS", "300")
        .env("SHEEPDOG_TEST_SIGNAL_LOG", j.file(".log"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|mut c| wait_bounded(&mut c, Duration::from_secs(40)))
        .unwrap()
        .expect("sheepdog kill did not end");
    assert_eq!(st.code(), Some(1));
    assert_eq!(j.log(), Vec::<String>::new(), "signals were attempted for sheepdog itself");
}

/// Another user's process: as root, a sleep started as `nobody`; otherwise any live process
/// of another uid.
#[test]
fn s6_another_users_process_is_refused() {
    let j = Job::new();
    let me = unsafe { libc::getuid() };
    if me == 0 {
        use std::os::unix::process::CommandExt;
        let mut c = Command::new("/bin/sleep").arg(&j.marker).uid(65534).spawn().unwrap();
        let (p, id) = found(c.id() as i32).unwrap();
        refused(&j, p);
        assert!(same(p, id), "the other user's process died");
        send_child(&mut c, libc::SIGKILL);
        let _ = c.wait();
    } else {
        let out = Command::new("ps").args(["-Ao", "pid=,uid="]).output().unwrap();
        let other = String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| {
                let mut w = l.split_whitespace();
                Some((w.next()?.parse::<i32>().ok()?, w.next()?.parse::<u32>().ok()?))
            })
            .find(|&(p, u)| p > 1 && u != me)
            .expect("no process of another user");
        let (p, id) = found(other.0).unwrap();
        refused(&j, p);
        assert!(same(p, id), "the other user's process died");
    }
}

/// Start `sheepdog run -- sd-fixture ticker M R` and wait until the job is complete: the root
/// has named itself and the escapee has its five ticking children (seven recorded: the root,
/// the escapee and its children).
fn ticker_job(j: &Job) -> Child {
    let c = Command::new(sheepdog())
        .args(["run", "--quiet", "--", fixture(), "ticker", &j.marker, &j.rec()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_for("the ticker root", Duration::from_secs(15), || j.file(".root").exists());
    j.wait_recorded(7);
    c
}

/// Every process of the job, including the root, is gone.
fn job_gone(j: &Job) -> Vec<(i32, u64)> {
    let mut v = j.alive();
    v.extend(read_pairs(&j.file(".root")).into_iter().filter(|&(p, id)| same(p, id)));
    v
}

/// Cell 29(f): `kill <outer pid>` with an inner `sheepdog run` in the tree. The inner job's
/// escapee (its own session, its parent gone) is not provable by `kill`; the inner supervisor,
/// ended first, kills it.
#[test]
fn s6_an_inner_supervisor_is_ended_first_so_its_escapees_die() {
    let j = Job::new();
    let mut outer = Command::new("/bin/sh")
        .args(["-c", "\"$0\" run --quiet -- \"$1\" ticker \"$2\" \"$3\"; exit 0", sheepdog(), fixture(), &j.marker, &j.rec()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_for("the ticker root", Duration::from_secs(15), || j.file(".root").exists());
    j.wait_recorded(7);
    let k = kill(&j, &["--grace", "0", &outer.id().to_string()], &[]);
    let _ = wait_bounded(&mut outer, Duration::from_secs(5));
    assert_eq!(k.code, Some(0), "stderr: {}", k.err);
    assert_eq!(job_gone(&j), vec![], "survivors of the inner job");
}

/// Cell 29(a): `kill <supervisor pid>` ends its whole job, the escapee included.
#[test]
fn s6_kill_of_a_supervisor_ends_its_whole_job() {
    let j = Job::new();
    let mut sup = ticker_job(&j);
    let k = kill(&j, &["--grace", "0", &sup.id().to_string()], &[]);
    let _ = wait_bounded(&mut sup, Duration::from_secs(5));
    assert_eq!(k.code, Some(0), "stderr: {}", k.err);
    assert_eq!(job_gone(&j), vec![], "survivors of the job");
}

/// §3.3 step 1: a stopped supervisor gets TERM and then CONT (it would never act on the TERM
/// while stopped), and so still ends its whole job.
#[test]
fn s6_a_stopped_supervisor_is_continued_after_its_term() {
    let j = Job::new();
    let mut sup = ticker_job(&j);
    assert!(send_child(&mut sup, libc::SIGSTOP));
    let k = kill(&j, &["--grace", "0", &sup.id().to_string()], &[("SHEEPDOG_TEST_DEADLINE_MS", "3000")]);
    let _ = wait_bounded(&mut sup, Duration::from_secs(5));
    assert_eq!(k.code, Some(0), "stderr: {}", k.err);
    assert_eq!(job_gone(&j), vec![], "survivors of the job");
}

/// A member of a running job (not the root): only its own subtree is killed, the job's root
/// and supervisor live on, and stderr names the supervisor's pid.
#[test]
fn s6_kill_of_a_member_kills_its_subtree_only_and_names_the_supervisor() {
    let j = Job::new();
    let mut sup = ticker_job(&j);
    // the escapee: the parent of a ticking child (the escapee itself does not tick)
    let tick: i32 = std::fs::read_to_string(j.file(".tick")).unwrap().lines().next().unwrap().trim().parse().unwrap();
    let out = Command::new("ps").args(["-o", "ppid=", "-p", &tick.to_string()]).output().unwrap();
    let g: i32 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap();
    let root = read_pairs(&j.file(".root"))[0];
    let subtree: Vec<(i32, u64)> = j.recorded().into_iter().filter(|&(p, _)| p != root.0).collect();
    assert!(subtree.iter().any(|&(p, _)| p == g), "control: the escapee {g} is not among the recorded");
    assert_eq!(subtree.len(), 6, "control: the escapee and its five children");
    let k = kill(&j, &["--grace", "0", &g.to_string()], &[]);
    let (root_alive, sup_running) = (same(root.0, root.1), matches!(sup.try_wait(), Ok(None)));
    let left: Vec<(i32, u64)> = subtree.into_iter().filter(|&(p, id)| same(p, id)).collect();
    send_child(&mut sup, libc::SIGTERM);
    let _ = wait_bounded(&mut sup, Duration::from_secs(10));
    assert_eq!(k.code, Some(0), "stderr: {}", k.err);
    assert_eq!(left, vec![], "the member's subtree survived");
    assert!(root_alive, "the job's root was killed");
    assert!(sup_running, "the supervisor ended");
    assert!(k.err_numbers().contains(&(sup.id() as i32)), "stderr does not name the supervisor {}: {}", sup.id(), k.err);
}

/// A ctrl-C (INT) or a TERM to `sheepdog kill` between its freeze and its SIGKILL must not leave
/// the tree stopped for good: the kill finishes, then sheepdog dies of the signal. A debug seam
/// holds sheepdog right after its first freeze (it creates the ready file there) until the test
/// creates the release file, after its signal.
#[test]
fn s6_an_interrupt_during_the_freeze_leaves_nothing_stopped() {
    for sig in [libc::SIGINT, libc::SIGTERM] {
        let j = Job::new();
        let mut c = spawn_fixture(&["deep", &j.marker, &j.rec(), "3"]);
        j.wait_recorded(3);
        let (ready, release) = (j.file(".tick"), j.file(".root"));
        let mut k = Command::new(sheepdog())
            .args(["kill", "--grace", "0", &c.id().to_string()])
            .env("SHEEPDOG_TEST_HOLD_AFTER_FREEZE", &release)
            .env("SHEEPDOG_TEST_READY_FILE", &ready)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        wait_for("the freeze", Duration::from_secs(15), || ready.exists());
        assert!(send_child(&mut k, sig));
        // the window closes only now: the signal is sent while sheepdog holds after its freeze
        std::fs::File::create(&release).unwrap();
        let st = wait_bounded(&mut k, Duration::from_secs(20));
        let _ = wait_bounded(&mut c, Duration::from_secs(5));
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(j.alive(), vec![], "signal {sig}: members were left (stopped)");
        assert_eq!(st.and_then(|s| s.signal()), Some(sig), "signal {sig}: sheepdog did not die of it: {st:?}");
    }
}

/// An inner supervisor whose caller ignored TERM does not end on the TERM that `kill` sends.
/// On macOS its escapees cannot be proved by `kill`, so `kill` must not claim a clean result:
/// exit 125, and stderr names the supervisor. On Linux its escapees are its adopted descendants,
/// so the kill after the wait proves and kills them all: exit 0.
#[test]
fn s6_a_supervisor_that_ignores_term_is_not_claimed_clean() {
    use std::os::unix::process::CommandExt;
    let j = Job::new();
    let mut c = Command::new(sheepdog());
    c.args(["run", "--quiet", "--", fixture(), "ticker", &j.marker, &j.rec()]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    unsafe {
        c.pre_exec(|| {
            libc::signal(libc::SIGTERM, libc::SIG_IGN);
            Ok(())
        });
    }
    let mut sup = c.spawn().unwrap();
    wait_for("the ticker root", Duration::from_secs(15), || j.file(".root").exists());
    j.wait_recorded(7);
    let k = kill(&j, &["--grace", "0", &sup.id().to_string()], &[("SHEEPDOG_TEST_DEADLINE_MS", "1000")]);
    let _ = wait_bounded(&mut sup, Duration::from_secs(5));
    if cfg!(target_os = "macos") {
        assert_eq!(k.code, Some(125), "stderr: {}", k.err);
        assert!(k.err_numbers().contains(&(sup.id() as i32)), "stderr does not name the supervisor {}: {}", sup.id(), k.err);
    } else {
        assert_eq!(k.code, Some(0), "stderr: {}", k.err);
        assert_eq!(job_gone(&j), vec![], "survivors of the job");
    }
}

/// The root of a running job is a target like any other, but its supervisor then ends the
/// whole job (PLAN.md §3.0): the job is gone, and stderr names the supervisor, as for any
/// member of a running job.
#[test]
fn s6_kill_of_a_jobs_root_ends_the_job_and_names_the_supervisor() {
    let j = Job::new();
    let mut sup = ticker_job(&j);
    let root = read_pairs(&j.file(".root"))[0];
    let k = kill(&j, &["--grace", "0", &root.0.to_string()], &[]);
    let st = wait_bounded(&mut sup, Duration::from_secs(10));
    assert_eq!(k.code, Some(0), "stderr: {}", k.err);
    assert!(st.is_some(), "the supervisor did not end");
    assert_eq!(job_gone(&j), vec![], "survivors of the job");
    // one line for every member of a running job, the root too: it names the supervisor
    assert!(k.err_numbers().contains(&(sup.id() as i32)), "stderr does not name the supervisor {}: {}", sup.id(), k.err);
}

/// While `kill` waits for an inner supervisor, the rest of the tree runs: a sibling that starts
/// a child in a new session and exits 300 ms later must still be proved (seen while its parent
/// lives) and killed. The inner job's member ignores TERM, so its supervisor takes its whole
/// grace (3 s); the sibling acts once `kill` has started that wait.
#[test]
fn s6_the_tree_is_scanned_while_a_supervisor_is_awaited() {
    let j = Job::new();
    let go = j.file(".tick");
    let mut outer = Command::new("/bin/sh")
        .args([
            "-c",
            "\"$0\" run --quiet --grace 3s -- \"$1\" fork-on-term \"$2\" \"$3\" & \"$1\" linger-on \"$2\" \"$3\" \"$4\"; wait",
            sheepdog(),
            fixture(),
            &j.marker,
            &j.rec(),
            go.to_str().unwrap(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    j.wait_recorded(3);
    let k = spawn_kill(&j, &["--grace", "0", &outer.id().to_string()], &[]);
    wait_for("the supervisor wait", Duration::from_secs(15), || j.log().iter().any(|l| l.starts_with("supervisor ")));
    std::fs::File::create(&go).unwrap();
    let k = finish(&j, k);
    let _ = wait_bounded(&mut outer, Duration::from_secs(5));
    assert!(j.recorded().len() >= 5, "control: the sibling's child was not created: {:?}", j.recorded());
    assert_eq!(k.code, Some(0), "stderr: {}", k.err);
    assert_eq!(j.alive(), vec![], "survivors");
}

/// The parent of `pid`, from `ps` (which reads any uid's parent).
fn ps_parent(pid: i32) -> Option<i32> {
    let out = Command::new("ps").args(["-o", "ppid=", "-p", &pid.to_string()]).output().ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// Every ancestor of sheepdog (the test process, its parent, and so on up to pid 1, whatever
/// their uid: in a macOS terminal the chain passes through a root-owned `login`, and the
/// terminal's own processes above it are the caller's again) is refused with no signal.
#[test]
fn s6_every_ancestor_of_sheepdog_is_refused() {
    let j = Job::new();
    let mut p = std::process::id() as i32;
    let mut n = 0;
    while p > 1 && n < 64 {
        refused(&j, p);
        p = ps_parent(p).expect("ps could not read a parent");
        n += 1;
    }
}

/// Fail closed: when sheepdog cannot read part of its own chain of ancestors (a debug seam makes
/// the test process's parent unreadable), a process above that point may still be an ancestor,
/// so it is refused.
#[test]
fn s6_an_unreadable_ancestor_chain_refuses_the_kill() {
    let j = Job::new();
    let me = std::process::id().to_string();
    let grandparent = ps_parent(std::process::id() as i32).expect("no parent");
    assert!(grandparent > 1, "control: the test process needs a parent above pid 1");
    let k = refused_with(&j, grandparent, &[("SHEEPDOG_TEST_PARENT_UNREADABLE", &me)]);
    // the refusal names the process whose parent could not be read (the operator's lead)
    assert!(k.err_numbers().contains(&(std::process::id() as i32)), "the refusal does not name the unreadable link: {}", k.err);
}

/// A TERM or INT during the TERM grace (nothing is frozen yet) ends `kill` at once, and leaves
/// no member stopped. The members ignore TERM, so the grace would otherwise last 30 s.
#[test]
fn s6_a_signal_during_the_grace_ends_kill_at_once() {
    use std::os::unix::process::CommandExt;
    for sig in [libc::SIGTERM, libc::SIGINT] {
        let j = Job::new();
        let mut c = Command::new(fixture());
        c.args(["deep", &j.marker, &j.rec(), "3"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        unsafe {
            c.pre_exec(|| {
                libc::signal(libc::SIGTERM, libc::SIG_IGN);
                Ok(())
            });
        }
        let mut c = c.spawn().unwrap();
        j.wait_recorded(3);
        let target = c.id().to_string();
        let mut k = spawn_kill(&j, &["--grace", "30s", &target], &[]);
        // the grace has begun once the target's TERM is logged
        wait_for("the grace", Duration::from_secs(15), || {
            j.log().iter().any(|l| {
                let w: Vec<&str> = l.split_whitespace().collect();
                w.len() == 3 && w[1] == target && w[2] == libc::SIGTERM.to_string()
            })
        });
        assert!(send_child(&mut k, sig));
        let t0 = Instant::now();
        let st = wait_bounded(&mut k, Duration::from_secs(20));
        let took = t0.elapsed();
        let stopped: Vec<i32> = j.recorded().iter().filter(|&&(p, id)| same(p, id) && ps_stat(p).starts_with('T')).map(|&(p, _)| p).collect();
        send_child(&mut c, libc::SIGKILL);
        let _ = c.wait();
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(st.and_then(|s| s.signal()), Some(sig), "signal {sig}: sheepdog did not die of it: {st:?}");
        assert!(took < Duration::from_secs(2), "signal {sig}: sheepdog took {took:?} to act on it");
        assert_eq!(stopped, Vec::<i32>::new(), "signal {sig}: members were left stopped");
    }
}

fn ps_stat(pid: i32) -> String {
    Command::new("ps").args(["-o", "stat=", "-p", &pid.to_string()]).output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
}

/// The relay path (`job & exec sheepdog run -- cmd`): the relay's older child, the background
/// job, is not part of the running job: stderr names the relay (a `sheepdog` whose kill ends
/// the job), and only the background job dies. Killing the real root then ends the job.
#[test]
fn s6_on_the_relay_path_a_background_job_is_killed_alone() {
    let j = Job::new();
    let mut relay = Command::new(fixture())
        .args(["bg-then-exec", &j.marker, sheepdog(), "run", "--quiet", "--", fixture(), "ticker", &j.marker, &j.rec()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_for("the ticker root", Duration::from_secs(15), || j.file(".root").exists());
    j.wait_recorded(7);
    let bg = scan(&j.marker, |w| w.len() >= 2 && w[1] == "/bin/sleep").expect("ps failed");
    assert_eq!(bg.len(), 1, "control: one background job: {bg:?}");
    let (bgp, bgid) = bg[0];
    let root = read_pairs(&j.file(".root"))[0];
    let k = kill(&j, &["--grace", "0", &bgp.to_string()], &[]);
    let (bg_alive, root_alive) = (same(bgp, bgid), same(root.0, root.1));
    let k2 = kill(&j, &["--grace", "0", &root.0.to_string()], &[]);
    let st = wait_bounded(&mut relay, Duration::from_secs(10));
    assert_eq!(k.code, Some(0), "stderr: {}", k.err);
    assert!(!bg_alive, "the background job survived");
    assert!(root_alive, "the job's root was killed with the background job");
    assert!(k.err_numbers().contains(&(relay.id() as i32)), "the background job did not get the member line naming the relay {}: {}", relay.id(), k.err);
    assert_eq!(k2.code, Some(0), "stderr: {}", k2.err);
    assert!(st.is_some(), "the job did not end with its root");
    assert_eq!(job_gone(&j), vec![], "survivors of the job");
}
