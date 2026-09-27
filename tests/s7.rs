//! Phase 1, step S7 (PHASE1.md): the escape cells of PLAN.md §6 not built by earlier steps
//! (1, 2, 4-12, 16) and the no-setup safety cells of `sheepdog kill`.
//!
//! Each escape cell has two halves: under sheepdog the route leaves 0 survivors; the control
//! runs the same shape with the naive method PLAN.md names (kill of the root only, a group kill,
//! TERM only, an env-tag sweep, waiting for the root only) and must leak. The checker reads the
//! identities the processes recorded, plus the iteration's marker in argv; it acts after the
//! root has exited and been reaped. Cleanup only by recorded identity or marker.

mod common;

use common::{found, scan, send, send_child, send_group};
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

/// One iteration's marker and record file; dropping it kills what is left.
struct Job {
    marker: String,
    rec: PathBuf,
}

impl Job {
    fn new() -> Self {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let marker = format!("27.{}{:06}", std::process::id(), n);
        let rec = std::env::temp_dir().join(format!("sd-s7-{marker}"));
        let j = Job { marker, rec };
        for ext in EXTS {
            let _ = std::fs::remove_file(j.file(ext));
        }
        j
    }
    fn rec(&self) -> String {
        self.rec.display().to_string()
    }
    fn file(&self, ext: &str) -> PathBuf {
        PathBuf::from(format!("{}{ext}", self.rec()))
    }
    fn recorded(&self) -> Vec<(i32, u64)> {
        let mut v = read_pairs(&self.rec);
        v.extend(read_pairs(&self.file(".b")));
        v.extend(read_pairs(&self.file(".c")));
        v
    }
    /// Live processes whose argv carries the marker, other than a sheepdog.
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
        v.sort();
        v.dedup();
        v
    }
    fn wait_recorded(&self, n: usize) {
        wait_for(&format!("{n} recorded processes"), Duration::from_secs(15), || {
            let mut v = self.recorded();
            v.sort();
            v.dedup();
            v.len() >= n
        });
    }
    /// Wait for `/bin/sleep <marker>`, the process at the end of a route (a shell or `script`
    /// that carries the marker in its argv exists before it).
    fn wait_sleep(&self) {
        wait_for("the route's /bin/sleep", Duration::from_secs(15), || {
            !scan(&self.marker, |w| w.len() >= 2 && w[1] == "/bin/sleep").expect("ps failed").is_empty()
        });
    }
    fn wait_marked(&self, n: usize) {
        wait_for(&format!("{n} marked processes"), Duration::from_secs(15), || self.marked().len() >= n);
    }
    /// sheepdog's signal log, one entry per line.
    fn log(&self) -> Vec<String> {
        std::fs::read_to_string(self.file(".log")).unwrap_or_default().lines().map(String::from).collect()
    }
}

const EXTS: [&str; 8] = ["", ".b", ".c", ".root", ".tick", ".log", ".go", ".err"];

