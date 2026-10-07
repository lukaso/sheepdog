//! Phase-2 P4b: the auto-sweep before every `sheepr run` (PLAN.md §3.5; PHASE2.md §3.5-§3.7):
//! the same owner's dead jobs, no grace, a 500 ms deadline per pass (what it stopped is continued
//! when a pass misses it), a live inner supervisor and its set left for an explicit `sweep`,
//! `--no-sweep`, `--owner`.

mod common;

use common::json::{self, Json};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

fn sheepr() -> &'static str {
    common::test_env();
    env!("CARGO_BIN_EXE_sheepr")
}
fn fixture() -> &'static str {
    common::test_env();
    env!("CARGO_BIN_EXE_sr-fixture")
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("sr-as-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn state(d: &Path) -> PathBuf {
    let s = d.join("state");
    std::fs::create_dir_all(&s).unwrap();
    std::fs::write(s.join(".sheepr-test"), b"").unwrap();
    s
}

fn journals(s: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    for b in std::fs::read_dir(s.join("jobs")).into_iter().flatten().flatten() {
        for f in std::fs::read_dir(b.path()).into_iter().flatten().flatten() {
            if f.path().extension().is_some_and(|e| e == "journal") {
                v.push(f.path());
            }
        }
    }
    v
}

fn journaled(s: &Path) -> Vec<i32> {
    journals(s)
        .iter()
        .flat_map(|j| std::fs::read_to_string(j).unwrap_or_default().lines().filter_map(|l| json::parse(l).ok()).collect::<Vec<_>>())
        .filter_map(|l| l.get("pid").and_then(Json::num).map(|n| n as i32))
        .collect()
}

fn records(r: &Path) -> Vec<(i32, u64)> {
    std::fs::read_to_string(r)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            Some((w.next()?.parse().ok()?, w.next()?.parse().ok()?))
        })
        .collect()
}

fn wait_until(secs: u64, mut f: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    while !f() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(10));
    }
    f()
}

fn counted(r: &Path) -> usize {
    std::fs::read_to_string(format!("{}.sig", r.display())).map(|s| s.lines().count()).unwrap_or(0)
}

/// A dead job: `sheepr run [flags] -- escapee-and-wait R`, SIGKILLed once its escapee is
/// journaled. Returns the escapee.
fn dead_job(d: &Path, s: &Path, name: &str, flags: &[&str]) -> ((i32, u64), PathBuf) {
    let r = d.join(name);
    let mut c: Child = Command::new(sheepr()).arg("run").args(flags).arg("--").arg(fixture()).arg("escapee-and-wait").arg(&r).env("SHEEPR_TEST_STATE", s).spawn().unwrap();
    assert!(wait_until(15, || !records(&r).is_empty()), "{name}: the escapee started");
    let g = records(&r)[0];
    assert!(wait_until(10, || journaled(s).contains(&g.0)), "{name}: the escapee was journaled");
    common::send_child(&mut c, libc::SIGKILL);
    let _ = c.wait();
    (g, r)
}

/// The outer run of a cell, SIGKILLed when the cell ends (a panic too): a live outer holds the
/// test's output pipe, and cargo would wait for it.
struct Outer(std::process::Child);
impl Drop for Outer {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            common::send_child(&mut self.0, libc::SIGKILL);
            let _ = self.0.wait();
        }
    }
}

/// Record files and argv markers whose processes are SIGKILLed (by identity) when the cell
/// ends, a panic too.
struct Leftovers(Vec<PathBuf>, Vec<String>);
impl Drop for Leftovers {
    fn drop(&mut self) {
        cleanup(&self.0.iter().map(PathBuf::as_path).collect::<Vec<_>>());
        common::kill_marked(&self.1.iter().map(String::as_str).collect::<Vec<_>>());
    }
}

fn cleanup(recs: &[&Path]) {
    for r in recs {
        for suffix in ["", ".g", ".root"] {
            for p in records(&PathBuf::from(format!("{}{suffix}", r.display()))) {
                common::send(p.0, p.1, libc::SIGKILL);
            }
        }
    }
}

