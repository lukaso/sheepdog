//! Phase-2 P4a: `sheepr sweep` (PLAN.md §3.5; PHASE2.md §3 sweep rules). Every cell has its
//! own test state; every process a sweep may aim at is built here from the fixture binary (its
//! environment carries the test tag: the sweep turns the latch on); decoys count every catchable
//! signal they get.

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
    let d = std::env::temp_dir().join(format!("sr-sw-{name}-{}", std::process::id()));
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

/// An identity no process has: added to a real one to forge a mismatch. Never `+ 1`: on macOS
/// the identity is the uniqueid, which is system-wide and sequential, so `+ 1` is the NEXT process
/// created anywhere (another cell's), and a journaled id is a `puniq` link to its children.
const WRONG: u64 = 1 << 40;

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

/// A decoy the test built: `sigcount R` (it counts every catchable signal it gets).
fn decoy(d: &Path, name: &str) -> (Child, (i32, u64), PathBuf) {
    let r = d.join(name);
    let c = Command::new(fixture()).arg("sigcount").arg(&r).spawn().unwrap();
    assert!(wait_until(10, || !records(&r).is_empty()), "the decoy started");
    let p = records(&r)[0];
    (c, p, r)
}

fn end_decoy(c: &mut Child) {
    common::send_child(c, libc::SIGKILL);
    let _ = c.wait();
}

