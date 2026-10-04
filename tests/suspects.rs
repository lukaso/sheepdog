//! Phase-2 P6: suspects in `sheepr kill <pid>` and `sheepr ps` (PLAN.md §3.0, PHASE2.md D7).
//!
//! Every tree is built by the fixture in a session and group the test made (`new-session`), so a
//! suspect rule that widens reaches only this cell's processes, never the test runner's session.
//! The cells run one at a time (each builds a tree in a session of its own). A kill with the
//! suspects uses the run's own sink: a withheld line there (a suspect outside this cell's tree)
//! fails the run, so a rule that widens past the tree cannot pass unseen.
//! The tree (`suspect-tree`): an orphan started before the target (`early`), the target (`t`), an
//! orphan of the target that stays in its session (`g`, a suspect) with a child (`gc`, under a
//! suspect: listed and killed with it), one in a session of its own (`g2`: no suspect; on macOS
//! its `puniq` alone is no link, PHASE2.md D9), the same but responsible for itself (`g3`,
//! macOS), a non-orphan started after the target (`n`), and the session leader (`leader`). Each
//! counts its signals in `<name>.sig`.

mod common;

use common::json::{self, Json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// One cell at a time (see the module comment).
fn serial() -> std::sync::MutexGuard<'static, ()> {
    static L: std::sync::Mutex<()> = std::sync::Mutex::new(());
    L.lock().unwrap_or_else(|e| e.into_inner())
}

fn sheepr() -> &'static str {
    common::test_env();
    env!("CARGO_BIN_EXE_sheepr")
}
fn fixture() -> &'static str {
    common::test_env();
    env!("CARGO_BIN_EXE_sr-fixture")
}

