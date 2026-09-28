//! Phase-2 P1: the journal (PHASE2.md §1 decisions 1-6). Every cell runs with its own test state
//! directory (the sentinel planted), so it reads and writes nothing else.

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
    let d = std::env::temp_dir().join(format!("sd-jr-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A test state directory: `<d>/state` with the sentinel.
fn state(d: &Path) -> PathBuf {
    let s = d.join("state");
    std::fs::create_dir_all(&s).unwrap();
    std::fs::write(s.join(".sheepdog-test"), b"").unwrap();
    s
}

/// Every published journal under a state directory (never a temporary file).
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

fn wait_for(p: &Path, secs: u64) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    while !p.exists() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(5));
    }
    p.exists()
}

fn finish(mut c: std::process::Child) -> Option<i32> {
    let end = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(s) = c.try_wait().unwrap() {
            return s.code();
        }
        if Instant::now() > end {
            common::send_child(&mut c, libc::SIGKILL);
            let _ = c.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn records(r: &Path, n: usize) -> Vec<(i32, u64)> {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        let v: Vec<(i32, u64)> = std::fs::read_to_string(r)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| {
                let mut w = l.split_whitespace();
                Some((w.next()?.parse().ok()?, w.next()?.parse().ok()?))
            })
            .collect();
        if v.len() >= n || Instant::now() > end {
            return v;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The parsed lines of a journal; panics on a line that does not parse.
fn lines(j: &Path) -> Vec<Json> {
    std::fs::read_to_string(j).unwrap().lines().map(|l| json::parse(l).unwrap_or_else(|e| panic!("{e}: {l}"))).collect()
}

fn marker() -> String {
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos();
    format!("29.{:09}", n)
}

/// A member's journal line is written before the first signal to it: in the signal log, every
/// pid that is signalled has a `journal <pid>` line before its first signal line. The shape: a
/// member that forks an escapee when it gets TERM (`fork-on-term`), so the escapee is born during
/// the kill's grace, is first seen by a grace scan, and gets its TERM right after that very scan
/// (a journal written one scan late would come after it). The job is ended by a TERM to sheepdog.
#[test]
fn a_member_is_journaled_before_its_first_signal() {
    let d = scratch("order");
    let s = state(&d);
    let r = d.join("rec");
    let log = d.join("log");
    let mut c = Command::new(sheepdog())
        .args(["run", "--grace", "0.5", "--", fixture(), "fork-on-term", &marker()])
        .arg(&r)
        .env("SHEEPDOG_TEST_STATE", &s)
        .env("SHEEPDOG_TEST_SIGNAL_LOG", &log)
        .spawn()
        .unwrap();
    let started = records(&r, 2).len() == 2;
    common::send_child(&mut c, libc::SIGTERM);
    let _ = finish(c);
    let recs = records(&r, 3);
    for p in recs.iter().filter(|p| common::alive(**p)) {
        common::send(p.0, p.1, libc::SIGKILL);
    }
    assert!(started, "the member was ready");
    assert_eq!(recs.len(), 3, "the escapee was born during the kill: {recs:?}");
    let text = std::fs::read_to_string(&log).unwrap();
    let mut journaled = std::collections::HashSet::new();
    let mut checked = std::collections::HashSet::new();
    for l in text.lines() {
        let w: Vec<&str> = l.split_whitespace().collect();
        match w.as_slice() {
            ["journal", p, ..] => {
                journaled.insert(p.to_string());
            }
            [k, p, ..] if *k == "kill" || *k == "pidfd" => {
                assert!(journaled.contains(*p), "pid {p} was signalled before it was journaled:\n{text}");
                checked.insert(p.to_string());
            }
            _ => {}
        }
    }
    assert!(checked.contains(&recs[2].0.to_string()), "control: the escapee born in the kill was signalled:\n{text}");
    let _ = std::fs::remove_dir_all(&d);
}

/// The journal exists while the job runs (it names the root and the escapee, and its header is
/// the first line), and is deleted after a clean end.
#[test]
fn the_journal_exists_while_the_job_runs_and_is_deleted_after_a_clean_end() {
    let d = scratch("life");
    let s = state(&d);
    let (go, ready) = (d.join("go"), d.join("ready"));
    let script = format!(r#"touch "{}"; while [ ! -e "{}" ]; do sleep 0.01; done"#, ready.display(), go.display());
    let c = Command::new(sheepdog()).args(["run", "--", "/bin/sh", "-c", &script]).env("SHEEPDOG_TEST_STATE", &s).spawn().unwrap();
    let sup = c.id() as i32;
    let started = wait_for(&ready, 20);
    let js = journals(&s);
    let text = js.first().map(|j| std::fs::read_to_string(j).unwrap_or_default());
    std::fs::write(&go, b"").unwrap(); // release the job before any assertion
    let code = finish(c);
    assert!(started, "the root ran");
    assert_eq!(js.len(), 1, "{js:?}");
    let ls: Vec<Json> = text.unwrap().lines().map(|l| json::parse(l).unwrap()).collect();
    assert_eq!(ls[0].get("kind").and_then(Json::str), Some("header"));
    assert_eq!(ls[0].get("sup").and_then(|x| x.get("pid")).and_then(Json::num), Some(sup as f64), "the header names the supervisor");
    let job = ls[0].get("job").and_then(Json::str).unwrap().to_string();
    assert!(job.starts_with("j-") && job.len() == 10 && js[0].file_stem().unwrap() == job.as_str(), "{job}");
    assert!(ls[1..].iter().any(|l| l.get("root") == Some(&Json::Bool(true))), "the root is journaled");
    assert_eq!(code, Some(0));
    assert!(journals(&s).is_empty(), "deleted after a clean end");
    let _ = std::fs::remove_dir_all(&d);
}

/// A journal is never visible unlocked: held just after it is published, the reader finds it
/// locked, and its header already there.
#[test]
fn a_published_journal_is_locked_and_has_its_header() {
    let d = scratch("locked");
    let s = state(&d);
    let (rel, ready) = (d.join("release"), d.join("ready"));
    let c = Command::new(sheepdog())
        .args(["run", "--", "/bin/sh", "-c", "exit 0"])
        .env("SHEEPDOG_TEST_STATE", &s)
        .env("SHEEPDOG_TEST_HOLD_AFTER_LINK", &rel)
        .env("SHEEPDOG_TEST_READY_FILE", &ready)
        .spawn()
        .unwrap();
    let held = wait_for(&ready, 20);
    let js = journals(&s);
    let verdict = js.first().map(|j| {
        let f = std::fs::File::open(j).unwrap();
        let locked = unsafe { libc::flock(std::os::fd::AsRawFd::as_raw_fd(&f), libc::LOCK_EX | libc::LOCK_NB) } != 0;
        let header = std::fs::read_to_string(j).unwrap().lines().next().and_then(|l| json::parse(l).ok());
        (locked, header.and_then(|h| h.get("kind").and_then(Json::str).map(String::from)))
    });
    std::fs::write(&rel, b"").unwrap();
    assert_eq!(finish(c), Some(0));
    assert!(held, "held after the link");
    assert_eq!(js.len(), 1);
    assert_eq!(verdict, Some((true, Some("header".to_string()))), "locked, with its header");
    let _ = std::fs::remove_dir_all(&d);
}

/// The job holds no journal or lock fd: the root's own open files name nothing in the state
/// directory, while the supervisor's do (the control: the listing can see such an fd).
#[test]
fn the_job_holds_no_journal_fd() {
    let d = scratch("fds");
    let s = state(&d);
    let (own, sup) = (d.join("own"), d.join("sup"));
    let list = if cfg!(target_os = "macos") { "lsof -p {} -Fn" } else { "ls -l /proc/{}/fd/" };
    let script = format!(
        r#"{} > "{}" 2>/dev/null; {} > "{}" 2>/dev/null; exit 0"#,
        list.replace("{}", "$$"),
        own.display(),
        list.replace("{}", "$PPID"),
        sup.display()
    );
    let code = finish(Command::new(sheepdog()).args(["run", "--", "/bin/sh", "-c", &script]).env("SHEEPDOG_TEST_STATE", &s).spawn().unwrap());
    assert_eq!(code, Some(0));
    let (own, sup) = (std::fs::read_to_string(&own).unwrap(), std::fs::read_to_string(&sup).unwrap());
    // the fd is listed by the name it was opened under (the temporary one, now removed) or by
    // the journal's own name
    let journal_fd = |t: &str| t.contains(".tmp-j-") || t.contains(".journal");
    assert!(journal_fd(&sup), "control: the supervisor's listing shows its journal:\n{sup}");
    assert!(!journal_fd(&own), "the root holds a journal fd:\n{own}");
    let _ = std::fs::remove_dir_all(&d);
}

/// Decision 2: the journal never stops the job. With no usable state directory (`jobs` is a
/// file) and with a write that fails (seam), the escapee still dies, the exit code is the
/// command's, and the run notes the failure.
#[test]
fn a_journal_that_cannot_be_written_never_stops_the_kill() {
    for (name, seam) in [("nodir", None), ("write", Some("SHEEPDOG_TEST_JOURNAL_WRITE_FAIL"))] {
        let d = scratch(name);
        let s = state(&d);
        if seam.is_none() {
            std::fs::write(s.join("jobs"), b"not a directory").unwrap();
        }
        let r = d.join("rec");
        let trace = d.join("trace");
        let mut cmd = Command::new(sheepdog());
        cmd.args(["run", "--grace", "0", "--", "/bin/sh", "-c", &format!(r#""$FX" escape {} "$R"; exit 3"#, marker())])
            .env("FX", fixture())
            .env("R", &r)
            .env("SHEEPDOG_TEST_STATE", &s)
            .env("SHEEPDOG_TEST_TRACE", &trace);
        if let Some(k) = seam {
            cmd.env(k, "1");
        }
        let code = finish(cmd.spawn().unwrap());
        let g = records(&r, 1)[0];
        let alive = common::alive(g);
        common::send(g.0, g.1, libc::SIGKILL);
        assert!(!alive, "{name}: the escapee died");
        assert_eq!(code, Some(3), "{name}: the command's own code");
        let notes = std::fs::read_to_string(&trace).unwrap_or_default();
        assert!(notes.lines().any(|l| l.starts_with("journal-failed")), "{name}: {notes}");
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// A scan with many new members writes one complete line per member (no count cap): 300
/// members, every line parses, every member is named.
#[test]
fn every_member_of_a_big_scan_has_a_complete_line() {
    let d = scratch("big");
    let s = state(&d);
    let r = d.join("rec");
    let code = finish(
        Command::new(sheepdog())
            .args(["run", "--grace", "0.05", "--", fixture(), "swarm", "300"])
            .arg(&r)
            .env("SHEEPDOG_TEST_STATE", &s)
            .env("SHEEPDOG_TEST_KEEP_JOURNAL", "1")
            .spawn()
            .unwrap(),
    );
    let recs = records(&r, 300);
    for p in recs.iter().filter(|p| common::alive(**p)) {
        common::send(p.0, p.1, libc::SIGKILL);
    }
    assert_eq!(code, Some(0));
    let js = journals(&s);
    assert_eq!(js.len(), 1, "kept by the seam");
    let ls = lines(&js[0]);
    let named: std::collections::HashSet<i64> = ls.iter().filter_map(|l| l.get("pid").and_then(Json::num)).map(|n| n as i64).collect();
    let missing: Vec<_> = recs.iter().filter(|p| !named.contains(&(p.0 as i64))).collect();
    assert_eq!(recs.len(), 300);
    assert!(missing.is_empty(), "{} members not journaled", missing.len());
    let _ = std::fs::remove_dir_all(&d);
}

/// A non-UTF-8 argument is kept in the line, escaped as \xHH, and the line still parses.
#[test]
fn a_non_utf8_argument_is_escaped() {
    use std::os::unix::ffi::OsStrExt;
    let d = scratch("bytes");
    let s = state(&d);
    let bad = std::ffi::OsStr::from_bytes(b"a\xff\xfeb");
    let code = finish(
        Command::new(sheepdog())
            .args(["run", "--", "/bin/sh", "-c", "exit 0"])
            .arg(bad)
            .env("SHEEPDOG_TEST_STATE", &s)
            .env("SHEEPDOG_TEST_KEEP_JOURNAL", "1")
            .spawn()
            .unwrap(),
    );
    assert_eq!(code, Some(0));
    let ls = lines(&journals(&s)[0]);
    let root = ls.iter().find(|l| l.get("root") == Some(&Json::Bool(true))).expect("the root line");
    let cmd = root.get("cmd").and_then(Json::str).unwrap();
    assert!(cmd.contains(r"a\xff\xfeb"), "{cmd}");
    let _ = std::fs::remove_dir_all(&d);
}

/// Publishing never replaces a journal: when the chosen id is taken, a new id is picked and the
/// existing file is left as it was. (`--no-sweep`: the first run's kept journal is a dead job's,
/// which the second run's auto-sweep would otherwise remove first.)
#[test]
fn a_taken_job_id_is_never_overwritten() {
    let d = scratch("taken");
    let s = state(&d);
    // a first run (kept) shows the folder; then a second run forced to the same id
    let first = finish(
        Command::new(sheepdog())
            .args(["run", "--no-sweep", "--", "/bin/sh", "-c", "exit 0"])
            .env("SHEEPDOG_TEST_STATE", &s)
            .env("SHEEPDOG_TEST_KEEP_JOURNAL", "1")
            .env("SHEEPDOG_TEST_JOB_ID", "0badc0de")
            .spawn()
            .unwrap(),
    );
    assert_eq!(first, Some(0));
    let js = journals(&s);
    assert_eq!(js.len(), 1);
    assert_eq!(js[0].file_stem().unwrap(), "j-0badc0de", "the forced id");
    let before = std::fs::read(&js[0]).unwrap();
    let second = finish(
        Command::new(sheepdog())
            .args(["run", "--no-sweep", "--", "/bin/sh", "-c", "exit 0"])
            .env("SHEEPDOG_TEST_STATE", &s)
            .env("SHEEPDOG_TEST_KEEP_JOURNAL", "1")
            .env("SHEEPDOG_TEST_JOB_ID", "0badc0de")
            .spawn()
            .unwrap(),
    );
    assert_eq!(second, Some(0));
    assert_eq!(std::fs::read(&js[0]).unwrap(), before, "the taken journal is unchanged");
    let js2 = journals(&s);
    assert_eq!(js2.len(), 2, "a second journal under a new id: {js2:?}");
    let _ = std::fs::remove_dir_all(&d);
}

/// The state wall, live (PHASE2.md §0.2): a debug run whose environment names the operator's
/// state (`SHEEPDOG_STATE`, `XDG_STATE_HOME` and `HOME` all pointing at a directory that even has
/// a sentinel) writes nothing there and notes that it has no state: with the runner's canary as
/// its test state (no sentinel), and with no test state at all. The control is every cell above:
/// with a sentinelled test state it writes a journal.
#[test]
fn a_debug_run_never_writes_the_operators_state() {
    for canary in [true, false] {
        let d = scratch(if canary { "wall-canary" } else { "wall-none" });
        let real = d.join("real");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join(".sheepdog-test"), b"").unwrap();
        let trace = d.join("trace");
        let mut cmd = Command::new(sheepdog());
        cmd.args(["run", "--", "/bin/sh", "-c", "exit 0"])
            .env("SHEEPDOG_STATE", &real)
            .env("XDG_STATE_HOME", &real)
            .env("HOME", &real)
            .env("SHEEPDOG_TEST_KEEP_JOURNAL", "1")
            .env("SHEEPDOG_TEST_TRACE", &trace);
        if !canary {
            cmd.env_remove("SHEEPDOG_TEST_STATE");
        }
        let code = finish(cmd.spawn().unwrap());
        assert_eq!(code, Some(0));
        // everything but the sentinel and the one known outside writer: Rosetta keeps its cache in
        // $HOME/.cache/rosetta on the emulated leg
        let written: Vec<_> = walk(&real)
            .into_iter()
            .filter(|p| {
                let r = p.strip_prefix(&real).unwrap().to_string_lossy().into_owned();
                r != ".sheepdog-test" && r != ".cache" && r != ".cache/rosetta" && !r.starts_with(".cache/rosetta/")
            })
            .collect();
        assert!(written.is_empty(), "canary {canary}: {written:?}");
        let notes = std::fs::read_to_string(&trace).unwrap_or_default();
        assert!(notes.lines().any(|l| l == "state-unset"), "canary {canary}: {notes}");
        let _ = std::fs::remove_dir_all(&d);
    }
}

fn walk(p: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    for e in std::fs::read_dir(p).into_iter().flatten().flatten() {
        v.push(e.path());
        if e.path().is_dir() {
            v.extend(walk(&e.path()));
        }
    }
    v
}

/// The root is journaled before it runs (macOS: before the resuming CONT; Linux: before the root
/// shim's go byte). A seam SIGKILLs the supervisor right after the root's line: the journal names
/// the root, and the root never ran its command.
#[test]
fn the_root_is_journaled_before_it_runs() {
    let d = scratch("rootfirst");
    let s = state(&d);
    let ran = d.join("ran");
    let code = finish(
        Command::new(sheepdog())
            .args(["run", "--", "/bin/sh", "-c", &format!(r#"touch "{}""#, ran.display())])
            .env("SHEEPDOG_TEST_STATE", &s)
            .env("SHEEPDOG_TEST_KILL_AFTER_ROOT_JOURNAL", "1")
            // Linux: PDEATHSIG would kill the shim anyway; off, only the go byte holds it back
            .env("SHEEPDOG_TEST_SHIM_NO_PDEATHSIG", "1")
            .spawn()
            .unwrap(),
    );
    assert_eq!(code, None, "the supervisor died of the seam's SIGKILL");
    let js = journals(&s);
    assert_eq!(js.len(), 1, "the journal stays (its supervisor died)");
    let ls = lines(&js[0]);
    let root = ls.iter().find(|l| l.get("root") == Some(&Json::Bool(true))).expect("the root was journaled");
    let (p, id) = (root.get("pid").and_then(Json::num).unwrap() as i32, root.get("id").and_then(Json::num).unwrap() as u64);
    std::thread::sleep(Duration::from_millis(300));
    let never_ran = !ran.exists();
    common::send(p, id, libc::SIGKILL); // the suspended root, by the identity the journal gave
    assert!(never_ran, "the root ran before it was journaled");
    let _ = std::fs::remove_dir_all(&d);
}

/// `--status-fd`, minimal (the full schema is P2): `root` says how the root ended, `job` names
/// the journal's id, and a journal failure is in `notes`.
#[test]
fn the_status_line_names_the_root_the_job_and_the_notes() {
    let d = scratch("status");
    let s = state(&d);
    let run = |cmd: &str, extra: &[(&str, &str)]| -> Json {
        let out = d.join("status");
        let _ = std::fs::remove_file(&out);
        let mut c = Command::new("/bin/sh");
        c.args(["-c", &format!(r#"exec "$SD" run --status-fd 3 -- /bin/sh -c '{cmd}' 3>"{}""#, out.display())])
            .env("SD", sheepdog())
            .env("SHEEPDOG_TEST_STATE", &s);
        for (k, v) in extra {
            c.env(k, v);
        }
        finish(c.spawn().unwrap());
        json::parse(std::fs::read_to_string(&out).unwrap().trim_end()).unwrap()
    };
    let ok = run("exit 4", &[]);
    assert_eq!(ok.get("root").and_then(Json::str), Some("exited"));
    assert!(ok.get("job").and_then(Json::str).is_some_and(|j| j.starts_with("j-")));
    let sig = run("kill -KILL $$", &[]);
    assert_eq!(sig.get("root").and_then(Json::str), Some("signaled"));
    let failed = run("exit 0", &[("SHEEPDOG_TEST_JOURNAL_WRITE_FAIL", "1")]);
    let notes: Vec<&str> = failed.get("notes").and_then(Json::arr).unwrap_or(&[]).iter().filter_map(Json::str).collect();
    assert!(notes.iter().any(|n| n.starts_with("journal-failed")), "{notes:?}");
    // a root that never started (not found: 127)
    let out = d.join("status-missing");
    let code = finish(
        Command::new("/bin/sh")
            .args(["-c", &format!(r#"exec "$SD" run --status-fd 3 -- /nonexistent/cmd 3>"{}""#, out.display())])
            .env("SD", sheepdog())
            .env("SHEEPDOG_TEST_STATE", &s)
            .spawn()
            .unwrap(),
    );
    assert_eq!(code, Some(127));
    let st = json::parse(std::fs::read_to_string(&out).unwrap().trim_end()).unwrap();
    assert_eq!(st.get("root").and_then(Json::str), Some("not-started"));
    let _ = std::fs::remove_dir_all(&d);
}

/// A running job's journal is locked for as long as the job runs, also after a journal write
/// failed (the published file must never look like a dead job's to `sweep`), and no journal is
/// left after a clean end. Probed mid-run, while the root waits.
#[test]
fn a_running_job_s_journal_stays_locked() {
    for fail in [false, true] {
        let d = scratch(if fail { "lockfail" } else { "lockok" });
        let s = state(&d);
        let (go, ready) = (d.join("go"), d.join("ready"));
        // the root starts an escapee, so a scan has a member line to write (and fail on)
        let script = format!(
            r#""$FX" escape {} "$R"; touch "{}"; while [ ! -e "{}" ]; do sleep 0.01; done"#,
            marker(),
            ready.display(),
            go.display()
        );
        let mut cmd = Command::new(sheepdog());
        cmd.args(["run", "--grace", "0", "--", "/bin/sh", "-c", &script]).env("FX", fixture()).env("R", d.join("rec")).env("SHEEPDOG_TEST_STATE", &s);
        if fail {
            cmd.env("SHEEPDOG_TEST_JOURNAL_WRITE_FAIL", "1");
        }
        let c = cmd.spawn().unwrap();
        let started = wait_for(&ready, 20);
        std::thread::sleep(Duration::from_millis(600)); // a few scans (and failed writes)
        let unlocked: Vec<PathBuf> = journals(&s)
            .into_iter()
            .filter(|j| {
                let f = std::fs::File::open(j).unwrap();
                unsafe { libc::flock(std::os::fd::AsRawFd::as_raw_fd(&f), libc::LOCK_EX | libc::LOCK_NB) == 0 }
            })
            .collect();
        let published = journals(&s).len();
        std::fs::write(&go, b"").unwrap();
        let code = finish(c);
        for p in records(&d.join("rec"), 1) {
            common::send(p.0, p.1, libc::SIGKILL);
        }
        assert!(started, "fail {fail}");
        assert_eq!(published, 1, "fail {fail}: the journal is published (its header is written before any line)");
        assert!(unlocked.is_empty(), "fail {fail}: a running job's journal is unlocked: {unlocked:?}");
        assert_eq!(code, Some(0), "fail {fail}");
        assert!(journals(&s).is_empty(), "fail {fail}: a journal is left after a clean end");
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// `--status-fd` gets exactly one line, from the supervisor, also when sheepdog runs with a
/// relay (it started with children of its own: here the shell's background `sleep`), and also
/// when the job ends by a TERM to sheepdog. The relay never writes it.
#[test]
fn the_status_line_is_written_once_with_a_relay() {
    let d = scratch("relay");
    let s = state(&d);
    let m = marker();
    for term in [false, true] {
        let (out, ready) = (d.join(format!("status-{term}")), d.join(format!("ready-{term}")));
        let root = if term { format!(r#"touch "{}"; sleep 30"#, ready.display()) } else { "exit 0".to_string() };
        let mut c = Command::new("/bin/sh")
            .args(["-c", &format!(r#"sleep {m} & exec "$SD" run --status-fd 3 -- /bin/sh -c '{root}' 3>"{}""#, out.display())])
            .env("SD", sheepdog())
            .env("SHEEPDOG_TEST_STATE", &s)
            .spawn()
            .unwrap();
        if term {
            let started = wait_for(&ready, 20);
            common::send_child(&mut c, libc::SIGTERM);
            assert!(started);
        }
        let _ = finish(c);
        let text = std::fs::read_to_string(&out).unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        common::kill_marked(&[&m]);
        assert_eq!(lines.len(), 1, "term {term}: {text:?}");
        let j = json::parse(lines[0]).unwrap();
        assert!(j.get("job").and_then(Json::str).is_some_and(|x| x.starts_with("j-")), "term {term}: {text}");
    }
    let _ = std::fs::remove_dir_all(&d);
}

/// Under the phase-1 opt-out (PHASE2.md §0.1) no journal is written: the opt-out disables the
/// state outright. The control is every cell above, where the same run writes one.
#[test]
fn the_phase_one_opt_out_writes_no_journal() {
    let d = scratch("optout");
    let s = state(&d);
    let code = finish(
        Command::new(sheepdog())
            .args(["run", "--", "/bin/sh", "-c", "exit 0"])
            .env("SHEEPDOG_TEST_STATE", &s)
            .env("SHEEPDOG_TEST_KEEP_JOURNAL", "1")
            .env("SHEEPDOG_TEST_PHASE1", "1")
            .spawn()
            .unwrap(),
    );
    assert_eq!(code, Some(0));
    assert!(!s.join("jobs").exists(), "a journal was written under the opt-out");
    let _ = std::fs::remove_dir_all(&d);
}

/// A journal that could not keep its lines under `--leave-strays` is not left behind without its
/// leave-strays mark (a sweep would take it for a dead job and kill the strays the operator chose
/// to keep). With a failed write, no journal remains; the control, the same run without the
/// failure, keeps one that carries the mark.
#[test]
fn a_leave_strays_journal_is_never_kept_without_its_mark() {
    for fail in [false, true] {
        let d = scratch(if fail { "strays-fail" } else { "strays-ok" });
        let s = state(&d);
        let m = marker();
        let mut cmd = Command::new(sheepdog());
        cmd.args(["run", "--leave-strays", "--", fixture(), "escape", &m]).arg(d.join("rec")).env("SHEEPDOG_TEST_STATE", &s);
        if fail {
            cmd.env("SHEEPDOG_TEST_JOURNAL_WRITE_FAIL", "1");
        }
        let code = finish(cmd.spawn().unwrap());
        for p in records(&d.join("rec"), 1) {
            common::send(p.0, p.1, libc::SIGKILL); // the stray it left
        }
        let js = journals(&s);
        assert_eq!(code, Some(0), "fail {fail}");
        if fail {
            assert!(js.is_empty(), "a journal without its mark was kept: {js:?}");
        } else {
            assert_eq!(js.len(), 1, "control: the leave-strays journal is kept");
            let marked = lines(&js[0]).iter().any(|l| l.get("kind").and_then(Json::str) == Some("leave-strays"));
            assert!(marked, "control: it carries the mark");
        }
        let _ = std::fs::remove_dir_all(&d);
    }
}

