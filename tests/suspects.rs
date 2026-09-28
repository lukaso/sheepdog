//! Phase-2 P6: suspects in `sheepdog kill <pid>` and `sheepdog ps` (PLAN.md §3.0, PHASE2.md D7).
//!
//! Every tree is built by the fixture in a session and group the test made (`new-session`), so a
//! suspect rule that widens reaches only this cell's processes, never the test runner's session.
//! The macOS `puniq` link is not bound to a session: any orphan whose parent is dead, started
//! after the target, with the same responsible process (every process under one terminal app)
//! matches it. So the cells run one at a time (their trees would be each other's suspects), a
//! cell that includes suspects has a sink of its own (a foreign process that matches is
//! withheld by the wall, which is right, and must not fail the run), and assertions look only at
//! the cell's own processes.
//! The tree (`suspect-tree`): an orphan started before the target (`early`), the target (`t`), an
//! orphan of the target that stays in its session (`g`, a suspect on both OSes), one in a session
//! of its own (`g2`, a suspect on macOS only, by `puniq` and the same responsible process), the
//! same but responsible for itself (`g3`, macOS: no suspect), a non-orphan started after the
//! target (`n`), and the session leader (`leader`). Each counts its signals in `<name>.sig`.

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

fn sheepdog() -> &'static str {
    common::test_env();
    env!("CARGO_BIN_EXE_sheepdog")
}
fn fixture() -> &'static str {
    common::test_env();
    env!("CARGO_BIN_EXE_sd-fixture")
}

const NAMES: [&str; 7] = ["early", "t", "g", "g2", "g3", "n", "leader"];

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("sd-sus-{name}-{}", std::process::id()));
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
        let mut c = Command::new(fixture());
        c.args(["new-session", fixture(), "suspect-tree", d.to_str().unwrap()]).stdin(Stdio::null());
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

/// `sheepdog ARGS` with the signal log, 60 s bound.
fn sd(d: &Path, args: &[&str], env: &[(&str, &str)]) -> Out {
    let log = d.join(format!("log-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    let mut c = Command::new(sheepdog());
    c.args(args).env("SHEEPDOG_TEST_SIGNAL_LOG", &log).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
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
    let mut want_suspects = vec!["g"];
    if cfg!(target_os = "macos") {
        want_suspects.push("g2");
    }
    let suspects: Vec<&str> = NAMES.iter().copied().filter(|n| t.procs.contains_key(n) && class(n).as_deref() == Some("suspect")).collect();
    assert_eq!(o.code, Some(0), "{}", o.out);
    assert_eq!(class("t").as_deref(), Some("target"), "{}", o.out);
    assert_eq!(suspects, want_suspects, "rows:\n{}", o.out);
    for n in ["early", "n", "leader", "g3"] {
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
    let sink = d.join("sink");
    let o = sd(&d, &["kill", "--include-suspects", &t.p("t").0.to_string()], &[("SHEEPDOG_TEST_SINK", sink.to_str().unwrap()), ("SHEEPDOG_TEST_DEADLINE_MS", "2000")]);
    let mut gone = vec!["t", "g"];
    if cfg!(target_os = "macos") {
        gone.push("g2");
    }
    let all_gone = wait_until(5, || gone.iter().all(|n| !t.alive(n)));
    // a withheld line names a foreign suspect (the wall kept it); never one of this tree
    let held: Vec<i32> = std::fs::read_to_string(&sink).unwrap_or_default().lines().filter_map(|l| l.split_whitespace().nth(1)?.parse().ok()).collect();
    assert!(held.iter().all(|p| !t.procs.values().any(|&(q, _)| q == *p)), "a tree process was withheld: {held:?}");
    assert!(o.code == Some(0) || (o.code == Some(125) && !held.is_empty()), "exit {:?}, withheld {held:?}", o.code);
    assert!(all_gone, "alive: {:?}", gone.iter().filter(|n| t.alive(n)).collect::<Vec<_>>());
    // (the leader is the target's parent: it counts the target's SIGCHLD, so only its life is
    // checked)
    for n in ["early", "n", "g3"] {
        if t.procs.contains_key(n) {
            assert!(t.untouched(n), "{n} got {} signal(s), alive {}", signals(&d, n), t.alive(n));
        }
    }
    assert!(t.alive("leader"), "the leader was killed");
    drop(t);
    let _ = std::fs::remove_dir_all(&d);
}

/// `kill --dry-run` lists the same rows as `ps`.
#[test]
fn dry_run_lists_what_ps_lists() {
    let _serial = serial();
    let d = scratch("dry");
    let t = Tree::build(&d, false);
    let tp = t.p("t").0.to_string();
    let a = sd(&d, &["ps", "--json", &tp], &[]);
    let b = sd(&d, &["kill", "--dry-run", "--json", &tp], &[]);
    let (ra, rb) = (rows(&a.out), rows(&b.out));
    assert!(!ra.is_empty());
    assert_eq!(ra.keys().collect::<std::collections::BTreeSet<_>>(), rb.keys().collect::<std::collections::BTreeSet<_>>());
    assert!(b.log.is_empty(), "the dry run signalled: {}", b.log);
    drop(t);
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
    let mut c = Command::new(sheepdog());
    c.args(["kill", "--include-suspects", &t.p("t").0.to_string()]).env("SHEEPDOG_TEST_DEADLINE_MS", "500").stdin(Stdio::null());
    common::cell_sink(&mut c, &sink);
    let _ = c.status();
    let lines = std::fs::read_to_string(&sink).unwrap_or_default();
    let g = t.p("g");
    assert!(t.untouched("g"), "the untagged suspect got {} signal(s)", signals(&d, "g"));
    assert!(lines.lines().any(|l| l.starts_with("withheld ") && l.split_whitespace().nth(1) == Some(&g.0.to_string())), "{lines:?}");
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
    std::fs::write(s.join(".sheepdog-test"), b"").unwrap();
    let r = d.join("esc");
    let mut sup = Command::new(sheepdog())
        .args(["run", "--", fixture(), "escapee-and-wait", r.to_str().unwrap()])
        .env("SHEEPDOG_TEST_STATE", &s)
        .stdin(Stdio::null())
        .spawn()
        .unwrap();
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
    let live = sd(&d, &["ps", "--json", &id], &[("SHEEPDOG_TEST_STATE", s.to_str().unwrap())]);
    common::send_child(&mut sup, libc::SIGKILL);
    let _ = sup.wait();
    let dead = sd(&d, &["ps", "--json", &id], &[("SHEEPDOG_TEST_STATE", s.to_str().unwrap())]);
    let (rl, rd) = (rows(&live.out), rows(&dead.out));
    common::send(g.0, g.1, libc::SIGKILL);
    for p in ["esc.root", "esc.g"] {
        if let Some(q) = record(&d, p) {
            common::send(q.0, q.1, libc::SIGKILL);
        }
    }
    assert_eq!(rl.get(&(sup.id() as i32)).map(|r| r.0.as_str()), Some("target"), "live:\n{}", live.out);
    assert_eq!(rl.get(&g.0).map(|r| r.0.as_str()), Some("proved"), "live:\n{}", live.out);
    assert_eq!(rd.get(&g.0).map(|r| r.0.as_str()), Some("proved"), "dead:\n{}", dead.out);
    assert!(live.log.is_empty() && dead.log.is_empty(), "ps signalled");
    let _ = std::fs::remove_dir_all(&d);
}
