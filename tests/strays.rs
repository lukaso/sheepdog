//! Phase-2 P7: `sheepdog strays` and `strays --kill` (PLAN.md §3.0, PHASE2.md D8).
//!
//! `strays` reads every process of this user, so every cell filters with `--cmd <word>`, a word
//! unique to the cell that is in the scratch path of every process it built; `--kill` then
//! reaches only them (and the wall keeps any untagged process). Cells run one at a time.

mod common;

use common::json::{self, Json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

static SEQ: AtomicUsize = AtomicUsize::new(0);

fn serial() -> std::sync::MutexGuard<'static, ()> {
    static L: std::sync::Mutex<()> = std::sync::Mutex::new(());
    L.lock().unwrap_or_else(|e| e.into_inner())
}

fn sheepdog() -> &'static str {
    common::test_env();
    env!("CARGO_BIN_EXE_sheepdog")
}
fn fixture() -> &'static str {
    common::test_env();
    env!("CARGO_BIN_EXE_sd-fixture")
}

/// A cell's scratch directory; its name is the cell's filter word.
struct Cell {
    dir: PathBuf,
    word: String,
    recs: Vec<PathBuf>,
}

impl Cell {
    fn new() -> Cell {
        let word = format!("sdstr{}x{}", std::process::id(), SEQ.fetch_add(1, Ordering::SeqCst));
        let dir = std::env::temp_dir().join(&word);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Cell { dir, word, recs: Vec::new() }
    }
    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }
    /// Run the fixture with ARGS (DIR is `{}`) to its end; remember NAMES as records to clean.
    fn make(&mut self, args: &[&str], names: &[&str]) {
        let d = self.dir.to_str().unwrap().to_string();
        let a: Vec<String> = args.iter().map(|x| x.replace("{}", &d)).collect();
        let st = Command::new(fixture()).args(&a).stdin(Stdio::null()).status().unwrap();
        assert!(st.success(), "fixture {a:?}: {st:?}");
        for n in names {
            self.recs.push(self.path(n));
        }
    }
    fn rec(&self, name: &str) -> Option<(i32, u64)> {
        let t = std::fs::read_to_string(self.path(name)).ok()?;
        let mut w = t.lines().next()?.split_whitespace();
        Some((w.next()?.parse().ok()?, w.next()?.parse().ok()?))
    }
    fn signals(&self, name: &str) -> usize {
        std::fs::read_to_string(self.path(&format!("{name}.sig"))).unwrap_or_default().lines().count()
    }
    fn untouched(&self, name: &str) -> bool {
        self.rec(name).is_some_and(common::alive) && self.signals(name) == 0
    }
}

