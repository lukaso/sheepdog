//! Phase-2 P4c: `sheepdog kill j-XXXX` (PHASE2.md §1 decision 11), `kill PID:ID`, and the journal
//! as a membership fact in `kill <pid>` (decision 12). Every process a kill may aim at is built
//! here from the fixture binary (these sources turn the latch on).

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
    let d = std::env::temp_dir().join(format!("sd-kj-{name}-{}", std::process::id()));
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

fn job_id(s: &Path) -> String {
    journals(s)[0].file_stem().unwrap().to_string_lossy().into_owned()
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

/// `sheepdog kill ARGS` with this state; its exit code (30 s bound).
fn kill(s: &Path, args: &[&str], env: &[(&str, &str)]) -> Option<i32> {
    let mut c = Command::new(sheepdog());
    c.arg("kill").args(args).env("SHEEPDOG_TEST_STATE", s);
    for (k, v) in env {
        c.env(k, v);
    }
    let mut ch = c.spawn().unwrap();
    let end = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(st) = ch.try_wait().unwrap() {
            return st.code();
        }
        if Instant::now() > end {
            common::send_child(&mut ch, libc::SIGKILL);
            let _ = ch.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A live job (`escapee-and-wait`), its escapee journaled.
fn live_job(d: &Path, s: &Path, name: &str, pre: impl FnOnce(&mut Command)) -> (Child, (i32, u64), PathBuf) {
    let r = d.join(name);
    let mut cmd = Command::new(sheepdog());
    cmd.args(["run", "--"]).arg(fixture()).arg("escapee-and-wait").arg(&r).env("SHEEPDOG_TEST_STATE", s);
    pre(&mut cmd);
    let c = cmd.spawn().unwrap();
    assert!(wait_until(15, || !records(&r).is_empty()), "{name}: the escapee started");
    let g = records(&r)[0];
    assert!(wait_until(10, || journaled(s).contains(&g.0)), "{name}: the escapee was journaled");
    (c, g, r)
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

fn finished(c: &mut Child, secs: u64) -> Option<std::process::ExitStatus> {
    let end = Instant::now() + Duration::from_secs(secs);
    loop {
        if let Some(st) = c.try_wait().unwrap() {
            return Some(st);
        }
        if Instant::now() > end {
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// `kill j-XXXX` of a live job ends its supervisor, and the supervisor ends its job (the escapee
/// too); a 4-digit prefix is enough.
#[test]
fn kill_job_ends_a_live_job() {
    let d = scratch("live");
    let s = state(&d);
    let (mut c, g, r) = live_job(&d, &s, "a", |_| {});
    let id = job_id(&s);
    let code = kill(&s, &[&id[..6]], &[]);
    let sup_ended = finished(&mut c, 15).is_some();
    let alive = common::alive(g);
    if !sup_ended {
        common::send_child(&mut c, libc::SIGKILL);
        let _ = c.wait();
    }
    cleanup(&[&r]);
    assert_eq!(code, Some(0));
    assert!(sup_ended, "the supervisor did not end");
    assert!(!alive, "the escapee survived");
    let _ = std::fs::remove_dir_all(&d);
}

/// A stopped supervisor gets the CONT with its TERM, so `kill j-` still ends the job.
#[test]
fn kill_job_ends_a_stopped_supervisor() {
    let d = scratch("stopped");
    let s = state(&d);
    let (mut c, g, r) = live_job(&d, &s, "a", |_| {});
    let sup = (c.id() as i32, sheepdog::ident::identity(c.id() as i32).unwrap());
    common::send(sup.0, sup.1, libc::SIGSTOP);
    let code = kill(&s, &[&job_id(&s)], &[]);
    let sup_ended = finished(&mut c, 15).is_some();
    let alive = common::alive(g);
    if !sup_ended {
        common::send_child(&mut c, libc::SIGKILL);
        let _ = c.wait();
    }
    cleanup(&[&r]);
    assert_eq!(code, Some(0));
    assert!(sup_ended && !alive, "supervisor ended {sup_ended}, escapee alive {alive}");
    let _ = std::fs::remove_dir_all(&d);
}

/// `kill j-` of a dead job (its supervisor SIGKILLed) sweeps that job: the escapee dies and the
/// journal is removed.
#[test]
fn kill_job_sweeps_a_dead_job() {
    let d = scratch("dead");
    let s = state(&d);
    let (mut c, g, r) = live_job(&d, &s, "a", |_| {});
    let id = job_id(&s);
    common::send_child(&mut c, libc::SIGKILL);
    let _ = c.wait();
    let before = common::alive(g);
    let code = kill(&s, &[&id], &[]);
    let alive = common::alive(g);
    cleanup(&[&r]);
    assert!(before, "control: the escapee outlived its supervisor");
    assert_eq!(code, Some(0));
    assert!(!alive, "the dead job's escapee survived");
    assert!(journals(&s).is_empty(), "the journal is removed");
    let _ = std::fs::remove_dir_all(&d);
}

/// A supervisor whose caller ignores TERM does not end on the kill's TERM: `kill j-` exits 125
/// and leaves the supervisor alone (no escalation).
#[test]
fn kill_job_of_a_supervisor_that_ignores_term_is_125() {
    use std::os::unix::process::CommandExt;
    let d = scratch("ignterm");
    let s = state(&d);
    let (mut c, _g, r) = live_job(&d, &s, "a", |cmd| unsafe {
        cmd.pre_exec(|| {
            libc::signal(libc::SIGTERM, libc::SIG_IGN);
            Ok(())
        });
    });
    let code = kill(&s, &[&job_id(&s)], &[("SHEEPDOG_TEST_DEADLINE_MS", "500")]);
    let sup_alive = finished(&mut c, 1).is_none();
    common::send_child(&mut c, libc::SIGKILL);
    let _ = c.wait();
    cleanup(&[&r]);
    assert_eq!(code, Some(125));
    assert!(sup_alive, "the supervisor was killed anyway");
    let _ = std::fs::remove_dir_all(&d);
}

/// A process inside a job naming its own job is refused (its supervisor is its ancestor): exit 1,
/// the job runs on.
#[test]
fn kill_job_from_inside_the_job_is_refused() {
    let d = scratch("inside");
    let s = state(&d);
    let (idf, rc, rr) = (d.join("id"), d.join("rc"), d.join("root"));
    let script = format!(
        r#"while [ ! -s "{}" ]; do sleep 0.02; done; sheepdog kill "$(cat "{}")"; echo $? > "{}"; exec "$FX" sigcount "{}""#,
        idf.display(),
        idf.display(),
        rc.display(),
        rr.display()
    );
    let mut c = Command::new(sheepdog()).args(["run", "--", "/bin/sh", "-c", &script]).env("FX", fixture()).env("SHEEPDOG_TEST_STATE", &s).spawn().unwrap();
    assert!(wait_until(10, || !journals(&s).is_empty()));
    std::fs::write(&idf, job_id(&s)).unwrap();
    let got = wait_until(15, || std::fs::read_to_string(&rc).is_ok_and(|t| !t.trim().is_empty()));
    let code: Option<i32> = std::fs::read_to_string(&rc).ok().and_then(|t| t.trim().parse().ok());
    let running = finished(&mut c, 1).is_none();
    common::send_child(&mut c, libc::SIGTERM);
    let _ = finished(&mut c, 15);
    cleanup(&[&rr]);
    assert!(got);
    assert_eq!(code, Some(1), "refused");
    assert!(running, "the job ended");
    let _ = std::fs::remove_dir_all(&d);
}

/// An ambiguous prefix is refused (exit 2); an unknown id is exit 1; an id without `j-` that is
/// all digits is a pid (here a gone one: exit 0); one that is not digits is a usage error (2).
#[test]
fn job_ids_are_parsed_strictly() {
    let d = scratch("parse");
    let s = state(&d);
    let dir = {
        let (mut c, _g, r) = live_job(&d, &s, "probe", |_| {});
        let j = journals(&s)[0].clone();
        common::send_child(&mut c, libc::SIGTERM);
        let _ = finished(&mut c, 15);
        cleanup(&[&r]);
        j.parent().unwrap().to_path_buf()
    };
    for id in ["j-abcd0001", "j-abcd0002"] {
        std::fs::write(dir.join(format!("{id}.journal")), b"").unwrap();
    }
    assert_eq!(kill(&s, &["j-abcd"], &[]), Some(2), "ambiguous");
    assert_eq!(kill(&s, &["j-ffff"], &[]), Some(1), "unknown");
    assert_eq!(kill(&s, &["abcd"], &[]), Some(2), "not a pid, no j-");
    assert_eq!(kill(&s, &["j-ab"], &[]), Some(2), "a prefix shorter than 4 hex digits");
    let _ = std::fs::remove_dir_all(&d);
}

/// `kill PID:ID` is refused when the pid is no longer that process (exit 1, the process gets no
/// signal); with the right identity it is killed (the control).
#[test]
fn kill_pid_id_checks_the_identity() {
    let d = scratch("pidid");
    let s = state(&d);
    let r = d.join("decoy");
    let mut c = Command::new(fixture()).arg("sigcount").arg(&r).spawn().unwrap();
    assert!(wait_until(10, || !records(&r).is_empty()));
    let p = records(&r)[0];
    let wrong = kill(&s, &[&format!("{}:{}", p.0, p.1 + (1 << 40))], &[]);
    let (alive, n) = (common::alive(p), counted(&r));
    let right = kill(&s, &[&format!("{}:{}", p.0, p.1)], &[]);
    let gone = finished(&mut c, 10).is_some();
    if !gone {
        common::send_child(&mut c, libc::SIGKILL);
        let _ = c.wait();
    }
    assert_eq!(wrong, Some(1));
    assert!(alive && n == 0, "a mismatched pid:id was signalled ({n})");
    assert_eq!(right, Some(0));
    assert!(gone, "control: the right pid:id was killed");
    let _ = std::fs::remove_dir_all(&d);
}

/// The journal in `kill <pid>` (decision 12): in a dead job with two workers, `kill <worker A>`
/// ends A and A's own double-forked escapee (the journal's lineage: its intermediate was scanned
/// alive), never worker B.
#[test]
fn kill_pid_proves_the_targets_subtree_from_the_journal() {
    let d = scratch("lineage");
    let s = state(&d);
    let (ra, rb, go, rr) = (d.join("a"), d.join("b"), d.join("go"), d.join("root"));
    let script = format!(
        r#""$FX" worker "{}" "{}" & "$FX" sigcount "{}" & exec "$FX" sigcount "{}""#,
        ra.display(),
        go.display(),
        rb.display(),
        rr.display()
    );
    let mut c = Command::new(sheepdog()).args(["run", "--", "/bin/sh", "-c", &script]).env("FX", fixture()).env("SHEEPDOG_TEST_STATE", &s).spawn().unwrap();
    assert!(wait_until(15, || records(&ra).len() >= 3 && !records(&rb).is_empty()), "the workers started");
    let (a, e, cc) = (records(&ra)[0], records(&ra)[1], records(&ra)[2]);
    let b = records(&rb)[0];
    assert!(wait_until(10, || [a.0, e.0, cc.0, b.0].iter().all(|p| journaled(&s).contains(p))), "all journaled (the intermediate alive)");
    std::fs::write(&go, b"").unwrap(); // the intermediate exits: E is reparented
    assert!(wait_until(10, || !common::alive(cc)));
    common::send_child(&mut c, libc::SIGKILL);
    let _ = c.wait();
    let code = kill(&s, &[&a.0.to_string()], &[]);
    let (a_alive, e_alive, b_alive, bn) = (common::alive(a), common::alive(e), common::alive(b), counted(&rb));
    for p in [a, e, b].into_iter().chain(records(&rr)) {
        common::send(p.0, p.1, libc::SIGKILL);
    }
    assert_eq!(code, Some(0));
    assert!(!a_alive, "the target survived");
    assert!(!e_alive, "the target's own escapee survived (the journal's lineage was not used)");
    assert!(b_alive && bn == 0, "worker B was reached ({bn} signals)");
    let _ = std::fs::remove_dir_all(&d);
}

/// The wall control for `kill PID:ID` (a phase-2 source): an untagged process the test built is
/// withheld (its counter 0, a `withheld` line in the cell's own sink).
#[test]
fn kill_pid_id_turns_the_latch_on() {
    let d = scratch("latch");
    let s = state(&d);
    let r = d.join("untagged");
    let mut c = Command::new("/usr/bin/env").args(["-i", fixture(), "sigcount"]).arg(&r).spawn().unwrap();
    assert!(wait_until(10, || !records(&r).is_empty()));
    let p = records(&r)[0];
    let sink = d.join("sink");
    let mut cmd = Command::new(sheepdog());
    cmd.args(["kill", &format!("{}:{}", p.0, p.1)]).env("SHEEPDOG_TEST_STATE", &s).env("SHEEPDOG_TEST_DEADLINE_MS", "500");
    common::cell_sink(&mut cmd, &sink);
    let _ = cmd.status();
    let (alive, n) = (common::alive(p), counted(&r));
    let lines = std::fs::read_to_string(&sink).unwrap_or_default();
    common::send_child(&mut c, libc::SIGKILL);
    let _ = c.wait();
    assert!(alive && n == 0, "the untagged process got {n} signal(s)");
    assert!(lines.lines().any(|l| l.starts_with("withheld ") && l.split_whitespace().nth(1) == Some(&p.0.to_string())), "{lines:?}");
    let _ = std::fs::remove_dir_all(&d);
}

/// A job whose supervisor pid now belongs to another process (the header's identity does not
/// match) is a dead job: it is swept, and that process gets no signal (its counter stays 0).
#[test]
fn a_reused_supervisor_pid_is_a_dead_job() {
    let d = scratch("reused");
    let s = state(&d);
    // the folder and a real header, from a probe job
    let (mut c, _g, r) = live_job(&d, &s, "probe", |_| {});
    let probe = journals(&s)[0].clone();
    let header = std::fs::read_to_string(&probe).unwrap().lines().next().unwrap().to_string();
    common::send_child(&mut c, libc::SIGTERM);
    let _ = finished(&mut c, 15);
    cleanup(&[&r]);
    let rd = d.join("decoy");
    let mut decoy = Command::new(fixture()).arg("sigcount").arg(&rd).spawn().unwrap();
    assert!(wait_until(10, || !records(&rd).is_empty()));
    let p = records(&rd)[0];
    // a header like the probe's, with the supervisor at the decoy's pid under a wrong identity
    let h = json::parse(&header).unwrap();
    let field = |k: &str| h.get(k).and_then(Json::str).unwrap().to_string();
    let forged = format!(
        "{{\"v\":1,\"kind\":\"header\",\"job\":\"j-5eed0001\",\"boot\":\"{}\",\"pidns\":\"{}\",\"owner\":\"default\",\"uid\":{},\"sup\":{{\"pid\":{},\"id\":{}}},\"argv\":\"sheepdog run\"}}\n",
        field("boot"),
        field("pidns"),
        unsafe { libc::getuid() },
        p.0,
        p.1 + (1 << 40)
    );
    let path = probe.parent().unwrap().join("j-5eed0001.journal");
    std::fs::write(&path, forged).unwrap();
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    let code = kill(&s, &["j-5eed"], &[]);
    let (alive, n) = (common::alive(p), counted(&rd));
    common::send_child(&mut decoy, libc::SIGKILL);
    let _ = decoy.wait();
    assert_eq!(code, Some(0));
    assert!(alive && n == 0, "the process at the reused supervisor pid got {n} signal(s)");
    assert!(!path.exists(), "the dead job's journal was swept");
    let _ = std::fs::remove_dir_all(&d);
}


/// Review P1-A: `kill --dry-run j-` of a dead job signals nothing and removes nothing (the
/// escapee's counter stays 0, the journal stays); the control without `--dry-run` kills it.
#[test]
fn dry_run_of_a_dead_job_touches_nothing() {
    let d = scratch("dryrun");
    let s = state(&d);
    let (mut c, g, r) = live_job(&d, &s, "a", |_| {});
    let id = job_id(&s);
    common::send_child(&mut c, libc::SIGKILL);
    let _ = c.wait();
    let gr = PathBuf::from(format!("{}.g", r.display()));
    let code = kill(&s, &["--dry-run", &id], &[]);
    let (alive, n, kept) = (common::alive(g), counted(&gr), !journals(&s).is_empty());
    let control = kill(&s, &[&id], &[]);
    let gone = !common::alive(g);
    cleanup(&[&r]);
    assert_eq!(code, Some(0));
    assert!(alive && n == 0, "a dry run signalled the escapee ({n})");
    assert!(kept, "a dry run removed the journal");
    assert_eq!(control, Some(0));
    assert!(gone, "control: without --dry-run the escapee dies");
    let _ = std::fs::remove_dir_all(&d);
}

/// Review P1-B: the journal's lineage in `kill <pid>` never reaches the kill's own caller. W's
/// journaled subtree holds E (E was journaled under C under W, then reparented), and E runs
/// `sheepdog kill W`: the kill is refused whole (exit 1), E and W get no signal.
#[test]
fn the_journal_lineage_never_reaches_the_caller() {
    let d = scratch("caller");
    let s = state(&d);
    let (r, go1, go2, rc, rr) = (d.join("rec"), d.join("go1"), d.join("go2"), d.join("rc"), d.join("root"));
    let script = format!(
        r#""$FX" lineage-kill "{}" "{}" "{}" "{}" & exec "$FX" sigcount "{}""#,
        r.display(),
        go1.display(),
        go2.display(),
        rc.display(),
        rr.display()
    );
    let mut c = Command::new(sheepdog()).args(["run", "--", "/bin/sh", "-c", &script]).env("FX", fixture()).env("SHEEPDOG_TEST_STATE", &s).spawn().unwrap();
    assert!(wait_until(15, || records(&r).len() >= 3), "W, E and C started");
    let (w, e, cc) = (records(&r)[0], records(&r)[1], records(&r)[2]);
    assert!(wait_until(10, || [w.0, e.0, cc.0].iter().all(|p| journaled(&s).contains(p))), "journaled while C lived");
    std::fs::write(&go1, b"").unwrap();
    assert!(wait_until(10, || !common::alive(cc)), "C exited (E reparented)");
    common::send_child(&mut c, libc::SIGKILL);
    let _ = c.wait();
    std::fs::write(&go2, b"").unwrap();
    let got = wait_until(20, || std::fs::read_to_string(&rc).is_ok_and(|t| !t.trim().is_empty()));
    let code = std::fs::read_to_string(&rc).unwrap_or_default().trim().to_string();
    let (e_alive, e_stopped, w_alive) = (common::alive(e), common::stopped(e), common::alive(w));
    for p in [w, e].into_iter().chain(records(&rr)) {
        common::send(p.0, p.1, libc::SIGKILL);
    }
    assert!(got, "the kill did not finish (did it stop its own caller?)");
    assert_eq!(code, "1", "refused");
    // for this reason: the journaled subtree holds E (a failed ancestor walk is also exit 1)
    let err = std::fs::read_to_string(format!("{}.err", rc.display())).unwrap_or_default();
    assert!(err.contains(&format!("journaled subtree holds pid {},", e.0)), "the refusal is not the lineage refusal for pid {}: {err}", e.0);
    assert!(e_alive && !e_stopped, "the kill signalled its own caller");
    assert!(w_alive, "a refused kill signalled the target");
    let _ = std::fs::remove_dir_all(&d);
}

/// Review P2-1: `kill j-` of a live job is a phase-2 source (its supervisor comes from a
/// journal): a forged header naming an untagged process the test built, with its correct
/// identity, gets it withheld (counter 0, a `withheld` line in the cell's own sink).
#[test]
fn kill_job_turns_the_latch_on() {
    let d = scratch("joblatch");
    let s = state(&d);
    let (mut c, _g, r) = live_job(&d, &s, "probe", |_| {});
    let probe = journals(&s)[0].clone();
    let header = std::fs::read_to_string(&probe).unwrap().lines().next().unwrap().to_string();
    common::send_child(&mut c, libc::SIGTERM);
    let _ = finished(&mut c, 15);
    cleanup(&[&r]);
    let ru = d.join("untagged");
    let mut un = Command::new("/usr/bin/env").args(["-i", fixture(), "sigcount"]).arg(&ru).spawn().unwrap();
    assert!(wait_until(10, || !records(&ru).is_empty()));
    let p = records(&ru)[0];
    let h = json::parse(&header).unwrap();
    let field = |k: &str| h.get(k).and_then(Json::str).unwrap().to_string();
    let forged = format!(
        "{{\"v\":1,\"kind\":\"header\",\"job\":\"j-5eed0002\",\"boot\":\"{}\",\"pidns\":\"{}\",\"owner\":\"default\",\"uid\":{},\"sup\":{{\"pid\":{},\"id\":{}}},\"argv\":\"sheepdog run\"}}\n",
        field("boot"),
        field("pidns"),
        unsafe { libc::getuid() },
        p.0,
        p.1
    );
    let path = probe.parent().unwrap().join("j-5eed0002.journal");
    std::fs::write(&path, forged).unwrap();
    let sink = d.join("sink");
    let mut cmd = Command::new(sheepdog());
    cmd.args(["kill", "j-5eed0002"]).env("SHEEPDOG_TEST_STATE", &s).env("SHEEPDOG_TEST_DEADLINE_MS", "500");
    common::cell_sink(&mut cmd, &sink);
    let _ = cmd.status();
    let (alive, n) = (common::alive(p), counted(&ru));
    let lines = std::fs::read_to_string(&sink).unwrap_or_default();
    common::send_child(&mut un, libc::SIGKILL);
    let _ = un.wait();
    assert!(alive && n == 0, "the untagged 'supervisor' got {n} signal(s)");
    assert!(lines.lines().any(|l| l.starts_with("withheld ") && l.split_whitespace().nth(1) == Some(&p.0.to_string())), "{lines:?}");
    let _ = std::fs::remove_dir_all(&d);
}

/// Review P2-8: `kill j-` of a dead job keeps the sweep's fences: a journal in this folder whose
/// header names another boot is refused (exit 1), its member gets no signal.
#[test]
fn kill_job_keeps_the_sweep_fences() {
    let d = scratch("jobfence");
    let s = state(&d);
    let (mut c, _g, r) = live_job(&d, &s, "probe", |_| {});
    let probe = journals(&s)[0].clone();
    let header = std::fs::read_to_string(&probe).unwrap().lines().next().unwrap().to_string();
    common::send_child(&mut c, libc::SIGTERM);
    let _ = finished(&mut c, 15);
    cleanup(&[&r]);
    let rd = d.join("decoy");
    let mut decoy = Command::new(fixture()).arg("sigcount").arg(&rd).spawn().unwrap();
    assert!(wait_until(10, || !records(&rd).is_empty()));
    let p = records(&rd)[0];
    let h = json::parse(&header).unwrap();
    let pidns = h.get("pidns").and_then(Json::str).unwrap().to_string();
    let forged = format!(
        "{{\"v\":1,\"kind\":\"header\",\"job\":\"j-5eed0003\",\"boot\":\"00000000-0000-0000-0000-000000000000\",\"pidns\":\"{pidns}\",\"owner\":\"default\",\"uid\":{},\"sup\":{{\"pid\":999999,\"id\":1}},\"argv\":\"sheepdog run\"}}\n{{\"v\":1,\"pid\":{},\"id\":{},\"ppid\":1,\"pid_id\":null,\"puniq\":null,\"cmd\":\"decoy\"}}\n",
        unsafe { libc::getuid() },
        p.0,
        p.1
    );
    let path = probe.parent().unwrap().join("j-5eed0003.journal");
    std::fs::write(&path, forged).unwrap();
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    let code = kill(&s, &["j-5eed0003"], &[]);
    let (alive, n) = (common::alive(p), counted(&rd));
    common::send_child(&mut decoy, libc::SIGKILL);
    let _ = decoy.wait();
    assert_eq!(code, Some(1), "refused");
    assert!(alive && n == 0, "another boot's journal was swept ({n})");
    assert!(path.exists(), "the journal was deleted");
    let _ = std::fs::remove_dir_all(&d);
}

/// `kill j-` of a dead job that carries the leave-strays mark refuses, touches nothing, and says
/// why: the operator chose to leave those strays.
#[test]
fn kill_job_of_a_leave_strays_job_names_the_reason() {
    use std::io::Write;
    let d = scratch("leave");
    let s = state(&d);
    let (mut c, g, r) = live_job(&d, &s, "a", |_| {});
    let id = job_id(&s);
    common::send_child(&mut c, libc::SIGKILL);
    let _ = c.wait();
    let j = journals(&s).pop().unwrap();
    std::fs::OpenOptions::new().append(true).open(&j).unwrap().write_all(b"{\"v\":1,\"kind\":\"leave-strays\"}\n").unwrap();
    let out = Command::new(sheepdog()).args(["kill", &id]).env("SHEEPDOG_TEST_STATE", &s).output().unwrap();
    let alive = common::alive(g);
    cleanup(&[&r]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1));
    assert!(alive, "the leave-strays job's escapee was signalled");
    assert!(err.contains("--leave-strays"), "the refusal does not name the reason: {err}");
    let _ = std::fs::remove_dir_all(&d);
}
