//! Phase-2 P3: the caps (PLAN.md §3.1, §3.4; PHASE2.md §1 decisions 9-10): `--timeout` (running
//! time), `--max-mem`, `--max-procs`, `--kill-deadline`; exit 124; the first trigger recorded,
//! later ones as notes; a cap that first fires after the root ended by itself is a note only.

mod common;

use common::json::{self, Json};
use std::path::{Path, PathBuf};
use std::process::Command;
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
    let d = std::env::temp_dir().join(format!("sd-caps-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
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

fn progress(p: &Path) -> u64 {
    std::fs::read_to_string(p).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0)
}

struct Run {
    code: Option<i32>,
    status: Option<Json>,
    took: Duration,
    /// sheepdog's stderr (for messages)
    err: String,
    /// sheepdog's trace (SHEEPDOG_TEST_TRACE)
    trace: String,
}

/// `sheepdog run --status-fd 3 ARGS` (through a shell), bounded by `limit`; `during` runs while
/// it does and may end it early (returning true stops the wait with a TERM to sheepdog).
fn run(d: &Path, name: &str, args: &str, env: &[(&str, &str)], limit: Duration, mut during: impl FnMut() -> bool) -> Run {
    let out = d.join(format!("{name}.status"));
    let errf = d.join(format!("{name}.err"));
    let tracef = d.join(format!("{name}.trace"));
    let mut cmd = Command::new("/bin/sh");
    cmd.args(["-c", &format!(r#"exec "$SD" run --status-fd 3 {args} 3>"{}" 2>"{}""#, out.display(), errf.display())]).env("SD", sheepdog()).env("FX", fixture());
    cmd.env("SHEEPDOG_TEST_TRACE", &tracef);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let start = Instant::now();
    let mut c = cmd.spawn().unwrap();
    let mut termed = false;
    let code = loop {
        if let Some(s) = c.try_wait().unwrap() {
            break s.code();
        }
        if !termed && during() {
            common::send_child(&mut c, libc::SIGTERM);
            termed = true;
        }
        if start.elapsed() > limit {
            common::send_child(&mut c, libc::SIGKILL);
            let _ = c.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let took = start.elapsed();
    let text = std::fs::read_to_string(&out).unwrap_or_default();
    Run { code, status: text.lines().next().and_then(|l| json::parse(l).ok()), took, err: std::fs::read_to_string(&errf).unwrap_or_default(), trace: std::fs::read_to_string(&tracef).unwrap_or_default() }
}

fn field<'a>(r: &'a Run, k: &str) -> Option<&'a Json> {
    r.status.as_ref().and_then(|s| s.get(k))
}

/// The cause the kill report names (its trace line `report-cause C`).
fn cause(r: &Run) -> Option<String> {
    r.trace.lines().find_map(|l| l.strip_prefix("report-cause ")).map(String::from)
}

fn notes(r: &Run) -> Vec<String> {
    field(r, "notes").and_then(Json::arr).unwrap_or(&[]).iter().filter_map(Json::str).map(String::from).collect()
}

/// Cell 13: a job that grows at 100 MB/s (every page written) is ended by `--max-mem 500M`
/// before it reaches 1 GB (exit 124, trigger `cap`); the control, the same job without the cap,
/// reaches its own stop at 1.2 GB. The two runs are serial, never in parallel.
#[test]
fn cell_13_max_mem_ends_a_growing_job() {
    let d = scratch("mem");
    let (p1, p2) = (d.join("capped"), d.join("control"));
    let capped = run(&d, "capped", &format!(r#"--max-mem 500M -- "$FX" alloc 100 1200 "{}""#, p1.display()), &[], Duration::from_secs(40), || false);
    let reached = progress(&p1);
    let control = run(&d, "control", &format!(r#"-- "$FX" alloc 100 1200 "{}""#, p2.display()), &[], Duration::from_secs(40), || progress(&p2) >= 1200);
    assert_eq!(capped.code, Some(124), "the cap ends the job");
    assert_eq!(field(&capped, "trigger").and_then(Json::str), Some("cap"));
    assert!(reached < 1000, "it reached {reached} MB under a 500 MB cap");
    assert!(progress(&p2) >= 1200, "control: without the cap it reached {} MB", progress(&p2));
    assert_eq!(field(&control, "trigger").and_then(Json::str), Some("term"), "control ended by the test's TERM");
    // the kill report names what ended the job, as the status line does
    assert_eq!(cause(&capped).as_deref(), Some("--max-mem"), "{}", capped.err);
    assert_eq!(cause(&control).as_deref(), Some("term"), "control: {}", control.err);
    let _ = std::fs::remove_dir_all(&d);
}

/// `--max-procs 8` ends a job that forks one process every 100 ms up to 20 (exit 124, trigger
/// `cap`, fewer than 20 forked); the control without the cap forks all 20.
#[test]
fn max_procs_ends_a_forking_job() {
    let d = scratch("procs");
    let (r1, r2) = (d.join("capped"), d.join("control"));
    let capped = run(&d, "capped", &format!(r#"--max-procs 8 -- "$FX" forker 20 100 "{}""#, r1.display()), &[], Duration::from_secs(30), || false);
    let n = records(&r1).len();
    let control = run(&d, "control", &format!(r#"-- "$FX" forker 20 100 "{}""#, r2.display()), &[], Duration::from_secs(30), || records(&r2).len() >= 20);
    for p in records(&r1).into_iter().chain(records(&r2)) {
        common::send(p.0, p.1, libc::SIGKILL);
    }
    assert_eq!(capped.code, Some(124));
    assert_eq!(field(&capped, "trigger").and_then(Json::str), Some("cap"));
    assert!(n < 20, "{n} forked under --max-procs 8");
    assert_eq!(records(&r2).len(), 20, "control: all 20 without the cap");
    assert_eq!(cause(&capped).as_deref(), Some("--max-procs"), "{}", capped.err);
    let _ = control;
    let _ = std::fs::remove_dir_all(&d);
}

/// `--timeout 1s` ends a job that would run 30 s: exit 124, trigger `timeout` at about 1000 ms.
#[test]
fn timeout_ends_the_job() {
    let d = scratch("timeout");
    let r = run(&d, "t", r#"--timeout 1s -- /bin/sh -c 'sleep 30'"#, &[], Duration::from_secs(20), || false);
    assert_eq!(r.code, Some(124));
    assert_eq!(field(&r, "trigger").and_then(Json::str), Some("timeout"));
    let at = field(&r, "trigger_at").and_then(Json::num).unwrap_or(0.0);
    assert!(at >= 1000.0, "trigger_at {at} ms before the 1 s timeout");
    assert!(r.took < Duration::from_secs(10), "took {:?}", r.took);
    assert_eq!(cause(&r).as_deref(), Some("--timeout"), "{}", r.err);
    let _ = std::fs::remove_dir_all(&d);
}

/// The first trigger is recorded: the timeout fires, then during the TERM grace the member
/// allocates past `--max-mem`; `trigger` stays `timeout` and the cap is a note (caps are also
/// evaluated while the job is being ended).
#[test]
fn the_first_trigger_is_recorded_and_a_later_cap_is_a_note() {
    let d = scratch("first");
    let p = d.join("p");
    let r = run(
        &d,
        "first",
        &format!(r#"--timeout 1s --max-mem 300M --grace 4s -- "$FX" alloc-on-term 600 "{}""#, p.display()),
        &[],
        Duration::from_secs(30),
        || false,
    );
    assert_eq!(r.code, Some(124));
    assert_eq!(field(&r, "trigger").and_then(Json::str), Some("timeout"));
    assert!(progress(&p) >= 600, "control: the member allocated in the grace ({} MB)", progress(&p));
    assert!(notes(&r).iter().any(|n| n.starts_with("cap")), "notes {:?}", notes(&r));
    let _ = std::fs::remove_dir_all(&d);
}

/// A timeout that first fires after the root ended by itself (while a stray that ignores TERM
/// is still in its grace) is a note only: the exit code is the command's own (0).
#[test]
fn a_cap_after_the_root_ended_by_itself_is_only_a_note() {
    let d = scratch("late");
    let r1 = d.join("rec");
    let r = run(
        &d,
        "late",
        &format!(r#"--timeout 1s --grace 3s -- /bin/sh -c '"$FX" sigcount "{}" & while [ ! -s "{}" ]; do sleep 0.01; done; exit 0'"#, r1.display(), r1.display()),
        &[],
        Duration::from_secs(30),
        || false,
    );
    for p in records(&r1) {
        common::send(p.0, p.1, libc::SIGKILL);
    }
    assert_eq!(r.code, Some(0), "the command's own code");
    assert_eq!(field(&r, "trigger"), Some(&Json::Null));
    assert!(notes(&r).iter().any(|n| n.starts_with("timeout")), "notes {:?}", notes(&r));
    assert!(r.took >= Duration::from_millis(1500), "control: the grace outlasted the timeout ({:?})", r.took);
    let _ = std::fs::remove_dir_all(&d);
}

/// One order when events meet in one wake, on both OSes: TERM, then the root's exit, then the
/// caps. A root that exits while the job is past `--max-procs 1` in the same wake (a seam holds
/// the supervisor before its wait while both happen) ends with its own code (7), the cap a
/// note; the control, the same job whose root keeps running, is ended by the cap (124).
#[test]
fn the_roots_exit_comes_before_a_cap_in_one_wake() {
    let d = scratch("exitcap");
    let (r1, r2) = (d.join("rec"), d.join("ctl"));
    // the root forks only once the supervisor holds (the seam writes the ready file after its
    // first check, which then saw the root and at most one `sleep`: within --max-procs 2); three
    // members outlive the root, so the job stays past the cap after the root exits
    let body = |r: &Path, end: &str| format!(r#"--max-procs 2 -- /bin/sh -c ': > "{0}"; while [ ! -e "{0}.ready" ]; do sleep 0.01; done; "$FX" sigcount "{0}" & "$FX" sigcount "{0}" & "$FX" sigcount "{0}" & while [ "$(wc -l < "{0}")" -lt 3 ]; do sleep 0.01; done; {end}'"#, r.display());
    let hold = |r: &Path| [("SHEEPDOG_TEST_SLEEP_BEFORE_WAIT_MS", "1500".to_string()), ("SHEEPDOG_TEST_READY_FILE", format!("{}.ready", r.display()))];
    let (h1, h2) = (hold(&r1), hold(&r2));
    let (e1, e2): (Vec<(&str, &str)>, Vec<(&str, &str)>) = (h1.iter().map(|(k, v)| (*k, v.as_str())).collect(), h2.iter().map(|(k, v)| (*k, v.as_str())).collect());
    let ended = run(&d, "exit", &body(&r1, "exit 7"), &e1, Duration::from_secs(30), || false);
    let control = run(&d, "ctl", &body(&r2, "while :; do sleep 0.05; done"), &e2, Duration::from_secs(30), || false);
    for p in records(&r1).into_iter().chain(records(&r2)) {
        common::send(p.0, p.1, libc::SIGKILL);
    }
    assert_eq!(ended.code, Some(7), "the command's own code: {:?} {}", ended.status, ended.err);
    assert_eq!(field(&ended, "trigger"), Some(&Json::Null));
    assert_eq!(control.code, Some(124), "control: the cap ends a job whose root runs");
    assert_eq!(field(&control, "trigger").and_then(Json::str), Some("cap"));
    let _ = std::fs::remove_dir_all(&d);
}

/// A TERM and a cap in one wake: the TERM is the trigger and sheepdog dies of TERM, on both
/// OSes (the TERM arrives while a seam holds the supervisor and the job is past the cap).
#[test]
fn a_term_comes_before_a_cap_in_one_wake() {
    let d = scratch("termcap");
    let r1 = d.join("rec");
    let ready = format!("{}.ready", r1.display());
    let hold = [("SHEEPDOG_TEST_SLEEP_BEFORE_WAIT_MS", "1500"), ("SHEEPDOG_TEST_READY_FILE", ready.as_str())];
    // as above: the root forks past --max-procs 2 once the supervisor holds; the TERM comes
    // while it holds
    let r = run(
        &d,
        "t",
        &format!(r#"--max-procs 2 -- /bin/sh -c 'while [ ! -e "{0}.ready" ]; do sleep 0.01; done; "$FX" sigcount "{0}" & "$FX" sigcount "{0}" & "$FX" sigcount "{0}" & while :; do sleep 0.05; done'"#, r1.display()),
        &hold,
        Duration::from_secs(30),
        || records(&r1).len() >= 3,
    );
    for p in records(&r1) {
        common::send(p.0, p.1, libc::SIGKILL);
    }
    assert_eq!(field(&r, "trigger").and_then(Json::str), Some("term"), "{:?} {}", r.status, r.err);
    assert_eq!(r.code, None, "death by TERM");
    assert!(r.took < Duration::from_secs(20), "the test's limit ended it ({:?})", r.took);
    let _ = std::fs::remove_dir_all(&d);
}

/// `--kill-deadline` shortens the kill deadline: a member that cannot be killed (seam) makes
/// sheepdog give up after about 0.5 s, not the default 10 s (exit 125, deadline_missed).
#[test]
fn kill_deadline_shortens_the_deadline() {
    let d = scratch("deadline");
    let r1 = d.join("rec");
    let r = run(
        &d,
        "dl",
        &format!(r#"--grace 0 --kill-deadline 500ms -- "$FX" escape 29.8 "{}""#, r1.display()),
        &[("SHEEPDOG_TEST_NOKILL", "1")],
        Duration::from_secs(30),
        || false,
    );
    for p in records(&r1) {
        common::send(p.0, p.1, libc::SIGKILL);
    }
    assert_eq!(r.code, Some(125));
    assert_eq!(field(&r, "deadline_missed"), Some(&Json::Bool(true)));
    assert!(r.took < Duration::from_secs(5), "took {:?} with a 500 ms deadline", r.took);
    let _ = std::fs::remove_dir_all(&d);
}

/// Review P2-4: a TERM from outside is the trigger from the moment it is taken: a timeout that
/// then expires during the kill's TERM grace is a note, and sheepdog dies of the TERM.
#[test]
fn a_term_before_the_timeout_stays_the_trigger() {
    let d = scratch("termfirst");
    let r1 = d.join("rec");
    let start = Instant::now();
    let r = run(
        &d,
        "tf",
        &format!(r#"--timeout 1s --grace 3s -- "$FX" sigcount "{}""#, r1.display()),
        &[],
        Duration::from_secs(30),
        || start.elapsed() > Duration::from_millis(500),
    );
    for p in records(&r1) {
        common::send(p.0, p.1, libc::SIGKILL);
    }
    assert_eq!(field(&r, "trigger").and_then(Json::str), Some("term"));
    assert!(notes(&r).iter().any(|n| n.starts_with("timeout")), "notes {:?}", notes(&r));
    assert_eq!(r.code, None, "sheepdog dies of the TERM");
    let _ = std::fs::remove_dir_all(&d);
}