const NAMES: [&str; 8] = ["early", "t", "g", "gc", "g2", "g3", "n", "leader"];

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("sr-sus-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
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

fn record(d: &Path, name: &str) -> Option<(i32, u64)> {
    let t = std::fs::read_to_string(d.join(name)).ok()?;
    let mut w = t.lines().next()?.split_whitespace();
    Some((w.next()?.parse().ok()?, w.next()?.parse().ok()?))
}

fn signals(d: &Path, name: &str) -> usize {
    std::fs::read_to_string(d.join(format!("{name}.sig"))).unwrap_or_default().lines().count()
}

/// A built tree; every process in it is SIGKILLed (by identity) when it drops, a panic too.
struct Tree {
    dir: PathBuf,
    procs: HashMap<&'static str, (i32, u64)>,
}

impl Tree {
    fn build(d: &Path, untagged: bool) -> Tree {
        Tree::build_with(d, untagged, false)
    }
    /// `sup`: the suspect's child is a `sheepr run` supervisor (its job's root is `gc`).
    fn build_with(d: &Path, untagged: bool, sup: bool) -> Tree {
        let mut c = Command::new(fixture());
        c.args(["new-session", fixture(), "suspect-tree", d.to_str().unwrap()]).stdin(Stdio::null());
        if sup {
            c.env("SR_SUP", sheepr());
        }
        if untagged {
            c.arg("untagged");
        }
        let _leader = c.spawn().unwrap(); // becomes `sigcount leader`; the Tree kills it
        let mut t = Tree { dir: d.to_path_buf(), procs: HashMap::new() };
        let want: Vec<&str> = NAMES.iter().copied().filter(|n| cfg!(target_os = "macos") || *n != "g3").collect();
        let ok = wait_until(20, || want.iter().all(|n| record(d, n).is_some()));
        for n in NAMES {
            if let Some(p) = record(d, n) {
                t.procs.insert(n, p);
            }
        }
        assert!(ok, "the tree did not start: {:?}", t.procs);
        t
    }
    fn p(&self, name: &str) -> (i32, u64) {
        self.procs[name]
    }
    fn alive(&self, name: &str) -> bool {
        common::alive(self.p(name))
    }
    fn untouched(&self, name: &str) -> bool {
        self.alive(name) && signals(&self.dir, name) == 0
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        for &(p, id) in self.procs.values() {
            common::send(p, id, libc::SIGKILL);
        }
    }
}

struct Out {
    code: Option<i32>,
    out: String,
    log: String,
}

/// `sheepr ARGS` with the signal log, 60 s bound.
fn sd(d: &Path, args: &[&str], env: &[(&str, &str)]) -> Out {
    let log = d.join(format!("log-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    let mut c = Command::new(sheepr());
    c.args(args).env("SHEEPR_TEST_SIGNAL_LOG", &log).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    for (k, v) in env {
        c.env(k, v);
    }
    let ch = c.spawn().unwrap();
    let o = ch.wait_with_output().unwrap();
    Out { code: o.status.code(), out: String::from_utf8_lossy(&o.stdout).into_owned(), log: std::fs::read_to_string(&log).unwrap_or_default() }
}

/// `ps --json` rows: pid -> (class, evidence).
fn rows(out: &str) -> HashMap<i32, (String, Vec<String>)> {
    out.lines()
        .filter_map(|l| json::parse(l).ok())
        .filter_map(|j| {
            let pid = j.get("pid")?.num()? as i32;
            let class = j.get("class")?.str()?.to_string();
            let ev = j.get("evidence").and_then(Json::arr).unwrap_or(&[]).iter().filter_map(|e| e.str().map(String::from)).collect();
            Some((pid, (class, ev)))
        })
        .collect()
}

/// The suspects `ps` lists for the target: on both OSes the orphan that stays in the target's
/// session; on macOS also the one in a session of its own (its `puniq` names its dead parent,
/// and it has the target's responsible process). Not listed: the orphan started before the
/// target, the non-orphan, the leader, and (macOS) the orphan responsible for itself. `ps`
/// signals nothing: its signal log is empty and no process counted a signal.
#[test]
fn ps_lists_the_suspects_and_signals_nothing() {
    let _serial = serial();
    let d = scratch("ps");
    let t = Tree::build(&d, false);
    let tp = t.p("t").0.to_string();
    let o = sd(&d, &["ps", "--json", &tp], &[]);
    let r = rows(&o.out);
    let class = |n: &str| r.get(&t.p(n).0).map(|(c, _)| c.clone());
    let want_suspects = vec!["g", "gc"];
    let suspects: Vec<&str> = NAMES.iter().copied().filter(|n| t.procs.contains_key(n) && class(n).as_deref() == Some("suspect")).collect();
    assert_eq!(o.code, Some(0), "{}", o.out);
    assert_eq!(class("t").as_deref(), Some("target"), "{}", o.out);
    assert_eq!(suspects, want_suspects, "rows:\n{}", o.out);
    for n in ["early", "n", "leader", "g2", "g3"] {
        if t.procs.contains_key(n) {
            assert_eq!(class(n), None, "{n} is listed:\n{}", o.out);
        }
    }
    assert!(o.log.is_empty(), "ps signalled: {}", o.log);
    for n in t.procs.keys() {
        assert_eq!(signals(&d, n), 0, "{n} counted a signal");
    }
    // the positive control: `kill` on a parallel tree of the same shape does signal (its log)
    let d2 = scratch("ps-control");
    let t2 = Tree::build(&d2, false);
    let k = sd(&d2, &["kill", &t2.p("t").0.to_string()], &[]);
    assert!(!k.log.is_empty(), "control: kill logged no signal");
    drop(t2);
    let _ = std::fs::remove_dir_all(&d2);
    drop(t);
    let _ = std::fs::remove_dir_all(&d);
}

/// `kill <target>` kills the proved tree and leaves the suspects alive, unsignalled.
#[test]
fn kill_leaves_the_suspects_alone_by_default() {
    let _serial = serial();
    let d = scratch("default");
    let t = Tree::build(&d, false);
    let o = sd(&d, &["kill", &t.p("t").0.to_string()], &[]);
    let target_gone = wait_until(5, || !t.alive("t"));
    assert_eq!(o.code, Some(0));
    assert!(target_gone, "the target survived");
    assert!(t.untouched("g"), "the suspect {:?} got {} signal(s), alive {}; tree {:?}; log:\n{}", t.p("g"), signals(&d, "g"), t.alive("g"), t.procs, o.log);
    drop(t);
    let _ = std::fs::remove_dir_all(&d);
}

/// `kill --include-suspects <target>` also kills the suspects, and nothing that is none: the
/// orphan started before the target, the non-orphan, the leader and (macOS) the orphan
/// responsible for itself are alive and unsignalled.
#[test]
fn include_suspects_kills_them_and_nothing_else() {
    let _serial = serial();
    let d = scratch("include");
    let t = Tree::build(&d, false);
    let listed = rows(&sd(&d, &["ps", "--json", &t.p("t").0.to_string()], &[]).out);
    let o = sd(&d, &["kill", "--include-suspects", &t.p("t").0.to_string()], &[("SHEEPR_TEST_DEADLINE_MS", "2000")]);
    let gone = vec!["t", "g", "gc"];
    // what the kill signalled (of this tree) is what `ps` listed before it
    let signalled: Vec<i32> = o.log.lines().filter_map(|l| {
        let w: Vec<&str> = l.split_whitespace().collect();
        (w.first() == Some(&"kill")).then(|| w.get(1)?.parse().ok()).flatten()
    }).collect();
    let mine: Vec<i32> = t.procs.values().map(|p| p.0).collect();
    let unlisted: Vec<i32> = signalled.iter().copied().filter(|p| mine.contains(p) && !listed.contains_key(p)).collect();
    assert!(unlisted.is_empty(), "signalled but not listed by ps: {unlisted:?}\nlog:\n{}", o.log);
    let all_gone = wait_until(5, || gone.iter().all(|n| !t.alive(n)));
    assert_eq!(o.code, Some(0), "log:\n{}", o.log);
    assert!(all_gone, "alive: {:?}", gone.iter().filter(|n| t.alive(n)).collect::<Vec<_>>());
    // (the leader is the target's parent: it counts the target's SIGCHLD, so only its life is
    // checked)
    for n in ["early", "n", "g2", "g3"] {
        if t.procs.contains_key(n) {
            assert!(t.untouched(n), "{n} got {} signal(s), alive {}", signals(&d, n), t.alive(n));
        }
    }
    assert!(t.alive("leader"), "the leader was killed");
    drop(t);
    let _ = std::fs::remove_dir_all(&d);
}

/// A process whose command line holds control characters (a newline, a tab, an escape) cannot
/// forge rows or reach the terminal through `ps`'s text output: one row, and no control byte but
/// the column tabs and the line end.
#[test]
fn ps_text_output_holds_no_control_bytes() {
    let _serial = serial();
    use std::os::unix::process::CommandExt;
    let d = scratch("ctl");
    let r = d.join("x");
    struct Guard(std::process::Child);
    impl Drop for Guard {
        fn drop(&mut self) {
            common::send_child(&mut self.0, libc::SIGKILL);
            let _ = self.0.wait();
        }
    }
    let c = Guard(Command::new(fixture()).arg0("sl\n4242\tfake\tproved\x1b[2Kx\u{9b}y\u{7f}").args(["sigcount", r.to_str().unwrap()]).spawn().unwrap());
    assert!(wait_until(10, || record(&d, "x").is_some()));
    let o = sd(&d, &["ps", &c.0.id().to_string()], &[]);
    drop(c);
    let lines: Vec<&str> = o.out.lines().collect();
    assert_eq!(lines.len(), 1, "one process, one row:\n{}", o.out);
    let bad: Vec<char> = o.out.chars().filter(|&ch| (ch < ' ' && ch != '\t' && ch != '\n') || ch == '\u{7f}' || ('\u{80}'..='\u{9f}').contains(&ch)).collect();
    assert!(bad.is_empty(), "control characters in the text output: {bad:?}");
    let _ = std::fs::remove_dir_all(&d);
}

/// The root of another live `sheepr run` job in the target's session, started after it, is
/// that job's member, never a suspect; nor is that job's orphan that stays in the session (its
/// parent exited; on macOS its responsible process is the job's supervisor). The orphan is the
/// live-job rule's witness, on macOS (the root is excluded by the parent-PID-1 rule too, and on
/// Linux the job's supervisor, a subreaper, adopts the orphan).
#[test]
fn a_live_jobs_root_in_the_session_is_no_suspect() {
    let _serial = serial();
    let d = scratch("jobroot");
    let t = d.join("t");
    let script = format!(
        r#""$0" sigcount "{}" & sleep 0.1; exec "$1" run -- /bin/sh -c '"$0" stray "$1" js && exec "$0" sigcount "$1/root"' "$0" "{}""#,
        t.display(),
        d.display()
    );
    let mut leader = Command::new(fixture()).args(["new-session", "/bin/sh", "-c", &script, fixture(), sheepr()]).stdin(Stdio::null()).spawn().unwrap();
    struct Recs<'a>(&'a Path);
    impl Drop for Recs<'_> {
        fn drop(&mut self) {
            for n in ["t", "root", "js"] {
                if let Some(p) = record(self.0, n) {
                    common::send(p.0, p.1, libc::SIGKILL);
                }
            }
        }
    }
    let _g = Recs(&d);
    assert!(wait_until(15, || record(&d, "t").is_some() && record(&d, "root").is_some() && record(&d, "js").is_some()), "the tree did not start");
    let (tp, rp, js) = (record(&d, "t").unwrap(), record(&d, "root").unwrap(), record(&d, "js").unwrap());
    let o = sd(&d, &["ps", "--json", &tp.0.to_string()], &[]);
    let r = rows(&o.out);
    common::send(tp.0, tp.1, libc::SIGKILL);
    common::send_child(&mut leader, libc::SIGTERM);
    let _ = leader.wait();
    common::send(rp.0, rp.1, libc::SIGKILL);
    common::send(js.0, js.1, libc::SIGKILL);
    assert_eq!(r.get(&tp.0).map(|x| x.0.as_str()), Some("target"), "{}", o.out);
    assert!(!r.contains_key(&rp.0), "a live job's root is listed:\n{}", o.out);
    assert!(!r.contains_key(&js.0), "a live job's orphan is listed:\n{}", o.out);
    let _ = std::fs::remove_dir_all(&d);
}

/// The wall control for suspects (PHASE2.md §0.1): the suspect is untagged; `kill
/// --include-suspects` turns the latch on before it adds the first suspect, so the door withholds
/// every signal to it (its counter stays 0, a `withheld` line names it).
#[test]
fn including_suspects_turns_the_latch_on() {
    let _serial = serial();
    let d = scratch("latch");
    let t = Tree::build(&d, true);
    let sink = d.join("sink");
    let mut c = Command::new(sheepr());
    c.args(["kill", "--include-suspects", &t.p("t").0.to_string()]).env("SHEEPR_TEST_DEADLINE_MS", "500").stdin(Stdio::null());
    common::cell_sink(&mut c, &sink);
    let _ = c.status();
    let lines = std::fs::read_to_string(&sink).unwrap_or_default();
    let g = t.p("g");
    assert!(t.untouched("g"), "the untagged suspect got {} signal(s)", signals(&d, "g"));
    // withheld lines, every one naming this cell's own untagged suspect or its (untagged) child,
    // which the kill takes with it; none foreign
    let own = [g.0.to_string(), t.p("gc").0.to_string()];
    let held: Vec<&str> = lines.lines().filter(|l| l.starts_with("withheld ")).collect();
    assert!(held.iter().any(|l| l.split_whitespace().nth(1) == Some(own[0].as_str())), "{lines:?}");
    assert!(held.iter().all(|l| l.split_whitespace().nth(1).is_some_and(|p| own.contains(&p.to_string()))), "{lines:?}");
    drop(t);
    let _ = std::fs::remove_dir_all(&d);
}

/// `ps j-XXXX` of a live job lists its supervisor as the target and the job's escapee as proved;
/// of a dead job (its supervisor SIGKILLed), the journal's live members as proved.
#[test]
fn ps_of_a_job_lists_its_members() {
    let _serial = serial();
    let d = scratch("job");
    let s = d.join("state");
    std::fs::create_dir_all(&s).unwrap();
    std::fs::write(s.join(".sheepr-test"), b"").unwrap();
    let r = d.join("esc");
    struct Guard(std::process::Child);
    impl Drop for Guard {
        fn drop(&mut self) {
            common::send_child(&mut self.0, libc::SIGKILL);
            let _ = self.0.wait();
        }
    }
    let mut sup = Guard(
        Command::new(sheepr())
            .args(["run", "--", fixture(), "escapee-and-wait", r.to_str().unwrap()])
            .env("SHEEPR_TEST_STATE", &s)
            .stdin(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let g = {
        assert!(wait_until(15, || record(&d, "esc").is_some()));
        record(&d, "esc").unwrap()
    };
    let job = || -> Option<String> {
        for b in std::fs::read_dir(s.join("jobs")).ok()?.flatten() {
            for f in std::fs::read_dir(b.path()).ok()?.flatten() {
                let n = f.file_name().to_string_lossy().into_owned();
                if let Some(j) = n.strip_suffix(".journal") {
                    return Some(j.to_string());
                }
            }
        }
        None
    };
    assert!(wait_until(10, || job().is_some()));
    let id = job().unwrap();
    // the escapee is journaled by now (the journal names it before it is listed below)
    std::thread::sleep(Duration::from_millis(600));
    let live = sd(&d, &["ps", "--json", &id], &[("SHEEPR_TEST_STATE", s.to_str().unwrap())]);
    common::send_child(&mut sup.0, libc::SIGKILL);
    let _ = sup.0.wait();
    let dead = sd(&d, &["ps", "--json", &id], &[("SHEEPR_TEST_STATE", s.to_str().unwrap())]);
    let (rl, rd) = (rows(&live.out), rows(&dead.out));
    common::send(g.0, g.1, libc::SIGKILL);
    for p in ["esc.root", "esc.g"] {
        if let Some(q) = record(&d, p) {
            common::send(q.0, q.1, libc::SIGKILL);
        }
    }
    assert_eq!(rl.get(&(sup.0.id() as i32)).map(|r| r.0.as_str()), Some("target"), "live:\n{}", live.out);
    assert_eq!(rl.get(&g.0).map(|r| r.0.as_str()), Some("proved"), "live:\n{}", live.out);
    assert_eq!(rd.get(&g.0).map(|r| r.0.as_str()), Some("proved"), "dead:\n{}", dead.out);
    assert!(live.log.is_empty() && dead.log.is_empty(), "ps signalled");
    let _ = std::fs::remove_dir_all(&d);
}

/// A supervisor under a suspect is ended first when the suspects are included (TERM, as for a
/// supervisor in the proved set), so its own kill reaches its job: the signal log names it as a
/// supervisor, and its job's root is gone.
#[test]
fn a_supervisor_under_a_suspect_is_ended_first() {
    let _serial = serial();
    let d = scratch("supsus");
    let t = Tree::build_with(&d, false, true);
    let gc = t.p("gc");
    // the supervisor: gc's parent
    let sup: i32 = String::from_utf8_lossy(&Command::new("ps").args(["-o", "ppid=", "-p", &gc.0.to_string()]).output().unwrap().stdout).trim().parse().unwrap_or(0);
    let supp = common::found(sup);
    let o = sd(&d, &["kill", "--include-suspects", &t.p("t").0.to_string()], &[("SHEEPR_TEST_DEADLINE_MS", "2000")]);
    let root_gone = wait_until(10, || !common::alive(gc));
    if let Some(s) = supp {
        common::send(s.0, s.1, libc::SIGKILL);
    }
    assert!(supp.is_some(), "no supervisor above gc");
    assert!(o.log.lines().any(|l| l == format!("supervisor {sup}")), "the supervisor under the suspect was not ended first:\n{}", o.log);
    assert!(root_gone, "its job's root survived");
    drop(t);
    let _ = std::fs::remove_dir_all(&d);
}
