//! Phase-2 P7: `sheepdog strays` and `strays --kill` (PLAN.md §3.0, PHASE2.md D8).
//!
//! `strays` reads every process of this user, so every cell filters with `--cmd <word>`, a word
//! unique to the cell that is in the scratch path of every process it built; `--kill` then
//! reaches only them (and the wall keeps any untagged process). Cells run one at a time.
//!
//! Linux: the legs' containers run `--init`, so an orphan's parent is PID 1, not an OS init, and
//! every stray would be a `pid1-child` (skipped unless named). So on Linux a cell's strays are
//! made under a subreaper of its own named `systemd` (the `reaper` fixture: an OS init's name,
//! whose adoptees are unmarked strays): the unnamed paths of `--kill` run there too. A control
//! cell makes one under a `tini`-named reaper, whose children stay marked.

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

/// A cell's scratch directory; its name is the cell's filter word. Its reapers (Linux) and every
/// recorded process die with it.
struct Cell {
    dir: PathBuf,
    word: String,
    recs: Vec<PathBuf>,
    reapers: Vec<std::process::Child>,
    /// the Linux reaper's command name (`systemd`; the control cell uses `tini`)
    reaper_name: &'static str,
}

impl Cell {
    fn new() -> Cell {
        Cell::under(&std::env::temp_dir())
    }
    /// A cell whose directory is under BASE.
    fn under(base: &std::path::Path) -> Cell {
        // ends with `z`, so no word is a prefix of another (x1z, x10z)
        let word = format!("sdstr{}x{}z", std::process::id(), SEQ.fetch_add(1, Ordering::SeqCst));
        let dir = base.join(&word);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Cell { dir, word, recs: Vec::new(), reapers: Vec::new(), reaper_name: "systemd" }
    }
    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }
    /// Run PROG ARGS (DIR is `{}` in ARGS) to its end, on Linux under a reaper of this cell's;
    /// remember NAMES as records to clean, and wait until each is written.
    fn spawn(&mut self, prog: &str, args: &[&str], names: &[&str]) {
        let d = self.dir.to_str().unwrap().to_string();
        let a: Vec<String> = args.iter().map(|x| x.replace("{}", &d)).collect();
        for n in names {
            self.recs.push(self.path(n));
        }
        if cfg!(target_os = "linux") {
            // the maker has ended once READY exists: its orphans are the reaper's by then
            let ready = self.path(&format!("ready{}", self.reapers.len()));
            let r = Command::new(fixture()).args(["reaper", self.reaper_name, ready.to_str().unwrap(), prog]).args(&a).stdin(Stdio::null()).spawn().unwrap();
            self.reapers.push(r);
            assert!(wait_until(20, || ready.exists()), "{prog} {a:?} did not end");
        } else {
            let st = Command::new(prog).args(&a).stdin(Stdio::null()).status().unwrap();
            assert!(st.success(), "{prog} {a:?}: {st:?}");
        }
        let want: Vec<PathBuf> = names.iter().map(|n| self.path(n)).collect();
        assert!(wait_until(20, || want.iter().all(|p| std::fs::read_to_string(p).is_ok_and(|t| !t.is_empty()))), "{prog} {a:?} did not start {names:?}");
    }
    /// The fixture with ARGS: see `spawn`.
    fn make(&mut self, args: &[&str], names: &[&str]) {
        let fx = fixture().to_string();
        self.spawn(&fx, args, names);
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
        // the recorded processes first, then the reapers (a reaper killed first would hand its
        // adoptees to PID 1)
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
        for r in &mut self.reapers {
            common::send_child(r, libc::SIGKILL);
            let _ = r.wait();
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

/// The `--pid PID:ID` arguments that name this cell's rows marked `pid1-child` (a Linux container
/// whose PID 1 is not an OS init: `--kill` skips them unless named, PHASE2.md D9).
fn named_if_pid1_child(word: &str) -> Vec<String> {
    let o = strays(&["--json", "--cmd", word], &[]);
    let mut v = Vec::new();
    for j in rows(&o.out).values() {
        if j.get("pid1_child") == Some(&Json::Bool(true)) {
            v.push("--pid".to_string());
            v.push(format!("{}:{}", j.get("pid").and_then(Json::num).unwrap_or(0.0) as i64, j.get("id").and_then(Json::num).unwrap_or(0.0) as u64));
        }
    }
    v
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
    // the refusals run inert: if one broke, its kill would log and send nothing
    let inert = [("SHEEPDOG_TEST_INERT", "1")];
    let none = strays(&["--kill"], &inert);
    let empty = strays(&["--kill", "--yes", "--cmd", ""], &inert);
    assert_eq!(empty.code, Some(2), "an empty --cmd is no filter: {}", empty.err);
    let zero = strays(&["--kill", "--min-mem", "0", "--older-than", "0"], &inert);
    let no_yes = strays(&["--kill", "--cmd", &a.word], &inert);
    assert_eq!(none.code, Some(2), "{}", none.err);
    assert_eq!(zero.code, Some(2), "{}", zero.err);
    assert_eq!(no_yes.code, Some(1), "{}", no_yes.err);
    assert!(a.untouched("s"), "a refused --kill signalled the stray");
    let named = named_if_pid1_child(&a.word);
    let mut args = vec!["--kill", "--yes", "--cmd", &a.word];
    args.extend(named.iter().map(String::as_str));
    let yes = strays(&args, &[]);
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
    c.make(&["stray-run", "{}", "daemon", &sd, "strays", "--kill", "--yes", "--cmd", &w], &["daemon.d"]);
    c.recs.push(c.path("daemon")); // written when its strays has ended
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
    let named = named_if_pid1_child(&c.word);
    let mut args = vec!["--kill", "--yes", "--cmd", &c.word];
    args.extend(named.iter().map(String::as_str));
    let o = strays(&args, &[("SHEEPDOG_TEST_STRAYS_WRONG_ID", "1")]);
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
    let named = named_if_pid1_child(&c.word);
    let mut args = vec!["--kill", "--yes", "--cmd", &c.word];
    args.extend(named.iter().map(String::as_str));
    let mut k = Command::new(sheepdog());
    k.arg("strays").args(&args).env("SHEEPDOG_TEST_DEADLINE_MS", "500").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    common::cell_sink(&mut k, &sink);
    let out = k.output().unwrap();
    let s = c.rec("s").unwrap();
    let lines = std::fs::read_to_string(&sink).unwrap_or_default();
    assert!(c.untouched("s"), "the untagged stray got {} signal(s): {}", c.signals("s"), String::from_utf8_lossy(&out.stderr));
    // withheld lines, every one naming this cell's own untagged stray (none foreign)
    let held: Vec<&str> = lines.lines().filter(|l| l.starts_with("withheld ")).collect();
    assert!(!held.is_empty() && held.iter().all(|l| l.split_whitespace().nth(1) == Some(&s.0.to_string())), "{lines:?}");
}

/// A stray's row shows how many processes its kill would take (its proved tree); a stray whose
/// tree holds a live `sheepdog run` supervisor is skipped by `--kill` unless named, and named, it
/// is killed with its tree.
#[test]
fn strays_kill_skips_a_tree_that_holds_a_supervisor() {
    let _s = serial();
    let mut c = Cell::new();
    let sd = sheepdog().to_string();
    let fx = fixture().to_string();
    let inner = c.path("inner").to_str().unwrap().to_string();
    c.make(&["stray-run", "{}", "daemon", &sd, "run", "--", &fx, "sigcount", &inner], &["daemon.d", "inner"]);
    c.recs.push(c.path("daemon")); // never written: its program runs until killed
    assert!(wait_until(15, || c.rec("inner").is_some()), "the inner job did not start");
    let d = c.rec("daemon.d").unwrap();
    let l = strays(&["--json", "--cmd", &c.word], &[]);
    let row = rows(&l.out).remove(&d.0);
    let tree = row.as_ref().and_then(|r| r.get("tree")).and_then(Json::num).unwrap_or(0.0);
    let k = strays(&["--kill", "--yes", "--cmd", &c.word], &[]);
    let kept = common::alive(d) && c.rec("inner").is_some_and(common::alive);
    let named = strays(&["--kill", "--yes", "--pid", &format!("{}:{}", d.0, d.1), "--cmd", &c.word], &[]);
    let gone = wait_until(15, || !common::alive(d) && !c.rec("inner").is_some_and(common::alive));
    assert!(row.is_some(), "the daemon is not listed:\n{}", l.out);
    assert!(tree >= 2.0, "its tree (the supervisor and the job's root) is not counted: {tree}\n{}", l.out);
    assert!(kept, "a stray whose tree holds a live supervisor was killed unnamed: {}", k.err);
    assert!(gone, "control: named, the stray and its tree survived ({:?}): {}", named.code, named.err);
}

/// A `sheepdog run` supervisor that is itself a stray (its caller exited: `nohup sheepdog run ...
/// &` and a closed terminal) is listed as its job's and skipped by `--kill` unless named; named,
/// it is killed (its job ends with it).
#[test]
fn strays_kill_skips_an_orphaned_supervisor() {
    let _s = serial();
    let mut c = Cell::new();
    let sd = sheepdog().to_string();
    let fx = fixture().to_string();
    let script = format!(r#""{sd}" run -- "{fx}" sigcount "{{}}/inner" & sleep 1"#);
    c.spawn("/bin/sh", &["-c", &script], &["inner"]);
    let inner = c.rec("inner").unwrap();
    let sup = common::found(String::from_utf8_lossy(&Command::new("ps").args(["-o", "ppid=", "-p", &inner.0.to_string()]).output().unwrap().stdout).trim().parse().unwrap_or(0)).expect("the supervisor");
    struct Kill((i32, u64));
    impl Drop for Kill {
        fn drop(&mut self) {
            common::send(self.0 .0, self.0 .1, libc::SIGKILL);
        }
    }
    let _k = Kill(sup);
    // its caller (the shell) is gone: the supervisor's parent is PID 1 (macOS) or the reaper
    let ppid = || String::from_utf8_lossy(&Command::new("ps").args(["-o", "ppid=", "-p", &sup.0.to_string()]).output().unwrap().stdout).trim().to_string();
    // macOS: launchd; Linux: this cell's reaper (never PID 1: that would be a marked row)
    let adopters: Vec<String> = if cfg!(target_os = "linux") { c.reapers.iter().map(|r| r.id().to_string()).collect() } else { vec!["1".to_string()] };
    assert!(wait_until(10, || adopters.contains(&ppid())), "the supervisor was not adopted (parent {})", ppid());
    let l = strays(&["--json", "--cmd", &c.word], &[]);
    let row = rows(&l.out).remove(&sup.0);
    let k = strays(&["--kill", "--yes", "--cmd", &c.word], &[]);
    let kept = common::alive(sup) && common::alive(inner);
    let named = strays(&["--kill", "--yes", "--pid", &format!("{}:{}", sup.0, sup.1), "--cmd", &c.word], &[]);
    let gone = wait_until(15, || !common::alive(sup) && !common::alive(inner));
    assert!(row.is_some(), "the orphaned supervisor is not listed:\n{}", l.out);
    assert!(row.as_ref().is_some_and(|r| r.get("job").is_some_and(|j| j.str().is_some())), "it is not marked with its job:\n{}", l.out);
    assert!(kept, "an orphaned supervisor was killed unnamed: {}", k.err);
    assert!(gone, "control: named, the supervisor and its job survived ({:?}): {}", named.code, named.err);
}

/// `--cmd` matches the whole command line, not the shown one (capped at 256 bytes): a stray whose
/// filter word comes after its first 256 bytes is found.
#[test]
fn strays_cmd_matches_the_whole_command_line() {
    let _s = serial();
    let base = std::env::temp_dir().join("p".repeat(240));
    std::fs::create_dir_all(&base).unwrap();
    let mut c = Cell::under(&base);
    c.make(&["stray", "{}", "s"], &["s"]);
    let o = strays(&["--json", "--cmd", &c.word], &[]);
    let s = c.rec("s").unwrap();
    let row = rows(&o.out).remove(&s.0);
    drop(c);
    let _ = std::fs::remove_dir(&base);
    let r = row.unwrap_or_else(|| panic!("the stray is not found by a word past its first 256 bytes:\n{}", o.out));
    let shown = r.get("cmd").and_then(Json::str).unwrap_or("").to_string();
    assert!(shown.len() <= 300, "control: the shown command line is capped: {} bytes", shown.len());
    assert!(shown.ends_with('…'), "a cut command line does not say so: {shown}");
}

/// Linux: a child of a `tini`-named init (whatever its pid) is marked as that init's program and
/// survives an unnamed `--kill`; named, it is killed.
#[cfg(target_os = "linux")]
#[test]
fn strays_kill_skips_a_program_inits_child() {
    let _s = serial();
    let mut c = Cell::new();
    c.reaper_name = "tini";
    c.make(&["stray", "{}", "s"], &["s"]);
    let s = c.rec("s").unwrap();
    let l = strays(&["--json", "--cmd", &c.word], &[]);
    let marked = rows(&l.out).remove(&s.0).is_some_and(|r| r.get("pid1_child") == Some(&Json::Bool(true)));
    let k = strays(&["--kill", "--yes", "--cmd", &c.word], &[]);
    let kept = c.untouched("s");
    let named = strays(&["--kill", "--yes", "--pid", &format!("{}:{}", s.0, s.1), "--cmd", &c.word], &[]);
    let gone = wait_until(5, || !common::alive(s));
    assert!(marked, "a tini child is not marked:\n{}", l.out);
    assert!(kept, "a tini child was killed unnamed: {}", k.err);
    assert!(gone, "control: named, it survived ({:?}): {}", named.code, named.err);
}

/// On a terminal, `--kill` without `--yes` asks first, and the question starts with `sheepdog:`
/// as every message sheepdog writes does; "n" reaches no kill and says so, "y" reaches the kill
/// (inert: it logs the signal it would send). (macOS: `script` gives it the terminal.)
#[cfg(target_os = "macos")]
#[test]
fn strays_kill_asks_with_the_sheepdog_prefix() {
    use std::io::{Read, Write};
    let _s = serial();
    let mut a = Cell::new();
    a.make(&["stray", "{}", "s"], &["s"]);
    let log = a.path("inert.log");
    // the answer goes in after the question is on the terminal, and the input stays open until the
    // command has ended (at the end of its input script sends an end-of-file first, measured, and
    // an empty answer reads as no)
    let ask = |answer: &[u8]| {
        let _ = std::fs::remove_file(&log);
        let mut c = Command::new("/usr/bin/script");
        c.args(["-q", "/dev/null", sheepdog(), "strays", "--kill", "--cmd", &a.word]).env("SHEEPDOG_TEST_INERT", "1").env("SHEEPDOG_TEST_SIGNAL_LOG", &log);
        c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut ch = c.spawn().unwrap();
        let (mut input, mut out) = (ch.stdin.take().unwrap(), ch.stdout.take().unwrap());
        // the terminal's output on a thread, so every wait below has a limit (a question that
        // never comes fails this test, never hangs the suite)
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        let reader = std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = out.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        let mut seen = Vec::new();
        let mut until = |seen: &mut Vec<u8>, done: &dyn Fn(&[u8]) -> bool, secs: u64, what: &str, ch: &mut std::process::Child| {
            let end = Instant::now() + Duration::from_secs(secs);
            while !done(seen) {
                match rx.recv_timeout(end.saturating_duration_since(Instant::now())) {
                    Ok(chunk) => seen.extend_from_slice(&chunk),
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        let _ = ch.kill();
                        let _ = ch.wait();
                        panic!("{what} within {secs} s: {:?}", String::from_utf8_lossy(seen));
                    }
                }
            }
        };
        until(&mut seen, &|s| String::from_utf8_lossy(s).contains("[y/N]"), 20, "the question did not come", &mut ch);
        input.write_all(answer).unwrap();
        input.flush().unwrap();
        until(&mut seen, &|_| false, 60, "the command did not end", &mut ch);
        drop(input);
        ch.wait().unwrap();
        reader.join().unwrap();
        let all = String::from_utf8_lossy(&seen).into_owned();
        let sent = std::fs::read_to_string(&log).unwrap_or_default().lines().filter(|l| l.starts_with("inert ")).count();
        (all, sent)
    };
    let (no, no_sent) = ask(b"n\n");
    assert!(no.contains("sheepdog: kill these 1 process(es)? [y/N]"), "the question: {no:?}");
    assert!(no.contains("sheepdog: nothing was signalled.") && no_sent == 0, "a 'no' went on: {no:?} ({no_sent} logged)");
    let (yes, yes_sent) = ask(b"y\n");
    assert!(yes_sent > 0 && !yes.contains("nothing was signalled"), "control: a 'yes' did not reach the kill: {yes:?} ({yes_sent} logged)");
    assert!(a.untouched("s"), "the inert kill signalled the stray");
}