/// `sheepr sweep ARGS` with this state; (exit code, trace notes).
fn sweep(s: &Path, d: &Path, args: &[&str], env: &[(&str, &str)]) -> (Option<i32>, String) {
    let trace = d.join(format!("trace-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    let mut c = Command::new(sheepr());
    c.arg("sweep").args(args).env("SHEEPR_TEST_STATE", s).env("SHEEPR_TEST_TRACE", &trace);
    for (k, v) in env {
        c.env(k, v);
    }
    let mut ch = c.spawn().unwrap();
    let end = Instant::now() + Duration::from_secs(30);
    let code = loop {
        if let Some(st) = ch.try_wait().unwrap() {
            break st.code();
        }
        if Instant::now() > end {
            common::send_child(&mut ch, libc::SIGKILL);
            let _ = ch.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    (code, std::fs::read_to_string(&trace).unwrap_or_default())
}

/// `sheepr sweep ARGS` with this state, within 30 s (then KILL, code None); (exit code, stderr).
fn sweep_said(s: &Path, args: &[&str]) -> (Option<i32>, String) {
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let err = std::env::temp_dir().join(format!("sr-sw-said-{}-{n}.err", std::process::id()));
    let mut c = Command::new(sheepr()).arg("sweep").args(args).env("SHEEPR_TEST_STATE", s).stderr(std::fs::File::create(&err).unwrap()).spawn().unwrap();
    let end = Instant::now() + Duration::from_secs(30);
    let code = loop {
        if let Some(st) = c.try_wait().unwrap() {
            break st.code();
        }
        if Instant::now() > end {
            common::send_child(&mut c, libc::SIGKILL);
            let _ = c.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let text = std::fs::read_to_string(&err).unwrap_or_default();
    let _ = std::fs::remove_file(&err);
    (code, text)
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

/// This machine's journal folder name (`<boot>-<pidns>`), from a kept journal of a short job.
fn folder(d: &Path) -> (String, String, String) {
    let s = state(&d.join("probe"));
    let st = Command::new(sheepr())
        .args(["run", "--", "/bin/sh", "-c", "exit 0"])
        .env("SHEEPR_TEST_STATE", &s)
        .env("SHEEPR_TEST_KEEP_JOURNAL", "1")
        .status()
        .unwrap();
    let _ = st;
    let j = journals(&s).pop().expect("a kept journal");
    let h = json::parse(std::fs::read_to_string(&j).unwrap().lines().next().unwrap()).unwrap();
    let boot = h.get("boot").and_then(Json::str).unwrap().to_string();
    let pidns = h.get("pidns").and_then(Json::str).unwrap().to_string();
    let _ = std::fs::remove_dir_all(d.join("probe"));
    (format!("{boot}-{pidns}"), boot, pidns)
}

/// A forged journal in `<state>/jobs/<dir>/<job>.journal` for a dead supervisor, naming `members`.
fn forge(s: &Path, dir: &str, job: &str, owner: &str, boot: &str, pidns: &str, members: &[(i32, u64)], leave_strays: bool) -> PathBuf {
    let folder = s.join("jobs").join(dir);
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::set_permissions(s.join("jobs"), std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
    std::fs::set_permissions(&folder, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
    let uid = unsafe { libc::getuid() };
    let mut text = format!(
        "{{\"v\":1,\"kind\":\"header\",\"job\":\"{job}\",\"boot\":\"{boot}\",\"pidns\":\"{pidns}\",\"owner\":\"{owner}\",\"uid\":{uid},\"sup\":{{\"pid\":999999,\"id\":1}},\"argv\":\"sheepr run\"}}\n"
    );
    for &(p, id) in members {
        text.push_str(&format!("{{\"v\":1,\"pid\":{p},\"id\":{id},\"ppid\":1,\"pid_id\":null,\"puniq\":null,\"cmd\":\"decoy\"}}\n"));
    }
    if leave_strays {
        text.push_str("{\"v\":1,\"kind\":\"leave-strays\"}\n");
    }
    let path = folder.join(format!("{job}.journal"));
    std::fs::write(&path, text).unwrap();
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    path
}

/// Cell 14: a job whose supervisor was SIGKILLed leaves its escapee; `sweep` finds it through
/// the journal and ends it, and the journal is removed.
#[test]
fn cell_14_sweep_ends_a_dead_jobs_escapee() {
    let d = scratch("c14");
    let s = state(&d);
    let r = d.join("rec");
    let mut c = Command::new(sheepr()).args(["run", "--", fixture(), "escapee-and-wait"]).arg(&r).env("SHEEPR_TEST_STATE", &s).spawn().unwrap();
    let g = wait_until(15, || !records(&r).is_empty()).then(|| records(&r)[0]);
    let journaled_g = g.is_some_and(|g| wait_until(10, || journaled(&s).contains(&g.0)));
    common::send_child(&mut c, libc::SIGKILL);
    let _ = c.wait();
    let alive_before = g.is_some_and(common::alive);
    let (code, _) = sweep(&s, &d, &[], &[]);
    let alive_after = g.is_some_and(common::alive);
    let root = records(&PathBuf::from(format!("{}.root", r.display())));
    for p in g.iter().chain(root.iter()) {
        common::send(p.0, p.1, libc::SIGKILL);
    }
    assert!(journaled_g, "the escapee was journaled");
    assert!(alive_before, "control: the escapee outlived its SIGKILLed supervisor");
    assert_eq!(code, Some(0));
    assert!(!alive_after, "sweep left the escapee alive");
    assert!(root.iter().all(|p| !common::alive(*p)), "the root is gone too");
    assert!(journals(&s).is_empty(), "the swept journal is removed");
    let _ = std::fs::remove_dir_all(&d);
}

/// The fences: a forged journal naming a live decoy with its correct identity is swept only from
/// this boot's and this pid namespace's folder, only for its owner tag, never with the
/// leave-strays mark; a journal elsewhere is left in place. The control, the same journal in
/// this folder for the default owner, ends the decoy.
#[test]
fn sweep_keeps_to_its_own_boot_pid_namespace_and_owner() {
    let d = scratch("fences");
    let (here, boot, pidns) = folder(&d);
    let cases: Vec<(&str, String, &str, bool, &[&str])> = vec![
        ("other-boot", format!("00000000-0000-0000-0000-000000000000-{pidns}"), "default", false, &[]),
        ("other-pidns", format!("{boot}-12345.678"), "default", false, &[]),
        ("other-owner", here.clone(), "liveapp", false, &[]),
        ("leave-strays", here.clone(), "default", true, &[]),
    ];
    for (name, dir, owner, strays, args) in cases {
        let s = state(&d.join(name));
        // this boot's folder exists too (with nothing to sweep), so a sweep does read this state
        std::fs::create_dir_all(s.join("jobs").join(&here)).unwrap();
        std::fs::set_permissions(s.join("jobs").join(&here), std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
        let (hb, hp) = match name {
            "other-boot" => ("00000000-0000-0000-0000-000000000000".to_string(), pidns.clone()),
            "other-pidns" => (boot.clone(), "12345.678".to_string()),
            _ => (boot.clone(), pidns.clone()),
        };
        let (mut c, p, r) = decoy(&d, &format!("decoy-{name}"));
        let j = forge(&s, &dir, "j-0badf00d", owner, &hb, &hp, &[p], strays);
        let (code, _) = sweep(&s, &d, args, &[]);
        let (alive, n) = (common::alive(p), counted(&r));
        end_decoy(&mut c);
        assert_eq!(code, Some(0), "{name}");
        assert!(alive && n == 0, "{name}: the decoy got {n} signal(s), alive {alive}");
        assert!(j.exists(), "{name}: the journal was deleted");
    }
    // controls: the same decoy journal in this folder (default owner), and the other owner's
    // journal swept by `--owner liveapp`
    for (name, owner, args) in [("control", "default", &[][..]), ("owner-control", "liveapp", &["--owner", "liveapp"][..])] {
        let s = state(&d.join(name));
        let (mut c, p, _) = decoy(&d, &format!("decoy-{name}"));
        forge(&s, &here, "j-0badf00d", owner, &boot, &pidns, &[p], false);
        let (code, _) = sweep(&s, &d, args, &[]);
        let alive = common::alive(p);
        end_decoy(&mut c);
        assert_eq!(code, Some(0), "{name}");
        assert!(!alive, "{name}: the sweep did not end the decoy");
    }
    let _ = std::fs::remove_dir_all(&d);
}

/// A live job's journal is locked: `sweep` leaves it and its members alone.
#[test]
fn sweep_skips_a_live_job() {
    let d = scratch("live");
    let s = state(&d);
    let r = d.join("rec");
    let mut c = Command::new(sheepr()).args(["run", "--", fixture(), "escapee-and-wait"]).arg(&r).env("SHEEPR_TEST_STATE", &s).spawn().unwrap();
    let g = wait_until(15, || !records(&r).is_empty()).then(|| records(&r)[0]);
    let journaled_g = g.is_some_and(|g| wait_until(10, || journaled(&s).contains(&g.0)));
    let (code, _) = sweep(&s, &d, &[], &[]);
    let alive = g.is_some_and(common::alive);
    let n = counted(&PathBuf::from(format!("{}.g", r.display())));
    common::send_child(&mut c, libc::SIGTERM); // its own supervisor ends it
    let _ = c.wait();
    assert!(journaled_g);
    assert_eq!(code, Some(0));
    assert!(alive && n == 0, "the live job's member got {n} signal(s), alive {alive}");
    let _ = std::fs::remove_dir_all(&d);
}

/// A journal entry whose identity does not match the live process at its pid is never
/// signalled, and neither is that process's child (the closure starts only from matching entries).
#[test]
fn a_wrong_identity_entry_is_never_signalled() {
    let d = scratch("wrongid");
    let (here, boot, pidns) = folder(&d);
    let s = state(&d);
    let r = d.join("forker");
    let mut c = Command::new(fixture()).args(["forker", "1", "10"]).arg(&r).spawn().unwrap();
    assert!(wait_until(10, || records(&r).len() == 1), "the child started");
    let parent = (c.id() as i32, sheepr::ident::identity(c.id() as i32).unwrap());
    let child = records(&r)[0];
    forge(&s, &here, "j-0badf00d", "default", &boot, &pidns, &[(parent.0, parent.1 + WRONG)], false);
    let (code, _) = sweep(&s, &d, &[], &[]);
    let (pa, ca) = (common::alive(parent), common::alive(child));
    common::send(child.0, child.1, libc::SIGKILL);
    end_decoy(&mut c);
    assert_eq!(code, Some(0));
    assert!(pa && ca, "parent alive {pa}, child alive {ca}");
    assert!(journals(&s).is_empty(), "control: the journal was read (and, naming nothing alive, removed)");
    let _ = std::fs::remove_dir_all(&d);
}

/// A state directory another user could write (the boot folder group-writable) is refused:
/// exit 1, nothing signalled. The control is the fences cell (a 0700 folder is swept).
#[test]
fn sweep_refuses_a_folder_others_can_write() {
    let d = scratch("perm");
    let (here, boot, pidns) = folder(&d);
    let s = state(&d);
    let (mut c, p, r) = decoy(&d, "decoy");
    forge(&s, &here, "j-0badf00d", "default", &boot, &pidns, &[p], false);
    std::fs::set_permissions(s.join("jobs").join(&here), std::os::unix::fs::PermissionsExt::from_mode(0o770)).unwrap();
    let (code, _) = sweep(&s, &d, &[], &[]);
    let (alive, n) = (common::alive(p), counted(&r));
    end_decoy(&mut c);
    assert_eq!(code, Some(1), "refused");
    assert!(alive && n == 0, "the decoy got {n} signal(s)");
    let _ = std::fs::remove_dir_all(&d);
}

/// The wall control for the sweep (PHASE2.md §0.1): a journal that names an untagged process the
/// test built, with its correct identity: the sweep turns the latch on, and the door withholds
/// every signal to it (its own counter stays 0, a `withheld` line names it).
#[test]
fn the_sweep_turns_the_latch_on() {
    let d = scratch("latch");
    let (here, boot, pidns) = folder(&d);
    let s = state(&d);
    let r = d.join("untagged");
    let mut c = Command::new("/usr/bin/env").args(["-i", fixture(), "sigcount"]).arg(&r).spawn().unwrap();
    assert!(wait_until(10, || !records(&r).is_empty()));
    let p = records(&r)[0];
    forge(&s, &here, "j-0badf00d", "default", &boot, &pidns, &[p], false);
    let sink = d.join("sink");
    let mut cmd = Command::new(sheepr());
    cmd.arg("sweep").env("SHEEPR_TEST_STATE", &s).env("SHEEPR_TEST_DEADLINE_MS", "500");
    common::cell_sink(&mut cmd, &sink);
    let _ = cmd.status();
    let (alive, n) = (common::alive(p), counted(&r));
    let lines = std::fs::read_to_string(&sink).unwrap_or_default();
    end_decoy(&mut c);
    assert!(alive && n == 0, "the untagged process got {n} signal(s)");
    assert!(lines.lines().any(|l| l.starts_with("withheld ") && l.split_whitespace().nth(1) == Some(&p.0.to_string())), "{lines:?}");
    let _ = std::fs::remove_dir_all(&d);
}

/// A child born to a journaled member after the supervisor died (no scan saw it) is swept
/// through the closure, and the sweeper journals it before its first signal to it.
#[test]
fn a_child_born_after_the_last_scan_is_swept_and_journaled_first() {
    let d = scratch("closure");
    let s = state(&d);
    let (go, r) = (d.join("go"), d.join("rec"));
    let log = d.join("log");
    // the member is a background child of the root (the root becomes a counting fixture), so it
    // outlives its supervisor on Linux too, where the root dies with it (PDEATHSIG)
    let rr = d.join("root");
    let script = format!(r#""$FX" spawn-on "{}" "{}" & exec "$FX" sigcount "{}""#, go.display(), r.display(), rr.display());
    let mut c = Command::new(sheepr()).args(["run", "--", "/bin/sh", "-c", &script]).env("FX", fixture()).env("SHEEPR_TEST_STATE", &s).spawn().unwrap();
    let parent = wait_until(15, || !records(&r).is_empty()).then(|| records(&r)[0]);
    let journaled_parent = parent.is_some_and(|p| wait_until(10, || journaled(&s).contains(&p.0)));
    common::send_child(&mut c, libc::SIGKILL);
    let _ = c.wait();
    let parent_alive = parent.is_some_and(common::alive);
    std::fs::write(&go, b"").unwrap();
    let child = wait_until(10, || records(&r).len() >= 2).then(|| records(&r)[1]);
    let (code, _) = sweep(&s, &d, &[], &[("SHEEPR_TEST_SIGNAL_LOG", log.to_str().unwrap())]);
    let alive: Vec<_> = records(&r).into_iter().chain(records(&rr)).filter(|p| common::alive(*p)).collect();
    for p in &alive {
        common::send(p.0, p.1, libc::SIGKILL);
    }
    assert!(journaled_parent && parent_alive && child.is_some(), "the member outlived its supervisor and forked");
    assert_eq!(code, Some(0));
    assert!(alive.is_empty(), "survivors {alive:?}");
    let text = std::fs::read_to_string(&log).unwrap_or_default();
    let cp = child.unwrap().0.to_string();
    let j = text.lines().position(|l| l.split_whitespace().collect::<Vec<_>>() == ["journal", cp.as_str()]);
    let first = text.lines().position(|l| {
        let w: Vec<&str> = l.split_whitespace().collect();
        w.len() >= 2 && (w[0] == "kill" || w[0] == "pidfd") && w[1] == cp
    });
    assert!(j.is_some() && first.is_some() && j < first, "journal at {j:?}, first signal at {first:?}:\n{text}");
    let _ = std::fs::remove_dir_all(&d);
}

/// `sweep` never touches its own ancestors: run from inside a dead job (the job's member is the
/// shell that starts it), it skips that job whole (the member lives, the journal is kept), and it
/// ends (never stops itself). A note names the skipped job.
#[test]
fn sweep_never_touches_its_own_ancestors() {
    let d = scratch("ancestors");
    let s = state(&d);
    let (go, rc, me) = (d.join("go"), d.join("rc"), d.join("me"));
    let rr = d.join("root");
    // the member W is a shell that waits for GO, then runs `sheepr sweep` (the debug build on
    // the test PATH) and writes its code
    // after it, W stays alive (a marked sleep), so the checks below see W itself
    let m = format!("29.{}", std::process::id());
    let w = format!(r#"echo $$ > "{}"; while [ ! -e "{}" ]; do sleep 0.02; done; sheepr sweep; echo $? > "{}"; sleep {m}"#, me.display(), go.display(), rc.display());
    // a witness member, also a background child (the root dies with its supervisor on Linux):
    // skipped whole, the job's witness is untouched
    let wit = d.join("witness");
    let script = format!(r#"/bin/sh -c '{}' & "$FX" sigcount "{}" & exec "$FX" sigcount "{}""#, w.replace('\'', r#"'\''"#), wit.display(), rr.display());
    let mut c = Command::new(sheepr()).args(["run", "--", "/bin/sh", "-c", &script]).env("FX", fixture()).env("SHEEPR_TEST_STATE", &s).spawn().unwrap();
    let wpid: Option<i32> = wait_until(15, || std::fs::read_to_string(&me).is_ok_and(|t| !t.trim().is_empty())).then(|| std::fs::read_to_string(&me).unwrap().trim().parse().unwrap());
    let wid = wpid.and_then(sheepr::ident::identity);
    let journaled_w = wpid.is_some_and(|p| wait_until(10, || journaled(&s).contains(&p)));
    common::send_child(&mut c, libc::SIGKILL);
    let _ = c.wait();
    let trace = d.join("trace");
    std::fs::write(d.join("trace-env"), trace.to_str().unwrap()).unwrap();
    std::fs::write(&go, b"").unwrap();
    // wait for the code itself, to its newline, not for the file: the shell creates the file
    // before it writes the code, and a read between the two parses nothing (the enosys leg)
    let finished = wait_until(20, || std::fs::read_to_string(&rc).is_ok_and(|t| t.ends_with('\n')));
    let code: Option<i32> = std::fs::read_to_string(&rc).ok().and_then(|t| t.trim().parse().ok());
    let w_alive = wpid.zip(wid).is_some_and(common::alive);
    // the job is skipped WHOLE: its other member (the root, a counting fixture) is untouched too
    let root = records(&rr);
    let witness = records(&wit);
    let root_untouched = !witness.is_empty() && witness.iter().all(|p| common::alive(*p)) && counted(&wit) == 0;
    let kept = !journals(&s).is_empty();
    if let Some(p) = wpid.zip(wid) {
        common::send(p.0, p.1, libc::SIGKILL);
    }
    for p in root.into_iter().chain(witness) {
        common::send(p.0, p.1, libc::SIGKILL);
    }
    common::kill_marked(&[&m]);
    assert!(journaled_w, "the shell was journaled as a member");
    assert!(finished, "the sweep did not end (did it stop itself?)");
    assert_eq!(code, Some(0));
    assert!(w_alive, "the sweep signalled its own ancestor");
    assert!(root_untouched, "the job holding the sweep's ancestor was swept");
    assert!(kept, "the skipped job's journal is kept");
    let _ = std::fs::remove_dir_all(&d);
}

/// An inner supervisor in a dead job is ended first (TERM, then CONT): it then ends its own job.
/// The inner job's escapee is born after the outer supervisor died (GO), so no outer scan saw it:
/// on macOS only the inner supervisor can reach it; on Linux the ppid closure does too.
#[test]
fn sweep_ends_an_inner_supervisor_first() {
    let d = scratch("inner");
    let s = state(&d);
    let (r, rr, go) = (d.join("rec"), d.join("root"), d.join("go"));
    let inner_state = state(&d.join("inner"));
    // the inner run is a background child (so it inherits no PDEATHSIG on Linux) with a state of
    // its own; the outer root becomes a counting fixture
    let script = format!(
        r#"SHEEPR_TEST_STATE="{}" sheepr run -- "$FX" escapee-and-wait "{}" "{}" & echo $! > "{}"; exec "$FX" sigcount "{}""#,
        inner_state.display(),
        r.display(),
        go.display(),
        d.join("innerpid").display(),
        rr.display()
    );
    let mut c = Command::new(sheepr()).args(["run", "--", "/bin/sh", "-c", &script]).env("FX", fixture()).env("SHEEPR_TEST_STATE", &s).spawn().unwrap();
    // the inner supervisor is journaled by the outer (readiness), then the outer is SIGKILLed
    let inner: i32 = { assert!(wait_until(15, || std::fs::read_to_string(d.join("innerpid")).is_ok_and(|t| !t.trim().is_empty()))); std::fs::read_to_string(d.join("innerpid")).unwrap().trim().parse().unwrap() };
    assert!(wait_until(10, || journaled(&s).contains(&inner)), "the inner supervisor was journaled");
    common::send_child(&mut c, libc::SIGKILL);
    let _ = c.wait();
    std::fs::write(&go, b"").unwrap();
    let g = wait_until(15, || !records(&r).is_empty()).then(|| records(&r)[0]);
    let g_before = g.is_some_and(common::alive);
    let (code, _) = sweep(&s, &d, &[], &[]);
    let g_after = wait_until(5, || !g.is_some_and(common::alive)) == false;
    for p in g.iter().copied().chain(records(&rr)).chain(records(&PathBuf::from(format!("{}.root", r.display())))) {
        common::send(p.0, p.1, libc::SIGKILL);
    }
    assert!(g_before, "control: the inner escapee was born and lives");
    assert_eq!(code, Some(0));
    assert!(!g_after, "the inner job's escapee survived the sweep");
    let _ = std::fs::remove_dir_all(&d);
}

/// Review P2-9: the header is a fence of its own: a journal in THIS folder whose header names
/// another boot (or pid namespace) is not swept.
#[test]
fn a_header_from_another_boot_is_not_swept_from_this_folder() {
    let d = scratch("header");
    let (here, boot, pidns) = folder(&d);
    for (name, hb, hp) in [("boot", "00000000-0000-0000-0000-000000000000".to_string(), pidns.clone()), ("pidns", boot.clone(), "12345.678".to_string())] {
        let s = state(&d.join(name));
        let (mut c, p, r) = decoy(&d, &format!("decoy-{name}"));
        let j = forge(&s, &here, "j-0badf00d", "default", &hb, &hp, &[p], false);
        let (code, _) = sweep(&s, &d, &[], &[]);
        let (alive, n) = (common::alive(p), counted(&r));
        end_decoy(&mut c);
        assert_eq!(code, Some(0), "{name}");
        assert!(alive && n == 0, "{name}: swept ({n})");
        assert!(j.exists(), "{name}: deleted");
    }
    let _ = std::fs::remove_dir_all(&d);
}

/// Issue #15 (first step): a journal `sweep` cannot read (no header), or will not act on for
/// safety (another boot's header in this boot's folder), is said on stderr, one line naming the
/// file; before, only a debug build's trace saw it. A journal kept on purpose (`--leave-strays`)
/// says nothing. The exit code stays 0. The control, a readable dead journal, names no file.
/// The cell runs the debug build (a release build refuses a test environment); the line is a
/// plain `say!`, the same in both builds.
#[test]
fn a_skipped_journal_is_said_on_stderr() {
    let d = scratch("saysskip");
    let (here, boot, pidns) = folder(&d);
    let say = |s: &Path| sweep_said(s, &[]);
    let s = state(&d.join("skips"));
    forge(&s, &here, "j-0th3rb00t", "default", "00000000-0000-0000-0000-000000000000", &pidns, &[], false);
    forge(&s, &here, "j-k3pt", "default", &boot, &pidns, &[], true);
    let bad = s.join("jobs").join(&here).join("j-b4d.journal");
    std::fs::write(&bad, "not a journal\n").unwrap();
    std::fs::set_permissions(&bad, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    let (code, err) = say(&s);
    let lines = |name: &str| err.lines().filter(|l| l.contains(name)).count();
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(lines("j-b4d.journal"), 1, "the unreadable journal:\n{err}");
    assert_eq!(lines("j-0th3rb00t.journal"), 1, "another boot's journal:\n{err}");
    assert_eq!(lines("j-k3pt"), 0, "a journal kept on purpose:\n{err}");
    let c = state(&d.join("control"));
    forge(&c, &here, "j-0k", "default", &boot, &pidns, &[], false);
    let (code, err) = say(&c);
    assert_eq!(code, Some(0), "control: {err}");
    assert!(journals(&c).is_empty(), "control: the readable journal was swept");
    assert!(!err.contains(".journal"), "control:\n{err}");
    let _ = std::fs::remove_dir_all(&d);
}

/// Review of 6d0ae1a, P2-1: a journal folder `sweep` cannot list (mode 0300), or cannot reach
/// (`jobs/` at 0600), is refused with one stderr line that names it, exit 1, as an unsafe folder
/// is; before, it was read as empty: exit 0, nothing said, the stale job's decoy alive. The
/// control, the same state at 0700, is swept. Skipped as root (root reads a folder of any mode).
/// (No "the decoy lives" assertion: nothing can reach a journal it cannot name, review of
/// 35624d5 P3-4.)
#[test]
fn a_folder_sweep_cannot_read_is_refused_and_said() {
    if unsafe { libc::geteuid() } == 0 {
        eprintln!("skipped: root reads a folder of any mode");
        return;
    }
    let d = scratch("unlisted");
    let (here, boot, pidns) = folder(&d);
    let s = state(&d);
    let (mut c, p, _r) = decoy(&d, "decoy");
    forge(&s, &here, "j-0badf00d", "default", &boot, &pidns, &[p], false);
    let dir = s.join("jobs").join(&here);
    let _restore = Modes(vec![dir.clone(), s.join("jobs")]);
    mode(&dir, 0o300);
    let (code1, err1) = sweep_said(&s, &[]);
    mode(&dir, 0o700);
    mode(&s.join("jobs"), 0o600);
    let (code2, err2) = sweep_said(&s, &[]);
    mode(&s.join("jobs"), 0o700);
    let (code3, err3) = sweep_said(&s, &[]);
    let gone = !common::alive(p);
    end_decoy(&mut c);
    let names = |e: &str| e.lines().filter(|l| l.contains(here.as_str())).count();
    assert_eq!((code1, names(&err1)), (Some(1), 1), "a folder it cannot list:\n{err1}");
    assert_eq!((code2, names(&err2)), (Some(1), 1), "a folder it cannot reach:\n{err2}");
    assert_eq!(code3, Some(0), "control: {err3}");
    assert!(gone, "control: the same state at 0700 is swept");
    assert_eq!(names(&err3), 0, "control:\n{err3}");
    let _ = std::fs::remove_dir_all(&d);
}

/// Review of 35624d5, P2-1: a normal state is swept in silence, exit 0, nothing on stderr: a fresh
/// state (only the sentinel), `jobs/` with no folder for this boot and an old boot's folder that
/// still holds a journal (as after a reboot), and this boot's folder empty. Only "not found" means nothing to sweep; liveapp's boot sweep meets the
/// first two on every new instance and after every reboot.
#[test]
fn a_normal_state_is_swept_in_silence() {
    let d = scratch("normal");
    let (here, _, _) = folder(&d);
    let fresh = state(&d.join("fresh"));
    let noboot = state(&d.join("noboot"));
    // after a reboot: no folder for this boot, and the old boot's folder still holds a journal
    let old = noboot.join("jobs").join("00000000-0000-0000-0000-000000000000-0");
    std::fs::create_dir_all(&old).unwrap();
    mode(&noboot.join("jobs"), 0o700);
    mode(&old, 0o700);
    std::fs::write(old.join("j-0ldb00t0.journal"), "not read\n").unwrap();
    let empty = state(&d.join("empty"));
    std::fs::create_dir_all(empty.join("jobs").join(&here)).unwrap();
    mode(&empty.join("jobs"), 0o700);
    mode(&empty.join("jobs").join(&here), 0o700);
    for (n, s) in [("fresh", &fresh), ("no boot folder", &noboot), ("empty boot folder", &empty)] {
        let (code, err) = sweep_said(s, &[]);
        assert_eq!((code, err.as_str()), (Some(0), ""), "{n}");
    }
    let _ = std::fs::remove_dir_all(&d);
}

/// The `go` file of a live job's root loop, written when the cell ends, a panic too, so a cell
/// that fails before its own `go` never leaves the loop (and its supervisor) running, which would
/// hold the test's output and hang the run (whole-branch review P3-1). Declare it after the job,
/// so it drops first.
struct GoOnDrop(PathBuf);
impl Drop for GoOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::write(&self.0, b"");
    }
}

/// A live job whose journal sorts after a stale job's, during a `sweep` held inside the stale
/// job's kill (after its freeze). `end_during`: the live job ends cleanly in that hold, so its
/// journal is gone when the sweep comes to it. Returns (held, gone, stderr).
fn race(tag: &str, end_during: bool) -> (bool, bool, String) {
    let d = scratch(tag);
    let (here, boot, pidns) = folder(&d);
    let s = state(&d);
    let go = d.join("go");
    let script = format!(r#"while [ ! -e "{}" ]; do sleep 0.05; done"#, go.display());
    let mut live = Command::new(sheepr())
        .args(["run", "--no-sweep", "--", "/bin/sh", "-c", &script])
        .env("SHEEPR_TEST_STATE", &s)
        .env("SHEEPR_TEST_JOB_ID", "zzzzzzzz")
        .spawn()
        .unwrap();
    let _go = GoOnDrop(go.clone());
    let lj = s.join("jobs").join(&here).join("j-zzzzzzzz.journal");
    assert!(wait_until(15, || lj.exists()), "the live job's journal");
    let (mut c, p, _r) = decoy(&d, "decoy");
    forge(&s, &here, "j-00000000", "default", &boot, &pidns, &[p], false);
    let (ready, release) = (d.join("ready"), d.join("release"));
    let (s2, rd2, rl2) = (s.clone(), ready.clone(), release.clone());
    let t = std::thread::spawn(move || {
        let o = Command::new(sheepr())
            .arg("sweep")
            .env("SHEEPR_TEST_STATE", &s2)
            .env("SHEEPR_TEST_DEADLINE_MS", "5000")
            .env("SHEEPR_TEST_HOLD_AFTER_FREEZE", &rl2)
            .env("SHEEPR_TEST_READY_FILE", &rd2)
            .output()
            .unwrap();
        String::from_utf8_lossy(&o.stderr).into_owned()
    });
    let held = wait_until(15, || ready.exists());
    if end_during {
        std::fs::write(&go, b"").unwrap();
        let _ = live.wait();
    }
    let gone = !lj.exists();
    std::fs::write(&release, b"").unwrap();
    let err = t.join().unwrap();
    if !end_during {
        std::fs::write(&go, b"").unwrap();
        let _ = live.wait();
    }
    end_decoy(&mut c);
    let _ = std::fs::remove_dir_all(&d);
    (held, gone, err)
}

/// Review of e3e2d3a, P1: a job that ends cleanly while a `sweep` works on another job is not
/// said (its journal is gone between the listing and the open: that is a live job ending, as a
/// clean end that unlinks after the lock is); the control, a job that stays live, is not said
/// either.
#[test]
fn a_job_that_ends_during_a_sweep_is_not_said() {
    let (held, gone, err) = race("ends", true);
    assert!(held && gone, "the race did not happen: held {held}, gone {gone}");
    assert!(!err.contains("j-zzzzzzzz"), "a job that ended cleanly during the sweep was said:\n{err}");
    let (held, gone, err) = race("stays", false);
    assert!(held && !gone, "control: held {held}, gone {gone}");
    assert!(!err.contains("j-zzzzzzzz"), "control: a live job was said:\n{err}");
}

/// The second wall control (PHASE2.md §4a, round 2): a journaled pid now held by an untagged
/// process under another identity is refused as `gone`: no signal, no `withheld` line.
#[test]
fn a_reused_journaled_pid_is_gone_not_withheld() {
    let d = scratch("reusedwall");
    let (here, boot, pidns) = folder(&d);
    let s = state(&d);
    let r = d.join("untagged");
    let mut c = Command::new("/usr/bin/env").args(["-i", fixture(), "sigcount"]).arg(&r).spawn().unwrap();
    assert!(wait_until(10, || !records(&r).is_empty()));
    let p = records(&r)[0];
    forge(&s, &here, "j-0badf00d", "default", &boot, &pidns, &[(p.0, p.1 + WRONG)], false);
    let sink = d.join("sink");
    let mut cmd = Command::new(sheepr());
    cmd.arg("sweep").env("SHEEPR_TEST_STATE", &s);
    common::cell_sink(&mut cmd, &sink);
    let code = cmd.status().unwrap().code();
    let (alive, n) = (common::alive(p), counted(&r));
    let lines = std::fs::read_to_string(&sink).unwrap_or_default();
    end_decoy(&mut c);
    assert_eq!(code, Some(0));
    assert!(alive && n == 0, "{n}");
    assert!(lines.is_empty(), "a reused pid is gone, not withheld: {lines:?}");
    assert!(journals(&s).is_empty(), "control: the journal was read (and, naming nothing alive, removed)");
    let _ = std::fs::remove_dir_all(&d);
}

/// Review P2-9 (other-user leg only): a journal the leg planted as root, owned by another user,
/// naming this user's decoy with its correct identity, is not swept; the same journal owned by
/// this user is (the control). The leg passes the directories and the decoy in SR_PLANTED.
#[test]
fn another_users_journal_is_never_swept() {
    let Ok(planted) = std::env::var("SR_PLANTED") else {
        eprintln!("skipped: only the other-user leg plants a journal as root");
        return;
    };
    // "<state owned>|<state control>|<decoy pid>|<decoy id>|<decoy record>"
    let v: Vec<&str> = planted.split('|').collect();
    let (foreign, own, p, rec) = (PathBuf::from(v[0]), PathBuf::from(v[1]), (v[2].parse::<i32>().unwrap(), v[3].parse::<u64>().unwrap()), PathBuf::from(v[4]));
    let d = scratch("planted");
    let (code, err) = sweep_said(&foreign, &[]);
    let (alive, n) = (common::alive(p), counted(&rec));
    let (code2, err2) = sweep_said(&own, &[]);
    let gone = !common::alive(p);
    assert_eq!(code, Some(0));
    assert!(alive && n == 0, "another user's journal was swept ({n})");
    // issue #15: the skip is said, one line naming the file
    assert_eq!(err.lines().filter(|l| l.contains("j-0badf00d.journal")).count(), 1, "another user's journal:\n{err}");
    assert_eq!(code2, Some(0));
    assert!(gone, "control: the same journal owned by this user is swept");
    assert!(!err2.contains("j-0badf00d.journal"), "control:\n{err2}");
    let _ = std::fs::remove_dir_all(&d);
}

/// Every candidate a sweep finds is journaled before its first signal, not only those of its
/// first scan (PHASE2.md §3 rule 2): a member's child born while the sweep waits for an inner
/// supervisor to end (its caller ignores TERM, so the wait runs its full length) has its journal
/// line before any signal to it.
#[test]
fn a_member_born_during_the_supervisor_wait_is_journaled_first() {
    let d = scratch("latejournal");
    let s = state(&d);
    let (go, r, inner, rr, log) = (d.join("go"), d.join("rec"), d.join("inner"), d.join("root"), d.join("log"));
    let script = format!(
        r#"(trap "" TERM; exec "$SD" run -- "$FX" sigcount "{}") & "$FX" spawn-on "{}" "{}" & exec "$FX" sigcount "{}""#,
        inner.display(),
        go.display(),
        r.display(),
        rr.display()
    );
    let mut c = Command::new(sheepr()).args(["run", "--", "/bin/sh", "-c", &script]).env("FX", fixture()).env("SD", sheepr()).env("SHEEPR_TEST_STATE", &s).spawn().unwrap();
    let parent = wait_until(15, || !records(&r).is_empty() && !records(&inner).is_empty()).then(|| records(&r)[0]);
    let ir = records(&inner).first().copied();
    let isup = ir.and_then(|x| {
        let o = Command::new("ps").args(["-o", "ppid=", "-p", &x.0.to_string()]).output().ok()?;
        common::found(String::from_utf8_lossy(&o.stdout).trim().parse().ok()?)
    });
    let journaled_all = parent.is_some_and(|p| wait_until(10, || journaled(&s).contains(&p.0) && isup.is_some_and(|i| journaled(&s).contains(&i.0))));
    common::send_child(&mut c, libc::SIGKILL);
    let _ = c.wait();
    // the child is born while the sweep waits for the inner supervisor (grace 2 s + the
    // deadline): `go` is written once the signal log shows that wait has begun
    let (g2, l2) = (go.clone(), log.clone());
    let t = std::thread::spawn(move || {
        let end = Instant::now() + Duration::from_secs(20);
        while !std::fs::read_to_string(&l2).unwrap_or_default().lines().any(|l| l.starts_with("supervisor ")) && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(5));
        }
        std::fs::write(&g2, b"").unwrap();
    });
    let (code, _) = sweep(&s, &d, &[], &[("SHEEPR_TEST_SIGNAL_LOG", log.to_str().unwrap()), ("SHEEPR_TEST_DEADLINE_MS", "1500")]);
    let _ = t.join();
    let child = records(&r).get(1).copied();
    let everyone: Vec<(i32, u64)> = records(&r).into_iter().chain(records(&rr)).chain(records(&inner)).chain(isup).collect();
    for p in &everyone {
        common::send(p.0, p.1, libc::SIGKILL);
    }
    assert!(journaled_all, "the outer did not journal the member and the inner supervisor");
    let cp = child.expect("the member's child was not born").0.to_string();
    let text = std::fs::read_to_string(&log).unwrap_or_default();
    let j = text.lines().position(|l| l.split_whitespace().collect::<Vec<_>>() == ["journal", cp.as_str()]);
    let first = text.lines().position(|l| {
        let w: Vec<&str> = l.split_whitespace().collect();
        w.len() >= 2 && (w[0] == "kill" || w[0] == "pidfd") && w[1] == cp
    });
    assert!(first.is_some(), "the sweep never signalled the late child (code {code:?}):\n{text}");
    assert!(j.is_some() && j < first, "journal at {j:?}, first signal at {first:?}:\n{text}");
    // born during the wait, not before the sweep's first scan
    let wait = text.lines().position(|l| l.starts_with("supervisor "));
    assert!(wait.is_some() && wait < j, "the child was journaled before the supervisor wait (at {j:?}, wait at {wait:?})");
    let _ = std::fs::remove_dir_all(&d);
}

/// The same rule inside the kill itself: a child born after the sweep's first freeze (its
/// parent, a member the test resumes while the sweep holds) is found by a later scan of the
/// kill, and has its journal line before any signal to it.
#[test]
fn a_member_born_during_the_kill_is_journaled_first() {
    let d = scratch("killjournal");
    let s = state(&d);
    let (go, r, rr, log, ready, release) = (d.join("go"), d.join("rec"), d.join("root"), d.join("log"), d.join("ready"), d.join("release"));
    let script = format!(r#""$FX" spawn-on "{}" "{}" & exec "$FX" sigcount "{}""#, go.display(), r.display(), rr.display());
    let mut c = Command::new(sheepr()).args(["run", "--", "/bin/sh", "-c", &script]).env("FX", fixture()).env("SHEEPR_TEST_STATE", &s).spawn().unwrap();
    let parent = wait_until(15, || !records(&r).is_empty() && !records(&rr).is_empty()).then(|| records(&r)[0]);
    let journaled_all = parent.is_some_and(|p| wait_until(10, || journaled(&s).contains(&p.0) && journaled(&s).contains(&records(&rr)[0].0)));
    common::send_child(&mut c, libc::SIGKILL);
    let _ = c.wait();
    let (s2, d2, l2, rd2, rl2) = (s.clone(), d.clone(), log.clone(), ready.clone(), release.clone());
    let t = std::thread::spawn(move || {
        let env = [("SHEEPR_TEST_SIGNAL_LOG", l2.to_str().unwrap()), ("SHEEPR_TEST_DEADLINE_MS", "5000"), ("SHEEPR_TEST_HOLD_AFTER_FREEZE", rl2.to_str().unwrap()), ("SHEEPR_TEST_READY_FILE", rd2.to_str().unwrap())];
        sweep(&s2, &d2, &[], &env)
    });
    // the sweep has frozen the job and holds: resume the parent, which spawns its child now
    let held = wait_until(15, || ready.exists());
    if let Some(p) = parent {
        common::send(p.0, p.1, libc::SIGCONT);
    }
    std::fs::write(&go, b"").unwrap();
    let born = wait_until(10, || records(&r).len() >= 2);
    std::fs::write(&release, b"").unwrap();
    let (code, _) = t.join().unwrap();
    let child = records(&r).get(1).copied();
    for p in records(&r).into_iter().chain(records(&rr)) {
        common::send(p.0, p.1, libc::SIGKILL);
    }
    assert!(journaled_all, "the job did not journal its members");
    assert!(held, "the sweep never reached its hold after the freeze");
    assert!(born, "the member's child was not born during the hold");
    let cp = child.unwrap().0.to_string();
    let text = std::fs::read_to_string(&log).unwrap_or_default();
    let j = text.lines().position(|l| l.split_whitespace().collect::<Vec<_>>() == ["journal", cp.as_str()]);
    let first = text.lines().position(|l| {
        let w: Vec<&str> = l.split_whitespace().collect();
        w.len() >= 2 && (w[0] == "kill" || w[0] == "pidfd") && w[1] == cp
    });
    assert!(first.is_some(), "the sweep never signalled the child born during the kill (code {code:?}):\n{text}");
    assert!(j.is_some() && j < first, "journal at {j:?}, first signal at {first:?}:\n{text}");
    let _ = std::fs::remove_dir_all(&d);
}

/// A journal line that names another user's process (with its correct identity) never reaches
/// the signal door: a root sweeper would otherwise deliver to it. Linux legs run as root; on a
/// normal account the kernel refuses the signal anyway, so the cell needs root.
#[cfg(target_os = "linux")]
#[test]
fn a_line_naming_another_users_process_is_never_signalled() {
    if unsafe { libc::getuid() } != 0 {
        eprintln!("skipped: needs root (a normal account cannot signal another user's process)");
        return;
    }
    let d = scratch("foreignuid");
    std::fs::set_permissions(&d, std::os::unix::fs::PermissionsExt::from_mode(0o777)).unwrap();
    let (here, boot, pidns) = folder(&d);
    let s = state(&d);
    let r = d.join("nobody");
    let mut c = Command::new(fixture()).args(["as-uid", "65534", fixture(), "sigcount"]).arg(&r).spawn().unwrap();
    assert!(wait_until(10, || !records(&r).is_empty()), "the other user's process did not start");
    let p = records(&r)[0];
    forge(&s, &here, "j-0badf00d", "default", &boot, &pidns, &[p], false);
    let (code, _) = sweep(&s, &d, &[], &[]);
    let (alive, n) = (common::alive(p), counted(&r));
    end_decoy(&mut c);
    assert!(alive && n == 0, "another user's process was signalled: alive {alive}, {n} counted signal(s) (code {code:?})");
    let _ = std::fs::remove_dir_all(&d);
}

/// `kill j-` of a journal whose header names a live process that is no `sheepr` refuses and
/// signals nothing (a live job's supervisor is always a sheepr; anything else is a stale or
/// forged header, and its process's tree is outside the journal's lineage).
#[test]
fn kill_job_refuses_a_header_that_names_no_supervisor() {
    let d = scratch("fakesup");
    let (here, boot, pidns) = folder(&d);
    let s = state(&d);
    let r = d.join("decoy");
    let mut c = Command::new(fixture()).args(["sigcount"]).arg(&r).spawn().unwrap();
    assert!(wait_until(10, || !records(&r).is_empty()));
    let p = records(&r)[0];
    let path = forge(&s, &here, "j-0badf00d", "default", &boot, &pidns, &[], false);
    let text = std::fs::read_to_string(&path).unwrap().replace("\"sup\":{\"pid\":999999,\"id\":1}", &format!("\"sup\":{{\"pid\":{},\"id\":{}}}", p.0, p.1));
    std::fs::write(&path, text).unwrap();
    let st = Command::new(sheepr()).args(["kill", "j-0badf00d"]).env("SHEEPR_TEST_STATE", &s).env("SHEEPR_TEST_DEADLINE_MS", "1000").status().unwrap();
    let (alive, n) = (common::alive(p), counted(&r));
    end_decoy(&mut c);
    assert!(alive && n == 0, "the process the header named got {n} signal(s)");
    assert_eq!(st.code(), Some(1));
    let _ = std::fs::remove_dir_all(&d);
}
