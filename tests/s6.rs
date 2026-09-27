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

/// Run `sheepdog kill ARGS` with `env` (plus the signal log in the job's `.log`), bounded.
fn kill(j: &Job, args: &[&str], env: &[(&str, &str)]) -> Kill {
    let (out, err) = (j.file(".out"), j.file(".err"));
    let mut c = Command::new(sheepdog());
    c.arg("kill").args(args).env("SHEEPDOG_TEST_SIGNAL_LOG", j.file(".log"));
    for (k, v) in env {
        c.env(k, v);
    }
    let mut c = c
        .stdin(Stdio::null())
        .stdout(std::fs::File::create(&out).unwrap())
        .stderr(std::fs::File::create(&err).unwrap())
        .spawn()
        .unwrap();
    let st = wait_bounded(&mut c, Duration::from_secs(40)).expect("sheepdog kill did not end within 40 s");
    Kill {
        code: st.code(),
        out: std::fs::read_to_string(&out).unwrap_or_default(),
        err: std::fs::read_to_string(&err).unwrap_or_default(),
    }
}

const INERT: (&str, &str) = ("SHEEPDOG_TEST_INERT", "1");

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
    for (p, _) in j.recorded() {
        assert!(listed.contains(&p), "pid {p} of the tree is not listed: {}", k.out);
    }
}

/// Exit 0 when the target is already gone (the goal is met).
#[test]
fn s6_a_target_that_is_already_gone_exits_0() {
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
    let k = kill(j, &["--grace", "0", &pid.to_string()], &[INERT, ("SHEEPDOG_TEST_DEADLINE_MS", "300")]);
    assert_eq!(k.code, Some(1), "pid {pid} was not refused: stderr {}", k.err);
    assert_eq!(j.log(), Vec::<String>::new(), "signals were attempted for the refused pid {pid}");
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
/// holds sheepdog right after its first freeze and creates the ready file there.
#[test]
fn s6_an_interrupt_during_the_freeze_leaves_nothing_stopped() {
    for sig in [libc::SIGINT, libc::SIGTERM] {
        let j = Job::new();
        let mut c = spawn_fixture(&["deep", &j.marker, &j.rec(), "3"]);
        j.wait_recorded(3);
        let ready = j.file(".tick");
        let mut k = Command::new(sheepdog())
            .args(["kill", "--grace", "0", &c.id().to_string()])
            .env("SHEEPDOG_TEST_SLEEP_AFTER_FREEZE_MS", "1500")
            .env("SHEEPDOG_TEST_READY_FILE", &ready)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        wait_for("the freeze", Duration::from_secs(15), || ready.exists());
        assert!(send_child(&mut k, sig));
        let st = wait_bounded(&mut k, Duration::from_secs(20));
        let _ = wait_bounded(&mut c, Duration::from_secs(5));
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(j.alive(), vec![], "signal {sig}: members were left (stopped)");
        assert_eq!(st.and_then(|s| s.signal()), Some(sig), "signal {sig}: sheepdog did not die of it: {st:?}");
    }
}