/// `sheepr run --status-fd 3 FLAGS -- CMD...` with this state; (exit code, status).
fn run(d: &Path, s: &Path, name: &str, flags: &str, cmd: &str, env: &[(&str, &str)]) -> (Option<i32>, Option<Json>) {
    let out = d.join(format!("{name}.status"));
    let mut c = Command::new("/bin/sh");
    c.args(["-c", &format!(r#"exec "$SD" run --status-fd 3 {flags} -- {cmd} 3>"{}""#, out.display())]).env("SD", sheepr()).env("FX", fixture()).env("SHEEPR_TEST_STATE", s);
    for (k, v) in env {
        c.env(k, v);
    }
    let code = c.status().unwrap().code();
    (code, std::fs::read_to_string(&out).ok().and_then(|t| json::parse(t.trim_end()).ok()))
}

fn notes(st: &Option<Json>) -> Vec<String> {
    st.as_ref().and_then(|s| s.get("notes")).and_then(Json::arr).unwrap_or(&[]).iter().filter_map(Json::str).map(String::from).collect()
}

/// Issue #15: a journal the auto-sweep cannot read (no header) is one `notes` entry that names
/// the file, as an explicit `sweep` says it on stderr; before, it left no trace at all. The
/// control, a live job's journal in the same folder, gives no note.
#[test]
fn the_auto_sweep_notes_a_journal_it_cannot_read() {
    let d = scratch("unread");
    let s = state(&d);
    let go = d.join("go");
    let script = format!(r#"while [ ! -e "{}" ]; do sleep 0.05; done"#, go.display());
    let mut live = Outer(Command::new(sheepr()).args(["run", "--no-sweep", "--", "/bin/sh", "-c", &script]).env("SHEEPR_TEST_STATE", &s).spawn().unwrap());
    assert!(wait_until(15, || journals(&s).len() == 1), "the live job's journal");
    let lj = journals(&s)[0].clone();
    let lname = lj.file_name().unwrap().to_string_lossy().into_owned();
    let (_, ctl) = run(&d, &s, "ctl", "", "/bin/sh -c 'exit 0'", &[]);
    let bad = lj.parent().unwrap().join("j-b4d.journal");
    std::fs::write(&bad, "not a journal\n").unwrap();
    std::fs::set_permissions(&bad, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    // a journal kept on purpose (--leave-strays), with this boot's header: skipped, never noted
    let header = std::fs::read_to_string(&lj).unwrap().lines().next().unwrap().to_string();
    let job = json::parse(&header).unwrap().get("job").and_then(Json::str).unwrap().to_string();
    let kept = lj.parent().unwrap().join("j-k3pt.journal");
    std::fs::write(&kept, format!("{}\n{{\"v\":1,\"kind\":\"leave-strays\"}}\n", header.replace(&job, "j-k3pt"))).unwrap();
    std::fs::set_permissions(&kept, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    let (code, st) = run(&d, &s, "bad", "", "/bin/sh -c 'exit 0'", &[]);
    std::fs::write(&go, b"").unwrap();
    let _ = live.0.wait();
    let said = |st: &Option<Json>, name: &str| notes(st).iter().filter(|n| n.contains(name)).count();
    assert!(ctl.is_some(), "control: a status line");
    assert_eq!(said(&ctl, &lname), 0, "control: a live job's journal: {:?}", notes(&ctl));
    assert_eq!(code, Some(0));
    assert_eq!(said(&st, "j-b4d.journal"), 1, "{:?}", notes(&st));
    assert_eq!(said(&st, &lname), 0, "a live job's journal: {:?}", notes(&st));
    assert_eq!(said(&st, "j-k3pt"), 0, "a journal kept on purpose: {:?}", notes(&st));
    assert!(kept.exists(), "the kept journal is left in place");
    let _ = std::fs::remove_dir_all(&d);
}

fn mode(p: &Path, m: u32) {
    std::fs::set_permissions(p, std::os::unix::fs::PermissionsExt::from_mode(m)).unwrap();
}

/// Folders a cell narrowed, put back to 0700 when the cell ends, a panic too, so a failed cell
/// never leaves a folder `rm -rf` cannot remove.
struct Modes(Vec<PathBuf>);
impl Drop for Modes {
    fn drop(&mut self) {
        for p in &self.0 {
            let _ = std::fs::set_permissions(p, std::os::unix::fs::PermissionsExt::from_mode(0o700));
        }
    }
}

/// Review of 6d0ae1a, P2-1: a journal folder the auto-sweep cannot list (0300), or cannot reach
/// (`jobs/` at 0600), is one `auto-sweep skipped` note that names it, and the command still runs;
/// before, it was read as empty and noted nothing. The control, the same state at 0700, sweeps
/// the dead job. Skipped as root (root reads a folder of any mode). (No "the escapee lives"
/// assertion: nothing can reach a journal it cannot name, review of 35624d5 P3-4.)
#[test]
fn the_auto_sweep_notes_a_folder_it_cannot_read() {
    if unsafe { libc::geteuid() } == 0 {
        eprintln!("skipped: root reads a folder of any mode");
        return;
    }
    let d = scratch("unlisted");
    let s = state(&d);
    let (g, r) = dead_job(&d, &s, "a", &[]);
    let dir = journals(&s)[0].parent().unwrap().to_path_buf();
    let here = dir.file_name().unwrap().to_string_lossy().into_owned();
    let skipped = |st: &Option<Json>| notes(st).iter().filter(|n| n.starts_with("auto-sweep skipped") && n.contains(here.as_str())).count();
    let _restore = Modes(vec![dir.clone(), s.join("jobs")]);
    mode(&dir, 0o300);
    let (code1, st1) = run(&d, &s, "unlisted", "", "/bin/sh -c 'exit 0'", &[]);
    mode(&dir, 0o700);
    mode(&s.join("jobs"), 0o600);
    let (code2, st2) = run(&d, &s, "unreached", "", "/bin/sh -c 'exit 0'", &[]);
    mode(&s.join("jobs"), 0o700);
    let (code3, st3) = run(&d, &s, "ctl", "", "/bin/sh -c 'exit 0'", &[]);
    let gone = !common::alive(g);
    cleanup(&[&r]);
    assert_eq!((code1, skipped(&st1)), (Some(0), 1), "a folder it cannot list: {:?}", notes(&st1));
    assert_eq!((code2, skipped(&st2)), (Some(0), 1), "a folder it cannot reach: {:?}", notes(&st2));
    assert_eq!(code3, Some(0));
    assert!(gone, "control: the same state at 0700 is swept");
    assert_eq!(skipped(&st3), 0, "control: {:?}", notes(&st3));
    let _ = std::fs::remove_dir_all(&d);
}

/// Review of 35624d5, P2-1: a normal state gives no auto-sweep note: the first `run` on a fresh
/// state, and a `run` whose `jobs/` has no folder for this boot (as after a reboot).
#[test]
fn a_normal_state_gives_no_auto_sweep_note() {
    let d = scratch("normal");
    let fresh = state(&d.join("fresh"));
    let noboot = state(&d.join("noboot"));
    std::fs::create_dir_all(noboot.join("jobs")).unwrap();
    mode(&noboot.join("jobs"), 0o700);
    for (n, s) in [("fresh", &fresh), ("no boot folder", &noboot)] {
        let (code, st) = run(&d, s, n.split(' ').next().unwrap(), "", "/bin/sh -c 'exit 0'", &[]);
        assert_eq!(code, Some(0), "{n}");
        assert!(st.is_some(), "{n}: a status line");
        assert!(!notes(&st).iter().any(|x| x.starts_with("auto-sweep")), "{n}: {:?}", notes(&st));
    }
    let _ = std::fs::remove_dir_all(&d);
}

/// The next run sweeps a dead job of its owner before its command starts; with `--no-sweep`
/// it does not (the control).
#[test]
fn the_next_run_sweeps_a_dead_job() {
    let d = scratch("next");
    let s = state(&d);
    let (g, r) = dead_job(&d, &s, "a", &[]);
    let (code_no, _) = run(&d, &s, "nosweep", "--no-sweep", "/bin/sh -c 'exit 0'", &[]);
    let alive_after_no = common::alive(g);
    let (code, _) = run(&d, &s, "sweep", "", "/bin/sh -c 'exit 0'", &[]);
    let alive = common::alive(g);
    cleanup(&[&r]);
    assert_eq!(code_no, Some(0));
    assert!(alive_after_no, "control: --no-sweep left the dead job alone");
    assert_eq!(code, Some(0));
    assert!(!alive, "the auto-sweep left the escapee alive");
    assert!(journals(&s).is_empty(), "the swept journal is removed (and the new run's too)");
    let _ = std::fs::remove_dir_all(&d);
}

/// The auto-sweep takes only its own owner's jobs: a default run leaves a `liveapp` job; a
/// `--owner liveapp` run sweeps it.
#[test]
fn the_auto_sweep_keeps_to_its_owner() {
    let d = scratch("owner");
    let s = state(&d);
    let (g, r) = dead_job(&d, &s, "a", &["--owner", "liveapp"]);
    let _ = run(&d, &s, "default", "", "/bin/sh -c 'exit 0'", &[]);
    let alive_default = common::alive(g);
    let _ = run(&d, &s, "liveapp", "--owner liveapp", "/bin/sh -c 'exit 0'", &[]);
    let alive = common::alive(g);
    cleanup(&[&r]);
    assert!(alive_default, "a default run swept a liveapp job");
    assert!(!alive, "the liveapp run did not sweep its own job");
    let _ = std::fs::remove_dir_all(&d);
}

/// A pass that misses its 500 ms deadline (one member cannot be killed: seam) continues what it
/// stopped, keeps the journal, notes `partial`, and the new command still runs.
#[test]
fn a_partial_pass_leaves_nothing_stopped() {
    let d = scratch("partial");
    let s = state(&d);
    let (g, r) = dead_job(&d, &s, "a", &[]);
    let ran = d.join("ran");
    let (code, st) = run(&d, &s, "p", "", &format!(r#"/bin/sh -c 'touch "{}"'"#, ran.display()), &[("SHEEPR_TEST_UNKILLABLE", &g.0.to_string())]);
    let (alive, stopped) = (common::alive(g), common::stopped(g));
    cleanup(&[&r]);
    assert_eq!(code, Some(0));
    assert!(ran.exists(), "the new command ran");
    assert!(alive, "control: the seam kept the member alive");
    assert!(!stopped, "the member was left stopped");
    assert!(notes(&st).iter().any(|n| n.starts_with("partial: job")), "notes {:?}", notes(&st));
    assert!(!journals(&s).is_empty(), "the journal is kept");
    let _ = std::fs::remove_dir_all(&d);
}

/// The auto-sweep journals every candidate before its first signal, also one born during its
/// kill: a child born after the pass's first freeze (its parent, a member the test resumes while
/// the pass holds) has its journal line before any signal to it.
#[test]
fn the_auto_sweep_journals_a_member_born_during_its_kill() {
    let d = scratch("autojournal");
    let s = state(&d);
    let (go, r, rr, log, ready, release) = (d.join("go"), d.join("rec"), d.join("root"), d.join("log"), d.join("ready"), d.join("release"));
    let _left = Leftovers(vec![r.clone(), rr.clone()], vec![]);
    let script = format!(r#""$FX" spawn-on "{}" "{}" & exec "$FX" sigcount "{}""#, go.display(), r.display(), rr.display());
    let mut c = Outer(Command::new(sheepr()).args(["run", "--", "/bin/sh", "-c", &script]).env("FX", fixture()).env("SHEEPR_TEST_STATE", &s).spawn().unwrap());
    assert!(wait_until(15, || !records(&r).is_empty() && !records(&rr).is_empty()), "the job did not start");
    let parent = records(&r)[0];
    assert!(wait_until(10, || journaled(&s).contains(&parent.0) && journaled(&s).contains(&records(&rr)[0].0)), "the job did not journal its members");
    common::send_child(&mut c.0, libc::SIGKILL);
    let _ = c.0.wait();
    let (d2, s2, l2, rd2, rl2) = (d.clone(), s.clone(), log.clone(), ready.clone(), release.clone());
    let t = std::thread::spawn(move || {
        let env = [
            ("SHEEPR_TEST_SIGNAL_LOG", l2.to_str().unwrap()),
            ("SHEEPR_TEST_DEADLINE_MS", "5000"),
            ("SHEEPR_TEST_HOLD_AFTER_FREEZE", rl2.to_str().unwrap()),
            ("SHEEPR_TEST_READY_FILE", rd2.to_str().unwrap()),
        ];
        run(&d2, &s2, "new", "", "/bin/sh -c 'exit 0'", &env)
    });
    // the auto pass has frozen the dead job and holds: resume the parent, which spawns now
    let held = wait_until(15, || ready.exists());
    common::send(parent.0, parent.1, libc::SIGCONT);
    std::fs::write(&go, b"").unwrap();
    let born = wait_until(10, || records(&r).len() >= 2);
    std::fs::write(&release, b"").unwrap();
    let (code, _) = t.join().unwrap();
    let child = records(&r).get(1).copied();
    assert!(held, "the auto pass never reached its hold");
    assert!(born, "the child was not born during the hold");
    assert_eq!(code, Some(0));
    let cp = child.unwrap().0.to_string();
    let text = std::fs::read_to_string(&log).unwrap_or_default();
    let j = text.lines().position(|l| l.split_whitespace().collect::<Vec<_>>() == ["journal", cp.as_str()]);
    let first = text.lines().position(|l| {
        let w: Vec<&str> = l.split_whitespace().collect();
        w.len() >= 2 && (w[0] == "kill" || w[0] == "pidfd") && w[1] == cp
    });
    assert!(first.is_some(), "the auto-sweep never signalled the child:\n{text}");
    assert!(j.is_some() && j < first, "journal at {j:?}, first signal at {first:?}:\n{text}");
    let _ = std::fs::remove_dir_all(&d);
}

/// The timeout counts the command's time, not the auto-sweep's: a pass held for 2 s (one
/// member cannot be killed, and the pass deadline is 2 s: seams) before `run --timeout 1.5s` of
/// a command that needs 0.5 s leaves it its whole 1.5 s (it finishes, exit 0, no trigger).
#[test]
fn the_timeout_starts_after_the_auto_sweep() {
    let d = scratch("clock");
    let s = state(&d);
    let (g, r) = dead_job(&d, &s, "a", &[]);
    let ran = d.join("ran");
    let t0 = std::time::Instant::now();
    let (code, st) = run(
        &d,
        &s,
        "c",
        "--timeout 1.5s",
        &format!(r#"/bin/sh -c '"$FX" sleep-ms 500 && touch "{}"'"#, ran.display()),
        &[("SHEEPR_TEST_UNKILLABLE", &g.0.to_string()), ("SHEEPR_TEST_DEADLINE_MS", "2000")],
    );
    let took = t0.elapsed();
    cleanup(&[&r]);
    assert!(notes(&st).iter().any(|n| n.starts_with("partial: job")), "control: the pass ran to its deadline; notes {:?}", notes(&st));
    assert!(took >= std::time::Duration::from_millis(2000), "control: the pass took {took:?}");
    assert!(ran.exists(), "the timeout fired before the command had its 1.5 s (code {code:?}, {st:?})");
    assert_eq!(code, Some(0));
    let _ = std::fs::remove_dir_all(&d);
}

/// A live inner supervisor in a dead outer job, and its set, are left for an explicit `sweep`:
/// the auto-sweep sends them nothing (the inner job's escapee counts no signal), a note names
/// the deferral, and the new command runs. The control: an explicit `sweep` ends them.
#[test]
fn the_auto_sweep_defers_a_live_inner_supervisor() {
    let d = scratch("defer");
    let s = state(&d);
    let (r, rr) = (d.join("rec"), d.join("root"));
    let inner_state = state(&d.join("inner"));
    let script = format!(
        r#"SHEEPR_TEST_STATE="{}" sheepr run -- "$FX" escapee-and-wait "{}" & echo $! > "{}"; exec "$FX" sigcount "{}""#,
        inner_state.display(),
        r.display(),
        d.join("innerpid").display(),
        rr.display()
    );
    // the inner run and its escapee carry r's path in their argv
    let _left = Leftovers(vec![r.clone(), rr.clone()], vec![r.display().to_string()]);
    let mut c = Outer(Command::new(sheepr()).args(["run", "--", "/bin/sh", "-c", &script]).env("FX", fixture()).env("SHEEPR_TEST_STATE", &s).spawn().unwrap());
    assert!(wait_until(15, || !records(&r).is_empty()));
    let g = records(&r)[0];
    // the outer supervisor has journaled the inner one (readiness, not a fixed wait)
    let inner: i32 = { assert!(wait_until(10, || std::fs::read_to_string(d.join("innerpid")).is_ok_and(|t| !t.trim().is_empty()))); std::fs::read_to_string(d.join("innerpid")).unwrap().trim().parse().unwrap() };
    assert!(wait_until(10, || journaled(&s).contains(&inner)), "the inner supervisor was journaled");
    common::send_child(&mut c.0, libc::SIGKILL);
    let _ = c.0.wait();
    let gr = PathBuf::from(format!("{}.g", r.display()));
    let ran = d.join("ran");
    let (code, st) = run(&d, &s, "auto", "", &format!(r#"/bin/sh -c 'touch "{}"'"#, ran.display()), &[]);
    let (alive, n) = (common::alive(g), counted(&gr));
    let deferred = notes(&st).iter().any(|n| n.starts_with("deferred"));
    // control: an explicit sweep ends the inner supervisor, and with it its job
    let _ = Command::new(sheepr()).arg("sweep").env("SHEEPR_TEST_STATE", &s).status();
    let gone = wait_until(5, || !common::alive(g));
    cleanup(&[&r, &rr]);
    assert_eq!(code, Some(0));
    assert!(ran.exists(), "the new command ran");
    assert!(alive && n == 0, "the auto-sweep reached the live inner job: {n} signal(s)");
    assert!(deferred, "notes {:?}", notes(&st));
    assert!(gone, "control: the explicit sweep ended the inner job");
    let _ = std::fs::remove_dir_all(&d);
}

/// Review P2-3: the auto-sweep's kill is not the new run's: a dead job with several escapees
/// swept before `run --max-procs 1 -- true` leaves the new run's record clean (exit 0, trigger
/// null, killed []), and its own cap still works on its own job (a control that forks past it
/// gives 124).
#[test]
fn the_auto_sweep_is_not_the_new_runs_kill() {
    let d = scratch("pollute");
    let s = state(&d);
    let (g1, r1) = dead_job(&d, &s, "a", &[]);
    let (g2, r2) = dead_job(&d, &s, "b", &[]);
    let (code, st) = run(&d, &s, "clean", "--max-procs 1", "/bin/sh -c 'exit 0'", &[]);
    let swept = !common::alive(g1) && !common::alive(g2);
    let rf = d.join("forker");
    let (capped, cst) = run(&d, &s, "capped", "--max-procs 1", &format!(r#""$FX" forker 5 50 "{}""#, rf.display()), &[]);
    for p in records(&rf) {
        common::send(p.0, p.1, libc::SIGKILL);
    }
    cleanup(&[&r1, &r2]);
    let field = |st: &Option<Json>, k: &str| st.as_ref().and_then(|s| s.get(k)).cloned();
    assert!(swept, "control: the auto-sweep ended the dead jobs");
    assert_eq!(code, Some(0));
    assert_eq!(field(&st, "trigger"), Some(Json::Null));
    assert_eq!(field(&st, "killed").and_then(|k| k.arr().map(|a| a.len())), Some(0), "the dead jobs' processes are in the new run's killed[]");
    assert_eq!(capped, Some(124), "the new run's own cap");
    assert_eq!(field(&cst, "trigger").and_then(|t| t.str().map(String::from)), Some("cap".to_string()));
    let _ = std::fs::remove_dir_all(&d);
}

/// Review P2-7: the auto-sweep of a `sheepr run` started inside a dead job (from its member W)
/// skips that job whole: W and the job's witness live, the inner run's command runs, and a note
/// names the skipped job.
#[test]
fn the_auto_sweep_skips_a_job_that_holds_the_run() {
    let d = scratch("autoanc");
    let s = state(&d);
    let (go, ran, out, me, wit, rr) = (d.join("go"), d.join("ran"), d.join("inner.status"), d.join("me"), d.join("witness"), d.join("root"));
    let m = format!("29.{}", std::process::id());
    let w = format!(
        r#"echo $$ > "{}"; while [ ! -e "{}" ]; do sleep 0.02; done; sheepr run --status-fd 9 -- /bin/sh -c 'touch "{}"' 9>"{}"; sleep {m}"#,
        me.display(),
        go.display(),
        ran.display(),
        out.display()
    );
    let script = format!(r#"/bin/sh -c '{}' & "$FX" sigcount "{}" & exec "$FX" sigcount "{}""#, w.replace('\'', r#"'\''"#), wit.display(), rr.display());
    let _left = Leftovers(vec![wit.clone(), rr.clone()], vec![m.clone()]);
    let mut c = Outer(Command::new(sheepr()).args(["run", "--", "/bin/sh", "-c", &script]).env("FX", fixture()).env("SHEEPR_TEST_STATE", &s).spawn().unwrap());
    let wpid: Option<i32> = wait_until(15, || std::fs::read_to_string(&me).is_ok_and(|t| !t.trim().is_empty())).then(|| std::fs::read_to_string(&me).unwrap().trim().parse().unwrap());
    assert!(wpid.is_some_and(|p| wait_until(10, || journaled(&s).contains(&p))) && wait_until(10, || !records(&wit).is_empty()));
    let witness = records(&wit)[0];
    assert!(wait_until(10, || journaled(&s).contains(&witness.0)));
    common::send_child(&mut c.0, libc::SIGKILL);
    let _ = c.0.wait();
    std::fs::write(&go, b"").unwrap();
    let done = wait_until(20, || ran.exists() && std::fs::read_to_string(&out).is_ok_and(|t| !t.is_empty()));
    let st = std::fs::read_to_string(&out).ok().and_then(|t| json::parse(t.trim_end()).ok());
    let (wit_alive, n) = (common::alive(witness), counted(&wit));
    let noted = wpid.is_some_and(|w| notes(&st).iter().any(|n| n.starts_with("auto-sweep skipped job j-") && n.contains(&w.to_string())));
    let wid = wpid.and_then(sheepr::ident::identity);
    if let Some(p) = wpid.zip(wid) {
        common::send(p.0, p.1, libc::SIGKILL);
    }
    cleanup(&[&wit, &rr]);
    common::kill_marked(&[&m]);
    assert!(done, "the inner run's command ran");
    assert!(wit_alive && n == 0, "the job holding the run was swept ({n})");
    assert!(noted, "notes {:?}", notes(&st));
    let _ = std::fs::remove_dir_all(&d);
}
