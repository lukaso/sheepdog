//! Phase-2 P4b: the auto-sweep before every `sheepdog run` (PLAN.md §3.5; PHASE2.md §3.5-§3.7):
//! the same owner's dead jobs, no grace, a 500 ms deadline per pass (what it stopped is continued
//! when a pass misses it), a live inner supervisor and its set left for an explicit `sweep`,
//! `--no-sweep`, `--owner`.

mod common;

use common::json::{self, Json};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

fn sheepdog() -> &'static str {
    common::test_env();
    env!("CARGO_BIN_EXE_sheepdog")
}
fn fixture() -> &'static str {
    common::test_env();
    env!("CARGO_BIN_EXE_sd-fixture")
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("sd-as-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn state(d: &Path) -> PathBuf {
    let s = d.join("state");
    std::fs::create_dir_all(&s).unwrap();
    std::fs::write(s.join(".sheepdog-test"), b"").unwrap();
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

/// A dead job: `sheepdog run [flags] -- escapee-and-wait R`, SIGKILLed once its escapee is
/// journaled. Returns the escapee.
fn dead_job(d: &Path, s: &Path, name: &str, flags: &[&str]) -> ((i32, u64), PathBuf) {
    let r = d.join(name);
    let mut c: Child = Command::new(sheepdog()).arg("run").args(flags).arg("--").arg(fixture()).arg("escapee-and-wait").arg(&r).env("SHEEPDOG_TEST_STATE", s).spawn().unwrap();
    assert!(wait_until(15, || !records(&r).is_empty()), "{name}: the escapee started");
    let g = records(&r)[0];
    assert!(wait_until(10, || journaled(s).contains(&g.0)), "{name}: the escapee was journaled");
    common::send_child(&mut c, libc::SIGKILL);
    let _ = c.wait();
    (g, r)
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

/// `sheepdog run --status-fd 3 FLAGS -- CMD...` with this state; (exit code, status).
fn run(d: &Path, s: &Path, name: &str, flags: &str, cmd: &str, env: &[(&str, &str)]) -> (Option<i32>, Option<Json>) {
    let out = d.join(format!("{name}.status"));
    let mut c = Command::new("/bin/sh");
    c.args(["-c", &format!(r#"exec "$SD" run --status-fd 3 {flags} -- {cmd} 3>"{}""#, out.display())]).env("SD", sheepdog()).env("FX", fixture()).env("SHEEPDOG_TEST_STATE", s);
    for (k, v) in env {
        c.env(k, v);
    }
    let code = c.status().unwrap().code();
    (code, std::fs::read_to_string(&out).ok().and_then(|t| json::parse(t.trim_end()).ok()))
}

fn notes(st: &Option<Json>) -> Vec<String> {
    st.as_ref().and_then(|s| s.get("notes")).and_then(Json::arr).unwrap_or(&[]).iter().filter_map(Json::str).map(String::from).collect()
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
    let (code, st) = run(&d, &s, "p", "", &format!(r#"/bin/sh -c 'touch "{}"'"#, ran.display()), &[("SHEEPDOG_TEST_UNKILLABLE", &g.0.to_string())]);
    let (alive, stopped) = (common::alive(g), common::stopped(g));
    cleanup(&[&r]);
    assert_eq!(code, Some(0));
    assert!(ran.exists(), "the new command ran");
    assert!(alive, "control: the seam kept the member alive");
    assert!(!stopped, "the member was left stopped");
    assert!(notes(&st).iter().any(|n| n.starts_with("partial")), "notes {:?}", notes(&st));
    assert!(!journals(&s).is_empty(), "the journal is kept");
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
        r#"SHEEPDOG_TEST_STATE="{}" sheepdog run -- "$FX" escapee-and-wait "{}" & echo $! > "{}"; exec "$FX" sigcount "{}""#,
        inner_state.display(),
        r.display(),
        d.join("innerpid").display(),
        rr.display()
    );
    let mut c = Command::new(sheepdog()).args(["run", "--", "/bin/sh", "-c", &script]).env("FX", fixture()).env("SHEEPDOG_TEST_STATE", &s).spawn().unwrap();
    assert!(wait_until(15, || !records(&r).is_empty()));
    let g = records(&r)[0];
    // the outer supervisor has journaled the inner one (readiness, not a fixed wait)
    let inner: i32 = { assert!(wait_until(10, || std::fs::read_to_string(d.join("innerpid")).is_ok_and(|t| !t.trim().is_empty()))); std::fs::read_to_string(d.join("innerpid")).unwrap().trim().parse().unwrap() };
    assert!(wait_until(10, || journaled(&s).contains(&inner)), "the inner supervisor was journaled");
    common::send_child(&mut c, libc::SIGKILL);
    let _ = c.wait();
    let gr = PathBuf::from(format!("{}.g", r.display()));
    let ran = d.join("ran");
    let (code, st) = run(&d, &s, "auto", "", &format!(r#"/bin/sh -c 'touch "{}"'"#, ran.display()), &[]);
    let (alive, n) = (common::alive(g), counted(&gr));
    let deferred = notes(&st).iter().any(|n| n.starts_with("deferred"));
    // control: an explicit sweep ends the inner supervisor, and with it its job
    let _ = Command::new(sheepdog()).arg("sweep").env("SHEEPDOG_TEST_STATE", &s).status();
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

/// Review P2-7: the auto-sweep of a `sheepdog run` started inside a dead job (from its member W)
/// skips that job whole: W and the job's witness live, the inner run's command runs, and a note
/// names the skipped job.
#[test]
fn the_auto_sweep_skips_a_job_that_holds_the_run() {
    let d = scratch("autoanc");
    let s = state(&d);
    let (go, ran, out, me, wit, rr) = (d.join("go"), d.join("ran"), d.join("inner.status"), d.join("me"), d.join("witness"), d.join("root"));
    let m = format!("29.{}", std::process::id());
    let w = format!(
        r#"echo $$ > "{}"; while [ ! -e "{}" ]; do sleep 0.02; done; sheepdog run --status-fd 9 -- /bin/sh -c 'touch "{}"' 9>"{}"; sleep {m}"#,
        me.display(),
        go.display(),
        ran.display(),
        out.display()
    );
    let script = format!(r#"/bin/sh -c '{}' & "$FX" sigcount "{}" & exec "$FX" sigcount "{}""#, w.replace('\'', r#"'\''"#), wit.display(), rr.display());
    let mut c = Command::new(sheepdog()).args(["run", "--", "/bin/sh", "-c", &script]).env("FX", fixture()).env("SHEEPDOG_TEST_STATE", &s).spawn().unwrap();
    let wpid: Option<i32> = wait_until(15, || std::fs::read_to_string(&me).is_ok_and(|t| !t.trim().is_empty())).then(|| std::fs::read_to_string(&me).unwrap().trim().parse().unwrap());
    assert!(wpid.is_some_and(|p| wait_until(10, || journaled(&s).contains(&p))) && wait_until(10, || !records(&wit).is_empty()));
    let witness = records(&wit)[0];
    assert!(wait_until(10, || journaled(&s).contains(&witness.0)));
    common::send_child(&mut c, libc::SIGKILL);
    let _ = c.wait();
    std::fs::write(&go, b"").unwrap();
    let done = wait_until(20, || ran.exists() && std::fs::read_to_string(&out).is_ok_and(|t| !t.is_empty()));
    let st = std::fs::read_to_string(&out).ok().and_then(|t| json::parse(t.trim_end()).ok());
    let (wit_alive, n) = (common::alive(witness), counted(&wit));
    let noted = wpid.is_some_and(|w| notes(&st).iter().any(|n| n.starts_with("auto-sweep skipped job j-") && n.contains(&w.to_string())));
    let wid = wpid.and_then(sheepdog::ident::identity);
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