impl Drop for Cell {
    fn drop(&mut self) {
        for r in &self.recs {
            for suffix in ["", ".d"] {
                let p = PathBuf::from(format!("{}{suffix}", r.display()));
                if let Some(t) = std::fs::read_to_string(&p).ok() {
                    for l in t.lines() {
                        let mut w = l.split_whitespace();
                        if let (Some(Ok(pid)), Some(Ok(id))) = (w.next().map(str::parse::<i32>), w.next().map(str::parse::<u64>)) {
                            common::send(pid, id, libc::SIGKILL);
                        }
                    }
                }
            }
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn wait_until(secs: u64, mut f: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    f()
}

struct Out {
    code: Option<i32>,
    out: String,
    err: String,
}

/// `sheepdog strays ARGS` (stdin not a tty), 60 s bound by the kill deadline.
fn strays(args: &[&str], env: &[(&str, &str)]) -> Out {
    let mut c = Command::new(sheepdog());
    c.arg("strays").args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    for (k, v) in env {
        c.env(k, v);
    }
    let o = c.output().unwrap();
    Out { code: o.status.code(), out: String::from_utf8_lossy(&o.stdout).into_owned(), err: String::from_utf8_lossy(&o.stderr).into_owned() }
}

/// `--json` rows by pid.
fn rows(out: &str) -> HashMap<i32, Json> {
    out.lines().filter_map(|l| json::parse(l).ok()).filter_map(|j| Some((j.get("pid")?.num()? as i32, j))).collect()
}

/// Cell 30: the 2026-09-25 shape (its parent exited, it was reparented) is listed, with its
/// memory, age, CPU and origin; a child of a live parent is not listed (not an orphan).
#[test]
fn strays_lists_the_orphan_with_its_origin() {
    let _s = serial();
    let mut c = Cell::new();
    c.make(&["stray", "{}", "orphan"], &["orphan"]);
    let kid_rec = c.path("kid");
    let mut kid = Command::new(fixture()).args(["sigcount", kid_rec.to_str().unwrap()]).spawn().unwrap();
    assert!(wait_until(10, || c.rec("kid").is_some()));
    let o = strays(&["--json", "--cmd", &c.word], &[]);
    let _ = common::send_child(&mut kid, libc::SIGKILL);
    let _ = kid.wait();
    let r = rows(&o.out);
    let orphan = c.rec("orphan").unwrap();
    assert_eq!(o.code, Some(0), "{}", o.err);
    let row = r.get(&orphan.0).unwrap_or_else(|| panic!("the orphan is not listed:\n{}", o.out));
    for k in ["mem", "age_s", "cpu_s", "cmd", "origin"] {
        assert!(row.get(k).is_some(), "no {k}: {}", o.out);
    }
    assert!(!r.contains_key(&c.rec("kid").unwrap().0), "a child of a live parent is listed:\n{}", o.out);
}

/// Cell 30, macOS: an orphan that exec'd only after it was reparented has `puniq` 1 (as an
/// `open -g` or launchd job has) and is not listed.
#[cfg(target_os = "macos")]
#[test]
fn strays_does_not_list_a_launchd_started_shape() {
    let _s = serial();
    let mut c = Cell::new();
    c.make(&["late-exec", "{}", "late"], &["late"]);
    c.make(&["stray", "{}", "orphan"], &["orphan"]);
    let o = strays(&["--json", "--cmd", &c.word], &[]);
    let r = rows(&o.out);
    assert!(r.contains_key(&c.rec("orphan").unwrap().0), "control: the orphan is not listed:\n{}", o.out);
    assert!(!r.contains_key(&c.rec("late").unwrap().0), "the puniq-1 shape is listed:\n{}", o.out);
}

/// Cell 31: `--kill` without a filter refuses (2); without `--yes` in a non-tty refuses (1) and
/// signals nothing; with both it kills only the rows that match the filter.
#[test]
fn strays_kill_needs_a_filter_and_a_yes() {
    let _s = serial();
    let mut a = Cell::new();
    let mut b = Cell::new();
    a.make(&["stray", "{}", "s"], &["s"]);
    b.make(&["stray", "{}", "s"], &["s"]);
    let none = strays(&["--kill"], &[]);
    let zero = strays(&["--kill", "--min-mem", "0", "--older-than", "0"], &[]);
    let no_yes = strays(&["--kill", "--cmd", &a.word], &[]);
    assert_eq!(none.code, Some(2), "{}", none.err);
    assert_eq!(zero.code, Some(2), "{}", zero.err);
    assert_eq!(no_yes.code, Some(1), "{}", no_yes.err);
    assert!(a.untouched("s"), "a refused --kill signalled the stray");
    let yes = strays(&["--kill", "--yes", "--cmd", &a.word], &[]);
    assert_eq!(yes.code, Some(0), "{}", yes.err);
    assert!(wait_until(5, || !a.rec("s").is_some_and(common::alive)), "the matching stray survived");
    assert!(b.untouched("s"), "a stray that does not match was signalled");
}

/// Cell 29(c): `strays --kill --older-than 1s` skips a stray that belongs to a running job (an
/// orphan of the job whose parent exited; its root lives on as `sigcount`); on macOS it is
/// listed as that job's, and naming it with `--pid` kills it. (On Linux the job's supervisor, a
/// subreaper, adopts it, so it is no stray there.)
#[test]
fn strays_kill_skips_a_running_jobs_escapee() {
    let _s = serial();
    let mut c = Cell::new();
    c.recs.push(c.path("esc"));
    c.recs.push(c.path("root"));
    let d = c.dir.to_str().unwrap().to_string();
    let mut sup = Command::new(sheepdog())
        .args(["run", "--", "/bin/sh", "-c", r#""$0" stray "$1" esc && exec "$0" sigcount "$1/root""#, fixture(), &d])
        .stdin(Stdio::null())
        .spawn()
        .unwrap();
    assert!(wait_until(15, || c.rec("esc").is_some() && c.rec("root").is_some()));
    if cfg!(target_os = "macos") {
        let l = strays(&["--json", "--cmd", &c.word], &[]);
        let row = rows(&l.out).remove(&c.rec("esc").unwrap().0);
        assert!(row.is_some_and(|r| r.get("job").is_some_and(|j| j.str().is_some())), "the job's stray is not listed as the job's:\n{}", l.out);
    }
    std::thread::sleep(Duration::from_millis(1200));
    let o = strays(&["--kill", "--yes", "--older-than", "1s", "--cmd", &c.word], &[]);
    let kept = c.untouched("esc");
    let named = if cfg!(target_os = "macos") {
        let g = c.rec("esc").unwrap();
        let n = strays(&["--kill", "--yes", "--pid", &format!("{}:{}", g.0, g.1), "--cmd", &c.word], &[]);
        Some((n.code, wait_until(5, || !common::alive(g))))
    } else {
        None
    };
    let _ = common::send_child(&mut sup, libc::SIGKILL);
    let _ = sup.wait();
    assert!(kept, "the running job's escapee was killed or signalled; strays said:\n{}{}", o.out, o.err);
    if let Some((code, gone)) = named {
        assert!(code == Some(0) && gone, "control: named with --pid, the escapee survived ({code:?})");
    }
}

/// Cell 29(d), macOS: a double-forked helper inside a live app's bundle (its responsible
/// process is the app) is not listed and survives `--kill`; the 2026-09-25 shape next to it is
/// listed and killed.
#[cfg(target_os = "macos")]
#[test]
fn strays_skips_a_live_apps_helper() {
    let _s = serial();
    let mut c = Cell::new();
    let bundle = c.path("Fake.app/Contents/MacOS");
    std::fs::create_dir_all(&bundle).unwrap();
    let (app_exe, helper_exe) = (bundle.join("Fake"), bundle.join("Helper"));
    std::fs::copy(fixture(), &app_exe).unwrap();
    std::fs::copy(fixture(), &helper_exe).unwrap();
    for p in ["app", "helper"] {
        c.recs.push(c.path(p));
    }
    let d = c.dir.to_str().unwrap().to_string();
    let mut app = Command::new(&app_exe).args(["app", &d, helper_exe.to_str().unwrap()]).stdin(Stdio::null()).spawn().unwrap();
    assert!(wait_until(20, || c.rec("app").is_some() && c.rec("helper").is_some()), "the app did not start");
    c.make(&["stray", "{}", "orphan"], &["orphan"]);
    let list = strays(&["--json", "--cmd", &c.word], &[]);
    let r = rows(&list.out);
    let k = strays(&["--kill", "--yes", "--cmd", &c.word], &[]);
    let helper_kept = c.untouched("helper");
    let orphan_gone = wait_until(5, || !c.rec("orphan").is_some_and(common::alive));
    let _ = common::send_child(&mut app, libc::SIGKILL);
    let _ = app.wait();
    assert!(r.contains_key(&c.rec("orphan").unwrap().0), "control: the 2026-09-25 shape is not listed:\n{}", list.out);
    assert!(!r.contains_key(&c.rec("helper").unwrap().0), "the app's helper is listed:\n{}", list.out);
    assert!(helper_kept, "the app's helper was signalled");
    assert!(orphan_gone, "control: the orphan survived --kill ({:?}): {}", k.code, k.err);
}

/// A stray that is an ancestor of the `strays --kill` that names it (a daemon that runs it) is
/// refused by `kill`'s target checks, and survives.
#[test]
fn strays_kill_refuses_the_callers_ancestor() {
    let _s = serial();
    let mut c = Cell::new();
    let sd = sheepdog().to_string();
    let w = c.word.clone();
    c.make(&["stray-run", "{}", "daemon", &sd, "strays", "--kill", "--yes", "--cmd", &w], &["daemon", "daemon.d"]);
    let d = c.rec("daemon.d").unwrap();
    // the daemon's strays ends (its code), or the daemon itself was killed
    assert!(wait_until(30, || c.path("daemon.code").exists() || !common::alive(d)), "the daemon's strays did not end");
    assert!(common::alive(d), "the daemon (the caller's ancestor) was killed");
    let code = std::fs::read_to_string(c.path("daemon.code")).unwrap_or_default();
    assert_eq!(code, "1", "the refusal's exit code");
}

/// The identity read at the listing is what `--kill` checks: with every row's identity made
/// wrong (the debug seam), the row is refused and the stray survives.
#[test]
fn strays_kill_checks_the_identity_it_listed() {
    let _s = serial();
    let mut c = Cell::new();
    c.make(&["stray", "{}", "s"], &["s"]);
    let o = strays(&["--kill", "--yes", "--cmd", &c.word], &[("SHEEPDOG_TEST_STRAYS_WRONG_ID", "1")]);
    assert!(c.untouched("s"), "a row whose identity changed was killed: {}", o.err);
    assert_eq!(o.code, Some(1), "{}", o.err);
}

/// The wall control for strays (PHASE2.md §0.1): an untagged stray that matches the filter gets
/// `withheld` from `--kill` (its counter stays 0, a `withheld` line names it).
#[test]
fn strays_kill_turns_the_latch_on() {
    let _s = serial();
    let mut c = Cell::new();
    c.make(&["stray", "{}", "s", "bare"], &["s"]);
    let sink = c.path("sink");
    let o = strays(&["--kill", "--yes", "--cmd", &c.word], &[("SHEEPDOG_TEST_SINK", sink.to_str().unwrap()), ("SHEEPDOG_TEST_DEADLINE_MS", "500")]);
    let s = c.rec("s").unwrap();
    let lines = std::fs::read_to_string(&sink).unwrap_or_default();
    assert!(c.untouched("s"), "the untagged stray got {} signal(s): {}", c.signals("s"), o.err);
    assert!(lines.lines().any(|l| l.starts_with("withheld ") && l.split_whitespace().nth(1) == Some(&s.0.to_string())), "{lines:?}");
}