impl Drop for Job {
    fn drop(&mut self) {
        for (p, id) in self.recorded().into_iter().chain(read_pairs(&self.file(".root"))) {
            send(p, id, libc::SIGKILL);
        }
        for (p, id) in self.marked() {
            send(p, id, libc::SIGKILL);
        }
        for ext in EXTS {
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

fn quiet(c: &mut Command) -> &mut Command {
    c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
}

/// `sheepdog run -- ARGS`, started.
fn under_sheepdog(args: &[&str]) -> Child {
    quiet(Command::new(sheepdog()).args(["run", "--quiet", "--"]).args(args)).spawn().unwrap()
}

/// `sheepdog run -- ARGS`, run to its end (bounded); the root ends by itself.
fn run_under_sheepdog(args: &[&str]) -> ExitStatus {
    let mut c = under_sheepdog(args);
    wait_bounded(&mut c, Duration::from_secs(30)).expect("sheepdog did not end within 30 s")
}

/// ARGS without sheepdog, run to the end of its first process (bounded).
fn run_plain(args: &[&str]) -> ExitStatus {
    let mut c = quiet(Command::new(args[0]).args(&args[1..])).spawn().unwrap();
    wait_bounded(&mut c, Duration::from_secs(30)).expect("the shape did not end within 30 s")
}

/// End a job by TERM to sheepdog, and wait for it.
fn term_and_wait(sd: &mut Child) {
    assert!(send_child(sd, libc::SIGTERM));
    wait_bounded(sd, Duration::from_secs(30)).expect("sheepdog did not end within 30 s after TERM");
}

fn bash() -> &'static str {
    "bash"
}

// ---------------------------------------------------------------------------------------------
// Escape cells

/// Cell 1: a deep tree (depth 50), ended by TERM to sheepdog: 0 survivors.
#[test]
fn cell1_a_deep_tree_leaves_no_survivor() {
    let j = Job::new();
    let mut sd = under_sheepdog(&[fixture(), "deep", &j.marker, &j.rec(), "50"]);
    j.wait_recorded(50);
    term_and_wait(&mut sd);
    assert_eq!(j.alive(), vec![], "survivors");
}

/// Cell 1 control: a kill of the root only leaves the other 49 alive.
#[test]
fn cell1_control_a_kill_of_the_root_only_leaks() {
    let j = Job::new();
    let mut c = quiet(Command::new(fixture()).args(["deep", &j.marker, &j.rec(), "50"])).spawn().unwrap();
    j.wait_recorded(50);
    assert!(send_child(&mut c, libc::SIGKILL));
    let _ = c.wait();
    assert_eq!(j.alive().len(), 49, "control: the rest of the tree should have leaked");
}

/// Cell 2: a grandchild calls setsid (its parent lives): 0 survivors.
#[test]
fn cell2_a_setsid_grandchild_leaves_no_survivor() {
    let j = Job::new();
    let mut sd = under_sheepdog(&[fixture(), "setsid-kid", &j.marker, &j.rec()]);
    j.wait_recorded(3);
    term_and_wait(&mut sd);
    assert_eq!(j.alive(), vec![], "survivors");
}

/// Cell 2 control: a group kill of the root's group misses the grandchild's new session (and
/// its parent, which made it).
#[test]
fn cell2_control_a_group_kill_leaks() {
    use std::os::unix::process::CommandExt;
    let j = Job::new();
    let mut c = quiet(Command::new(fixture()).args(["setsid-kid", &j.marker, &j.rec()]).process_group(0)).spawn().unwrap();
    j.wait_recorded(3);
    let (pg, id) = found(c.id() as i32).unwrap();
    assert!(send_group(pg, id, libc::SIGKILL));
    let _ = c.wait();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(j.alive().len(), 2, "control: the new session should have leaked");
}

/// Cell 4: `nohup ... & disown`, then the shell exits: 0 survivors.
#[test]
fn cell4_nohup_and_disown_leave_no_survivor() {
    let j = Job::new();
    let script = format!("nohup /bin/sleep {} >/dev/null 2>&1 & disown; exit 0", j.marker);
    run_under_sheepdog(&[bash(), "-c", &script]);
    assert_eq!(j.alive(), vec![], "survivors");
}

/// Cell 4 control: the shell's exit leaves the disowned job alive.
#[test]
fn cell4_control_the_shells_exit_leaks() {
    let j = Job::new();
    let script = format!("nohup /bin/sleep {} >/dev/null 2>&1 & disown; exit 0", j.marker);
    run_plain(&[bash(), "-c", &script]);
    j.wait_marked(1);
    assert_eq!(j.alive().len(), 1, "control: the disowned job should have leaked");
}

fn ignores_term(marker: &str) -> String {
    format!("trap '' TERM; /bin/sleep {marker} & exit 0")
}

/// Cell 5: a child that ignores TERM: 0 survivors (the grace ends in SIGKILL).
#[test]
fn cell5_a_child_that_ignores_term_leaves_no_survivor() {
    let j = Job::new();
    let mut sd = quiet(Command::new(sheepdog()).args(["run", "--quiet", "--grace", "200ms", "--", "/bin/sh", "-c", &ignores_term(&j.marker)])).spawn().unwrap();
    wait_bounded(&mut sd, Duration::from_secs(30)).expect("sheepdog did not end");
    assert_eq!(j.alive(), vec![], "survivors");
}

/// Cell 5 control: TERM only leaves it alive.
#[test]
fn cell5_control_term_only_leaks() {
    let j = Job::new();
    run_plain(&["/bin/sh", "-c", &ignores_term(&j.marker)]);
    j.wait_marked(1);
    for (p, id) in j.marked() {
        send(p, id, libc::SIGTERM);
    }
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(j.alive().len(), 1, "control: the TERM-ignoring child should have leaked");
}

// Cell 6 (a stopped child whose parent dies) needs a real session, so it lives in tests/pty.rs:
// outside one, the orphan's group is orphaned and the kernel HUPs and CONTs it.

/// Live (non-zombie) processes whose parent is `ppid`, from /proc.
#[cfg(target_os = "linux")]
fn children_of(ppid: i32) -> usize {
    std::fs::read_dir("/proc")
        .unwrap()
        .flatten()
        .filter_map(|e| std::fs::read_to_string(e.path().join("stat")).ok())
        .filter(|s| {
            let Some(rest) = s.rfind(')').and_then(|i| s.get(i + 2..)) else { return false };
            let f: Vec<&str> = rest.split_whitespace().collect();
            f.first() != Some(&"Z") && f.get(1).and_then(|p| p.parse::<i32>().ok()) == Some(ppid)
        })
        .count()
}

/// Zombies (state Z) whose parent is `ppid`, from /proc.
#[cfg(target_os = "linux")]
fn zombies_of(ppid: i32) -> usize {
    std::fs::read_dir("/proc")
        .unwrap()
        .flatten()
        .filter_map(|e| std::fs::read_to_string(e.path().join("stat")).ok())
        .filter(|s| {
            let Some(rest) = s.rfind(')').and_then(|i| s.get(i + 2..)) else { return false };
            let f: Vec<&str> = rest.split_whitespace().collect();
            f.first() == Some(&"Z") && f.get(1).and_then(|p| p.parse::<i32>().ok()) == Some(ppid)
        })
        .count()
}

/// Cell 7 (zombies, Linux): a 3 s storm of orphans (200/s, each adopted by the subreaper and
/// exiting 20 ms later) leaves no zombie of the supervisor: it reaps what it adopts. A load
/// generator, brief (3 s, about 600 short processes).
#[cfg(target_os = "linux")]
#[test]
fn cell7_a_fork_storm_leaves_no_zombie_of_the_supervisor() {
    let j = Job::new();
    let done = j.file(".go");
    let mut sd = under_sheepdog(&[fixture(), "storm", done.to_str().unwrap(), "3"]);
    let sup = sd.id() as i32;
    // during the storm, count the supervisor's children beyond its root: the adopted orphans
    // (without adoption the zero-zombie count below would say nothing)
    let mut adopted = 0;
    wait_for("the storm", Duration::from_secs(30), || {
        adopted = adopted.max(children_of(sup).saturating_sub(1));
        done.exists()
    });
    std::thread::sleep(Duration::from_millis(200)); // the last orphans (20 ms) have exited
    let z = zombies_of(sup);
    term_and_wait(&mut sd);
    assert!(adopted > 0, "control: the supervisor adopted no orphan during the storm");
    assert_eq!(z, 0, "zombies of the supervisor after the storm");
}

/// An env-tag sweep (the Jenkins ProcessTreeKiller method): every process of this user whose
/// environment shows SD_TAG=`tag` (macOS: `ps eww`, which cannot show the environment of an
/// Apple platform binary; Linux: /proc/<pid>/environ).
fn tag_sweep(tag: &str) -> Vec<(i32, u64)> {
    let want = format!("SD_TAG={tag}");
    let mut v = Vec::new();
    if cfg!(target_os = "macos") {
        // BSD `e` (no dash) appends the environment; `-e` does not
        let out = Command::new("ps").args(["eww", "-A", "-o", "pid=,command="]).output().unwrap();
        for l in String::from_utf8_lossy(&out.stdout).lines() {
            if l.split_whitespace().any(|w| w == want) {
                if let Some(p) = l.split_whitespace().next().and_then(|p| p.parse().ok()).and_then(found) {
                    v.push(p);
                }
            }
        }
    } else {
        for e in std::fs::read_dir("/proc").unwrap().flatten() {
            let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<i32>().ok()) else { continue };
            if let Ok(env) = std::fs::read(e.path().join("environ")) {
                if env.split(|&c| c == 0).any(|w| w == want.as_bytes()) {
                    if let Some(p) = found(pid) {
                        v.push(p);
                    }
                }
            }
        }
    }
    v
}

/// The control of an env-tag sweep: the escape shape runs with SD_TAG set, next to a tagged
/// witness (an sd-fixture, whose environment the sweep can read). The sweep kills what it
/// finds: it must find the witness (it works), and must miss the escapee (`leaks`) or find it
/// (the paired control). `ready` says when the escapee runs its final program.
fn tag_sweep_control(j: &Job, escape: &[&str], ready: impl Fn(i32) -> bool, leaks: bool) {
    let mut witness = quiet(Command::new(fixture()).args(["deep", &j.marker, j.file(".b").to_str().unwrap(), "2"]).env("SD_TAG", &j.marker)).spawn().unwrap();
    let mut root = quiet(Command::new(fixture()).args(escape).env("SD_TAG", &j.marker)).spawn().unwrap();
    wait_bounded(&mut root, Duration::from_secs(15)).expect("the escape root did not exit");
    let g = read_pairs(&j.rec).first().copied().expect("the escapee was not recorded");
    wait_for("the escapee's final program", Duration::from_secs(15), || ready(g.0));
    let w = read_pairs(&j.file(".b")).first().copied().expect("the witness was not recorded");
    let swept = tag_sweep(&j.marker);
    for &(p, id) in &swept {
        send(p, id, libc::SIGKILL);
    }
    let _ = wait_bounded(&mut witness, Duration::from_secs(5));
    assert!(swept.iter().any(|&(p, _)| p == w.0), "control: the sweep did not find the tagged witness {w:?}: {swept:?}");
    if leaks {
        assert!(same(g.0, g.1), "control: the sweep found the escapee, so the route did not leak");
    } else {
        assert!(swept.iter().any(|&(p, _)| p == g.0), "paired control: the sweep missed the escapee {g:?} although its environment is intact");
    }
}

/// `pid` runs the program `prog` (its argv[0]).
fn runs(pid: i32, prog: &str) -> bool {
    let out = Command::new("ps").args(["-o", "args=", "-p", &pid.to_string()]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).split_whitespace().next() == Some(prog)
}

/// The escaped loop: it writes `ready` once `/bin/bash` runs it, then loops 30 s at most.
fn bash_loop(ready: &std::path::Path) -> String {
    format!("echo run > '{}'; for i in $(seq 300); do /bin/sleep 0.1; done", ready.display())
}

/// Cell 8 (macOS): an escaped `/bin/bash` loop: 0 survivors.
#[cfg(target_os = "macos")]
#[test]
fn cell8_an_escaped_bash_loop_leaves_no_survivor() {
    let j = Job::new();
    let ready = j.file(".go");
    let script = bash_loop(&ready);
    let mut sd = quiet(Command::new(sheepdog()).args(["run", "--quiet", "--", fixture(), "escape-exec", &j.rec(), "/bin/bash", "-c", &script, &j.marker]).env("SD_EXEC_READY", &ready)).spawn().unwrap();
    wait_bounded(&mut sd, Duration::from_secs(30)).expect("sheepdog did not end");
    assert!(ready.exists(), "control: the route did not reach /bin/bash");
    assert_eq!(j.alive(), vec![], "survivors");
}

/// Cell 8 control (macOS): an env-tag sweep cannot read an Apple binary's environment.
#[cfg(target_os = "macos")]
#[test]
fn cell8_control_an_env_tag_sweep_misses_an_apple_binary() {
    let j = Job::new();
    let script = bash_loop(&j.file(".go"));
    tag_sweep_control(&j, &["escape-exec", &j.rec(), "/bin/bash", "-c", &script, &j.marker], |g| runs(g, "/bin/bash"), true);
}

/// The cell-9 escapee's program after `env [-i]`: the fixture (not an Apple binary, so on macOS
/// `ps eww` could read its environment), which records itself in `.c` once it runs.
fn env_route(j: &Job, drop_env: bool) -> Vec<String> {
    let mut v = vec!["escape-exec".to_string(), j.rec(), "/usr/bin/env".to_string()];
    if drop_env {
        v.push("-i".into());
    }
    v.extend([fixture().to_string(), "deep".into(), j.marker.clone(), j.file(".c").display().to_string(), "2".into()]);
    v
}

/// Cell 9: an escapee that runs `env -i` (it drops every variable): 0 survivors.
#[test]
fn cell9_an_env_i_escapee_leaves_no_survivor() {
    let j = Job::new();
    let r = env_route(&j, true);
    // the root exits only once the program after `env -i` runs (on Linux sheepdog would
    // otherwise kill the escapee before it gets there)
    let mut sd = quiet(Command::new(sheepdog()).args(["run", "--quiet", "--", fixture()]).args(&r).env("SD_EXEC_READY", j.file(".c"))).spawn().unwrap();
    wait_bounded(&mut sd, Duration::from_secs(30)).expect("sheepdog did not end");
    assert!(!read_pairs(&j.file(".c")).is_empty(), "control: the route did not reach the program after `env -i`");
    assert_eq!(j.alive(), vec![], "survivors");
}

/// Cell 9 control: an env-tag sweep misses a process that dropped its environment.
#[test]
fn cell9_control_an_env_tag_sweep_misses_env_i() {
    let j = Job::new();
    let r = env_route(&j, true);
    let r: Vec<&str> = r.iter().map(String::as_str).collect();
    tag_sweep_control(&j, &r, |_| !read_pairs(&j.file(".c")).is_empty(), true);
}

/// Cell 9 paired control: the same program without `-i` is found by the sweep (so the leak above
/// is `env -i`'s doing, not a blind spot of the sweep).
#[test]
fn cell9_control_without_env_i_the_sweep_finds_it() {
    let j = Job::new();
    let r = env_route(&j, false);
    let r: Vec<&str> = r.iter().map(String::as_str).collect();
    tag_sweep_control(&j, &r, |_| !read_pairs(&j.file(".c")).is_empty(), false);
}

/// `script` running CMD in a new session on a pty (the syntax differs by OS; Alpine needs the
/// util-linux-misc package).
fn script_args(cmd: &str) -> Vec<String> {
    if cfg!(target_os = "macos") {
        vec!["script".into(), "-q".into(), "/dev/null".into(), "/bin/sh".into(), "-c".into(), cmd.into()]
    } else {
        vec!["script".into(), "-q".into(), "-c".into(), cmd.into(), "/dev/null".into()]
    }
}

fn hup_proof(marker: &str) -> String {
    format!("trap '' HUP; exec /bin/sleep {marker}")
}

/// Cell 10: a pty via `script` (its child is in a session of its own): 0 survivors.
#[test]
fn cell10_a_script_pty_leaves_no_survivor() {
    let j = Job::new();
    let a = script_args(&hup_proof(&j.marker));
    let a: Vec<&str> = a.iter().map(String::as_str).collect();
    let mut sd = under_sheepdog(&a);
    j.wait_sleep();
    term_and_wait(&mut sd);
    assert_eq!(j.alive(), vec![], "survivors");
}

/// Cell 10 control: a group kill of `script`'s group misses its child's session.
#[test]
fn cell10_control_a_group_kill_leaks() {
    use std::os::unix::process::CommandExt;
    let j = Job::new();
    let a = script_args(&hup_proof(&j.marker));
    let mut c = quiet(Command::new(&a[0]).args(&a[1..]).process_group(0)).spawn().unwrap();
    j.wait_sleep();
    let (pg, id) = found(c.id() as i32).unwrap();
    assert!(send_group(pg, id, libc::SIGKILL));
    let _ = c.wait();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(j.alive().len(), 1, "control: the pty child should have leaked");
}

fn set_m(marker: &str) -> Vec<String> {
    vec![bash().into(), "-c".into(), "set -m; \"$@\"".into(), "_".into(), "/bin/sh".into(), "-c".into(), format!("/bin/sleep {marker} & wait")]
}

/// Cell 11 (#332): `bash -c 'set -m; "$@"'` execs the last command, so no group is made: 0
/// survivors.
#[test]
fn cell11_set_m_leaves_no_survivor() {
    let j = Job::new();
    let a = set_m(&j.marker);
    let a: Vec<&str> = a.iter().map(String::as_str).collect();
    let mut sd = under_sheepdog(&a);
    j.wait_sleep();
    term_and_wait(&mut sd);
    assert_eq!(j.alive(), vec![], "survivors");
}

/// Cell 11 control: the group a `kill -<pid>` expects was never made (the #332 hang). The cell
/// reads the group (no pgid equal to the pid) instead of sending `kill -<pid>`: a group it did
/// not create is never signalled.
#[test]
fn cell11_control_no_group_is_made_for_kill_pgid() {
    let j = Job::new();
    let a = set_m(&j.marker);
    let mut c = quiet(Command::new(&a[0]).args(&a[1..])).spawn().unwrap();
    j.wait_sleep();
    let pid = c.id() as i32;
    let pg = unsafe { libc::getpgid(pid) };
    send_child(&mut c, libc::SIGKILL);
    let _ = c.wait();
    assert_ne!(pg, pid, "control: bash made a group, so `kill -{pid}` would reach it");
    // the job leaks; with macOS's bash 3.2 `set -m` does not exec the last command, so the shell
    // it runs leaks with it
    assert!(!j.alive().is_empty(), "control: the job should have leaked");
}

fn stray(marker: &str) -> String {
    format!("/bin/sleep {marker} & exit 0")
}

/// Cell 12: a clean root exit that leaves a stray: 0 survivors.
#[test]
fn cell12_a_clean_exit_with_a_stray_leaves_no_survivor() {
    let j = Job::new();
    let st = run_under_sheepdog(&["/bin/sh", "-c", &stray(&j.marker)]);
    assert_eq!(st.code(), Some(0));
    assert_eq!(j.alive(), vec![], "survivors");
}

/// Cell 12 control: waiting for the root only leaves the stray.
#[test]
fn cell12_control_waiting_for_the_root_leaks() {
    let j = Job::new();
    run_plain(&["/bin/sh", "-c", &stray(&j.marker)]);
    j.wait_marked(1);
    assert_eq!(j.alive().len(), 1, "control: the stray should have leaked");
}

/// The 2026-09-25 shape (a ccwho mutant): the harness runs a job in a subshell, `kill $P` hits
/// the subshell, and the job it started is orphaned. The subshell creates GO once the job runs.
fn harness(j: &Job) -> String {
    let go = j.file(".go");
    format!("( /bin/sleep {m} & : > {go}; wait ) & P=$!; while [ ! -e {go} ]; do /bin/sleep 0.01; done; kill $P; wait $P; exit 0", m = j.marker, go = go.display())
}

/// Cell 16: the 2026-09-25 harness shape inside sheepdog: 0 survivors.
#[test]
fn cell16_the_2026_09_25_shape_leaves_no_survivor() {
    let j = Job::new();
    run_under_sheepdog(&["/bin/sh", "-c", &harness(&j)]);
    assert!(j.file(".go").exists(), "control: the job never started");
    assert_eq!(j.alive(), vec![], "survivors");
}

/// Cell 16 control: the same shape without sheepdog leaks the orphan.
#[test]
fn cell16_control_without_sheepdog_leaks() {
    let j = Job::new();
    run_plain(&["/bin/sh", "-c", &harness(&j)]);
    assert_eq!(j.alive().len(), 1, "control: the orphan should have leaked");
}

// ---------------------------------------------------------------------------------------------
// No-setup safety cells: `sheepdog kill` never reaches wider than the target's proved tree.

/// Run `sheepdog kill --grace 0 PID` (bounded), with the signal log in the job's `.log`.
fn kill(j: &Job, pid: i32) -> Option<i32> {
    let mut c = Command::new(sheepdog())
        .args(["kill", "--grace", "0", &pid.to_string()])
        .env("SHEEPDOG_TEST_SIGNAL_LOG", j.file(".log"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(j.file(".err")).unwrap())
        .spawn()
        .unwrap();
    wait_bounded(&mut c, Duration::from_secs(40)).expect("sheepdog kill did not end").code()
}

/// Pids sheepdog sent a signal to, from its log (lines "<door> <pid> <sig>").
fn signalled(j: &Job) -> Vec<i32> {
    j.log().iter().filter_map(|l| l.split_whitespace().nth(1)?.parse().ok()).collect()
}

/// A sibling command (another child of the same parent) is not killed, nor the parent.
#[test]
fn kill_leaves_a_sibling_command_alive() {
    let j = Job::new();
    use std::os::unix::process::CommandExt;
    let script = format!("\"$0\" deep {m} \"$1\" 2 & \"$0\" deep {m} \"$2\" 2 & wait", m = j.marker);
    let mut sh = quiet(Command::new("/bin/sh").args(["-c", &script, fixture(), &j.rec(), j.file(".b").to_str().unwrap()]).process_group(0)).spawn().unwrap();
    j.wait_recorded(4);
    let a = read_pairs(&j.rec);
    let b = read_pairs(&j.file(".b"));
    let target = a[0]; // A's first level (it records itself first)
    contained(target.0);
    let code = kill(&j, target.0);
    let outside: Vec<i32> = b.iter().map(|&(p, _)| p).chain([sh.id() as i32]).collect();
    assert!(signalled(&j).contains(&target.0), "control: the signal log does not name the target: {:?}", j.log());
    assert!(signalled(&j).iter().all(|p| !outside.contains(p)), "a process outside the target's tree was signalled: {:?}", j.log());
    let (a_left, b_left) = (a.iter().filter(|&&(p, id)| same(p, id)).count(), b.iter().filter(|&&(p, id)| same(p, id)).count());
    let sh_alive = matches!(sh.try_wait(), Ok(None));
    send_child(&mut sh, libc::SIGKILL);
    let _ = sh.wait();
    assert_eq!(code, Some(0));
    assert_eq!(a_left, 0, "the target's tree survived");
    assert_eq!(b_left, 2, "the sibling command was killed");
    assert!(sh_alive, "the parent shell was killed");
}

/// The other side of a pipe (`producer | tee`) is not signalled when the producer is killed.
#[test]
fn kill_does_not_signal_a_tee_partner() {
    let j = Job::new();
    use std::os::unix::process::CommandExt;
    let script = format!("\"$0\" deep {m} \"$1\" 2 | tee /dev/null", m = j.marker);
    let mut sh = quiet(Command::new("/bin/sh").args(["-c", &script, fixture(), &j.rec()]).process_group(0)).spawn().unwrap();
    j.wait_recorded(2);
    let tee = wait_for_child_named(sh.id() as i32, "tee");
    let target = read_pairs(&j.rec)[0];
    contained(target.0);
    let code = kill(&j, target.0);
    let _ = wait_bounded(&mut sh, Duration::from_secs(10));
    assert_eq!(code, Some(0));
    assert!(!signalled(&j).is_empty(), "control: the log shows no signal at all");
    assert!(!signalled(&j).contains(&tee), "the tee partner {tee} was signalled: {:?}", j.log());
}

/// A child of `parent` whose command name is `name`.
fn wait_for_child_named(parent: i32, name: &str) -> i32 {
    let mut pid = 0;
    wait_for(name, Duration::from_secs(10), || {
        let out = Command::new("ps").args(["-Ao", "pid=,ppid=,comm="]).output().unwrap();
        pid = String::from_utf8_lossy(&out.stdout)
            .lines()
            .find_map(|l| {
                let w: Vec<&str> = l.split_whitespace().collect();
                (w.len() >= 3 && w[1] == parent.to_string() && w[2].rsplit('/').next() == Some(name)).then(|| w[0].parse().ok()).flatten()
            })
            .unwrap_or(0);
        pid > 0
    });
    pid
}

/// The target of a `kill_*` cell sits under a parent and in a group the test made, never under
/// the test binary or in its group: a regression that widens the kill set (to the group, the
/// siblings, the parent's children) then reaches only this cell's processes.
fn contained(target: i32) {
    let ppid: i32 = String::from_utf8_lossy(&Command::new("ps").args(["-o", "ppid=", "-p", &target.to_string()]).output().unwrap().stdout).trim().parse().unwrap();
    assert_ne!(ppid, std::process::id() as i32, "the target's parent is the test binary");
    assert_ne!(unsafe { libc::getpgid(target) }, unsafe { libc::getpgrp() }, "the target is in the test binary's group");
}

/// Two concurrent jobs under one shell the test made, in a group of its own: the jobs are
/// siblings and group-mates (so a kill widened to the group or to siblings would reach the other
/// job), and nothing outside the cell is. `sheepdog kill` of one job's supervisor leaves the other
/// job whole, its supervisor and the shell included, and signals none of them.
#[test]
fn kill_of_one_job_leaves_a_concurrent_job_whole() {
    use std::os::unix::process::CommandExt;
    let j = Job::new();
    let k = Job::new();
    let mut sh = quiet(Command::new("/bin/sh").args([
        "-c",
        "\"$0\" run --quiet -- \"$1\" ticker \"$2\" \"$3\" & \"$0\" run --quiet -- \"$1\" ticker \"$4\" \"$5\" & wait",
        sheepdog(),
        fixture(),
        &j.marker,
        &j.rec(),
        &k.marker,
        &k.rec(),
    ]).process_group(0)).spawn().unwrap();
    for x in [&j, &k] {
        wait_for("the ticker root", Duration::from_secs(15), || x.file(".root").exists());
        x.wait_recorded(7);
    }
    // each job's supervisor: its root's parent
    let sup_of = |x: &Job| -> (i32, u64) {
        let root = read_pairs(&x.file(".root"))[0];
        let ppid: i32 = String::from_utf8_lossy(&Command::new("ps").args(["-o", "ppid=", "-p", &root.0.to_string()]).output().unwrap().stdout).trim().parse().unwrap();
        found(ppid).expect("the supervisor")
    };
    let (sup1, sup2) = (sup_of(&j), sup_of(&k));
    contained(sup1.0);
    let code = kill(&j, sup1.0);
    let mut two_all: Vec<(i32, u64)> = k.recorded().into_iter().chain(read_pairs(&k.file(".root"))).chain([sup2]).collect();
    two_all.sort();
    two_all.dedup();
    let two_left: Vec<(i32, u64)> = two_all.iter().copied().filter(|&(p, id)| same(p, id)).collect();
    let sh_alive = matches!(sh.try_wait(), Ok(None));
    let log = signalled(&j);
    let hit: Vec<i32> = log.iter().copied().filter(|p| *p == sh.id() as i32 || two_all.iter().any(|&(q, _)| q == *p)).collect();
    send(sup2.0, sup2.1, libc::SIGTERM);
    let _ = wait_bounded(&mut sh, Duration::from_secs(30));
    assert_eq!(code, Some(0));
    assert_eq!(j.alive(), vec![], "the killed job survived");
    assert!(log.contains(&sup1.0), "control: the signal log does not name the target: {:?}", j.log());
    assert_eq!(two_left.len(), 8, "the other job lost processes: {two_left:?}");
    assert!(sh_alive, "the shared shell ended");
    assert_eq!(hit, Vec::<i32>::new(), "the other job or the shell was signalled: {:?}", j.log());
}

/// A same-uid process in the target's process group, started before the target, is not killed
/// (a group is not membership).
#[test]
fn kill_leaves_an_older_process_of_the_targets_group_alive() {
    use std::os::unix::process::CommandExt;
    let j = Job::new();
    let script = format!("/bin/sleep {m} & \"$0\" deep {m} \"$1\" 2 & wait", m = j.marker);
    let mut sh = quiet(Command::new("/bin/sh").args(["-c", &script, fixture(), &j.rec()]).process_group(0)).spawn().unwrap();
    j.wait_recorded(2);
    let tree = read_pairs(&j.rec);
    // the older process, once it runs /bin/sleep (before that it is a fork of the shell)
    let mut older: Vec<(i32, u64)> = Vec::new();
    wait_for("the older /bin/sleep", Duration::from_secs(15), || {
        older = scan(&j.marker, |w| w.len() == 3 && w[1] == "/bin/sleep").unwrap().into_iter().filter(|&(p, _)| !tree.iter().any(|&(q, _)| q == p)).collect();
        older.len() == 1
    });
    let target = tree[0];
    contained(target.0);
    assert_eq!(unsafe { libc::getpgid(older[0].0) }, unsafe { libc::getpgid(target.0) }, "control: not in the target's group");
    let code = kill(&j, target.0);
    let older_alive = same(older[0].0, older[0].1);
    let outside = [older[0].0, sh.id() as i32];
    assert!(signalled(&j).contains(&target.0), "control: the signal log does not name the target: {:?}", j.log());
    assert!(signalled(&j).iter().all(|p| !outside.contains(p)), "a process outside the target's tree was signalled: {:?}", j.log());
    send_child(&mut sh, libc::SIGKILL);
    let _ = sh.wait();
    assert_eq!(code, Some(0));
    assert!(older_alive, "the older process of the target's group was killed");
    assert!(read_pairs(&j.rec).iter().all(|&(p, id)| !same(p, id)), "the target's tree survived");
}
