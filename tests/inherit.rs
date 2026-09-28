//! Phase-2 P8: `--inherit-terminal-permissions`, `doctor`, the D10 start warning, `--version`
//! (PLAN.md §4.3, §4.4, §10.7; PHASE2.md "P8 design").
//!
//! The inherit-mode cells run under T (PHASE2 §0): a test-made fixture that is responsible for
//! itself and runs each "tab" as its own child, so the job's responsible process is T, never the
//! operator's terminal. A mutant that widens R to the supervisor's responsible process then
//! reaches only T's tree. Preconditions (red, never a skip): T is responsible for itself; the
//! supervisor and every decoy are responsible to T.

mod common;

use common::json::{self, Json};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

static SEQ: AtomicUsize = AtomicUsize::new(0);

fn sheepdog() -> &'static str {
    common::test_env();
    env!("CARGO_BIN_EXE_sheepdog")
}
fn fixture() -> &'static str {
    common::test_env();
    env!("CARGO_BIN_EXE_sd-fixture")
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("sd-inh-{name}-{}-{}", std::process::id(), SEQ.fetch_add(1, Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[cfg(target_os = "linux")]
fn marker() -> String {
    format!("27.{:04}{:03}", std::process::id() % 10_000, SEQ.fetch_add(1, Ordering::SeqCst) % 1000)
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
    while Instant::now() < end {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    f()
}

fn parent(pid: i32) -> i32 {
    String::from_utf8_lossy(&Command::new("ps").args(["-o", "ppid=", "-p", &pid.to_string()]).output().unwrap().stdout).trim().parse().unwrap_or(0)
}

/// Every recorded process of these files and every process carrying one of the markers is
/// SIGKILLed (by identity) when the guard drops, a panic too.
struct Guard {
    recs: Vec<PathBuf>,
    markers: Vec<String>,
    children: Vec<Child>,
}
impl Drop for Guard {
    fn drop(&mut self) {
        for r in &self.recs {
            for suffix in ["", ".g", ".root"] {
                for p in records(&PathBuf::from(format!("{}{suffix}", r.display()))) {
                    common::send(p.0, p.1, libc::SIGKILL);
                }
            }
        }
        common::kill_marked(&self.markers.iter().map(String::as_str).collect::<Vec<_>>());
        for c in &mut self.children {
            common::send_child(c, libc::SIGKILL);
            let _ = c.wait();
        }
    }
}

/// macOS: the uniqueid of the process responsible for `pid` (the private SPI, as sheepdog reads it).
#[cfg(target_os = "macos")]
fn resp_of(pid: i32) -> Option<u64> {
    type F = unsafe extern "C" fn(libc::pid_t) -> u64;
    let name = std::ffi::CString::new("responsibility_get_uniqueid_responsible_for_pid").unwrap();
    let f = unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr()) };
    if f.is_null() {
        return None;
    }
    let f: F = unsafe { std::mem::transmute(f) };
    let v = unsafe { f(pid) };
    (v != 0 && v != u64::MAX).then_some(v)
}

/// Every pid named in the journals under state directory `st` (a supervisor journals what its
/// scans see: the readiness handshake waits for a line here, never for a fixed time).
fn journaled(st: &Path) -> Vec<i32> {
    let mut v = Vec::new();
    for b in std::fs::read_dir(st.join("jobs")).into_iter().flatten().flatten() {
        for f in std::fs::read_dir(b.path()).into_iter().flatten().flatten() {
            for l in std::fs::read_to_string(f.path()).unwrap_or_default().lines() {
                if let Some(p) = json::parse(l).ok().and_then(|j| j.get("pid").and_then(Json::num)) {
                    v.push(p as i32);
                }
            }
        }
    }
    v
}

/// Start T (a disclaimed fixture) running PROG ARGS as its child tab; T's (pid, uniqueid), after
/// the precondition that T is responsible for itself.
#[cfg(target_os = "macos")]
fn t_launch(d: &Path, g: &mut Guard, prog: &str, args: &[&str]) -> (i32, u64) {
    t_launch_env(d, g, prog, args, &[])
}

#[cfg(target_os = "macos")]
fn t_launch_env(d: &Path, g: &mut Guard, prog: &str, args: &[&str], env: &[(&str, &Path)]) -> (i32, u64) {
    let tr = d.join("T");
    let c = Command::new(fixture()).arg("t").arg(&tr).arg(prog).args(args).envs(env.iter().copied()).stdin(Stdio::null()).spawn().unwrap();
    g.children.push(c);
    g.recs.push(tr.clone());
    assert!(wait_until(15, || !records(&tr).is_empty()), "T did not start");
    let t = records(&tr)[0];
    assert_eq!(resp_of(t.0), Some(t.1), "precondition: T is responsible for itself");
    t
}

/// The supervisor above the job's first recorded process, with the precondition that it is
/// responsible to T (inherit mode: it did not disclaim).
#[cfg(target_os = "macos")]
fn supervisor_under_t(t: (i32, u64), first: (i32, u64)) -> (i32, u64) {
    let sup = common::found(parent(first.0)).expect("the supervisor");
    assert_eq!(resp_of(sup.0), Some(t.1), "precondition: the supervisor is responsible to T (no disclaim)");
    sup
}

/// Cells 2, 3 and 12 in inherit mode (under T): the job's escapees are members by the `puniq`
/// facts sheepdog saw, and none survives the job's end.
#[cfg(target_os = "macos")]
#[test]
fn cell2_inherit_a_setsid_grandchild_leaves_no_survivor() {
    let d = scratch("c2");
    let r = d.join("r");
    let rg = PathBuf::from(format!("{}.g", r.display()));
    let mut g = Guard { recs: vec![r.clone()], markers: vec![], children: vec![] };
    let t = t_launch(&d, &mut g, sheepdog(), &["run", "--quiet", "--inherit-terminal-permissions", "--", fixture(), "setsid-kid-fx", r.to_str().unwrap()]);
    assert!(wait_until(15, || !records(&r).is_empty() && !records(&rg).is_empty()), "the tree did not start");
    let sup = supervisor_under_t(t, records(&r)[0]);
    let all: Vec<(i32, u64)> = records(&r).into_iter().chain(records(&rg)).collect();
    common::send(sup.0, sup.1, libc::SIGTERM);
    let left = wait_until(20, || all.iter().all(|&p| !common::alive(p)));
    assert!(left, "survivors: {:?}", all.iter().filter(|&&p| common::alive(p)).collect::<Vec<_>>());
    drop(g);
    let _ = std::fs::remove_dir_all(&d);
}

#[cfg(target_os = "macos")]
#[test]
fn cell3_inherit_an_escapee_seen_by_a_scan_leaves_no_survivor() {
    let d = scratch("c3");
    let (r, go) = (d.join("r"), d.join("go"));
    let st = state(&d);
    let mut g = Guard { recs: vec![r.clone()], markers: vec![], children: vec![] };
    let t = t_launch_env(&d, &mut g, sheepdog(), &["run", "--quiet", "--inherit-terminal-permissions", "--", fixture(), "escape-after", r.to_str().unwrap(), go.to_str().unwrap()], &[("SHEEPDOG_TEST_STATE", &st)]);
    assert!(wait_until(15, || !records(&r).is_empty()), "C did not start");
    let sup = supervisor_under_t(t, records(&r)[0]);
    // the readiness handshake: C escapes only after a supervisor scan has seen it (journaled)
    let c = records(&r)[0];
    assert!(wait_until(10, || journaled(&st).contains(&c.0)), "the supervisor never journaled C");
    std::fs::write(&go, b"").unwrap();
    let rg = PathBuf::from(format!("{}.g", r.display()));
    assert!(wait_until(15, || !records(&rg).is_empty()), "G did not start");
    let esc = records(&rg)[0];
    common::send(sup.0, sup.1, libc::SIGTERM);
    let gone = wait_until(20, || !common::alive(esc));
    assert!(gone, "the escapee survived");
    drop(g);
    let _ = std::fs::remove_dir_all(&d);
}

#[cfg(target_os = "macos")]
#[test]
fn cell12_inherit_a_clean_exit_with_a_stray_leaves_no_survivor() {
    let d = scratch("c12");
    let (s, rootf, go) = (d.join("s"), d.join("root"), d.join("go"));
    let mut g = Guard { recs: vec![s.clone()], markers: vec![], children: vec![] };
    // the root exits by itself, after the test has checked the supervisor's precondition
    let script = format!(r#"echo $$ > "{}"; "{}" sigcount "{}" & while [ ! -e "{}" ]; do sleep 0.01; done; exit 0"#, rootf.display(), fixture(), s.display(), go.display());
    let t = t_launch(&d, &mut g, sheepdog(), &["run", "--quiet", "--inherit-terminal-permissions", "--", "/bin/sh", "-c", &script]);
    assert!(wait_until(15, || std::fs::read_to_string(&rootf).is_ok_and(|x| x.ends_with('\n')) && !records(&s).is_empty()), "the job did not start");
    let root: i32 = std::fs::read_to_string(&rootf).unwrap().trim().parse().unwrap();
    let rid = sheepdog::ident::identity(root).expect("the root");
    supervisor_under_t(t, (root, rid));
    std::fs::write(&go, b"").unwrap();
    let code = {
        let end = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(s) = g.children[0].try_wait().unwrap() {
                break s.code();
            }
            assert!(Instant::now() < end, "the job did not end");
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    // (T exits 4 when its disclaim did not take effect: the exit code is the precondition here)
    let survivors: Vec<(i32, u64)> = records(&s).into_iter().filter(|&p| common::alive(p)).collect();
    assert_eq!(code, Some(0));
    assert!(!records(&s).is_empty(), "the stray did not start");
    assert!(survivors.is_empty(), "survivors: {survivors:?}");
    drop(g);
    let _ = std::fs::remove_dir_all(&d);
}

/// The other-tab cell: T runs a job in one tab and a decoy in another; both are responsible to
/// T. Ending the job leaves the decoy alive and unsignalled (R is the supervisor, never its
/// responsible process).
#[cfg(target_os = "macos")]
#[test]
fn another_tabs_process_survives_an_inherit_mode_job() {
    let d = scratch("tab");
    let (a, decoy) = (d.join("a"), d.join("decoy"));
    let mut g = Guard { recs: vec![a.clone(), decoy.clone()], markers: vec![], children: vec![] };
    let script = format!(r#""$0" run --quiet --inherit-terminal-permissions -- "$1" sigcount "{}" & "$1" sigcount "{}" & wait"#, a.display(), decoy.display());
    let t = t_launch(&d, &mut g, "/bin/sh", &["-c", &script, sheepdog(), fixture()]);
    assert!(wait_until(15, || !records(&a).is_empty() && !records(&decoy).is_empty()), "the tabs did not start");
    let (ap, dp) = (records(&a)[0], records(&decoy)[0]);
    assert_eq!(resp_of(dp.0), Some(t.1), "precondition: the decoy is responsible to T");
    let sup = supervisor_under_t(t, ap);
    common::send(sup.0, sup.1, libc::SIGTERM);
    let job_gone = wait_until(20, || !common::alive(ap) && !common::alive(sup));
    let sigs = std::fs::read_to_string(format!("{}.sig", decoy.display())).unwrap_or_default().lines().count();
    assert!(job_gone, "the job did not end");
    assert!(common::alive(t), "T, an ancestor of the supervisor, was killed");
    assert!(common::alive(dp) && sigs == 0, "the other tab's decoy got {sigs} signal(s), alive {}", common::alive(dp));
    drop(g);
    let _ = std::fs::remove_dir_all(&d);
}

/// Cell 17 (macOS): an app opened with `open -g` inside a job is launchd's child (`puniq` 1),
/// not the job's: it survives the job's end (the documented leak).
#[cfg(target_os = "macos")]
#[test]
fn cell17_an_open_g_app_survives_the_job() {
    let d = scratch("c17");
    let bundle = d.join("SdFake17.app");
    let macos = bundle.join("Contents/MacOS");
    std::fs::create_dir_all(&macos).unwrap();
    std::fs::copy(fixture(), macos.join("SdFake17")).unwrap();
    std::fs::write(
        bundle.join("Contents/Info.plist"),
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>SdFake17</string>
<key>CFBundleIdentifier</key><string>com.lukaso.sheepdog.test.fake17</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>LSUIElement</key><true/>
</dict></plist>
"#,
    )
    .unwrap();
    let r = d.join("app");
    let g = Guard { recs: vec![r.clone()], markers: vec![], children: vec![] };
    let st = Command::new(sheepdog())
        .args(["run", "--quiet", "--", "/usr/bin/open", "-g", "-n", bundle.to_str().unwrap(), "--args", "sigcount", r.to_str().unwrap()])
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert!(wait_until(20, || !records(&r).is_empty()), "the app did not start");
    let app = records(&r)[0];
    let survived = wait_until(2, || common::alive(app)) && { std::thread::sleep(Duration::from_millis(500)); common::alive(app) };
    assert_eq!(st.code(), Some(0));
    assert!(survived, "the open -g app did not survive (the leak this cell documents is gone: update PLAN cell 17)");
    drop(g);
    let _ = std::fs::remove_dir_all(&d);
}

/// `doctor --json` probes every call: with the responsibility SPI forced broken it reports a
/// degraded mechanism; the next call without the seam reports none (no cached answer).
#[test]
fn doctor_reports_what_is_degraded_now() {
    let run = |env: &[(&str, &str)]| -> Json {
        let mut c = Command::new(sheepdog());
        c.args(["doctor", "--json"]);
        for (k, v) in env {
            c.env(k, v);
        }
        let o = c.output().unwrap();
        assert_eq!(o.status.code(), Some(0), "{}", String::from_utf8_lossy(&o.stderr));
        json::parse(String::from_utf8_lossy(&o.stdout).trim()).expect("doctor --json is one JSON object")
    };
    let degraded = |j: &Json| j.get("degraded").and_then(Json::arr).map_or(usize::MAX, |a| a.len());
    if cfg!(target_os = "macos") {
        let broken = run(&[("SHEEPDOG_TEST_SPI", "broken")]);
        assert!(degraded(&broken) >= 1 && degraded(&broken) != usize::MAX, "the forced missing SPI is not degraded: {broken:?}");
    }
    let plain = run(&[]);
    assert_eq!(degraded(&plain), 0, "a plain doctor reports degraded mechanisms: {plain:?}");
    assert!(plain.get("mechanisms").and_then(Json::arr).is_some_and(|a| !a.is_empty()), "{plain:?}");
}

/// The doctor's disclaim self-check needs a control (D6): a process whose responsible process
/// died reads as responsible for itself, so a disclaimed probe child proves nothing unless a
/// child spawned WITHOUT the disclaim is not. A doctor whose responsible process (T) has exited
/// says it cannot tell (a note, and no "ok" for the check); the control, a doctor under a live
/// T, reports the check ok.
#[cfg(target_os = "macos")]
#[test]
fn the_doctor_disclaim_check_has_a_control() {
    let d = scratch("docctl");
    let check = |j: &Json| -> (Option<bool>, bool) {
        let ok = j.get("mechanisms").and_then(Json::arr).and_then(|a| a.iter().find(|m| m.get("name").and_then(Json::str) == Some("disclaim self-check"))).and_then(|m| m.get("ok")).and_then(|o| if *o == Json::Bool(true) { Some(true) } else if *o == Json::Bool(false) { Some(false) } else { None });
        let cannot = j.get("notes").and_then(Json::arr).is_some_and(|a| a.iter().filter_map(Json::str).any(|n| n.starts_with("disclaim self-check: cannot tell")));
        (ok, cannot)
    };
    // control: the doctor is T's tab, T alive
    let o = Command::new(fixture()).arg("t").arg(d.join("T1")).args([sheepdog(), "doctor", "--json"]).output().unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", String::from_utf8_lossy(&o.stderr));
    let live = json::parse(String::from_utf8_lossy(&o.stdout).trim()).expect("one JSON object");
    // the doctor starts only after T (its responsible process) has exited
    let (go, out) = (d.join("go"), d.join("orphan.json"));
    let tab = format!(r#""$FX" after "{}" "$SD" doctor --json > "{}" 2>/dev/null & exit 0"#, go.display(), out.display());
    let t = Command::new(fixture()).arg("t").arg(d.join("T2")).args(["/bin/sh", "-c", &tab]).env("FX", fixture()).env("SD", sheepdog()).status().unwrap();
    assert_eq!(t.code(), Some(0), "T did not run its tab");
    let tp = std::fs::read_to_string(d.join("T2")).unwrap_or_default().split_whitespace().next().and_then(|p| p.parse::<i32>().ok());
    assert!(tp.is_some_and(|p| wait_until(5, || unsafe { libc::kill(p, 0) } != 0)), "T is still alive");
    std::fs::write(&go, b"").unwrap();
    assert!(wait_until(20, || std::fs::read_to_string(&out).is_ok_and(|t| t.ends_with('\n'))), "the orphaned doctor did not answer");
    let orphan = json::parse(std::fs::read_to_string(&out).unwrap().trim()).expect("one JSON object");
    assert_eq!(check(&live), (Some(true), false), "control: {live:?}");
    let (ok, cannot) = check(&orphan);
    assert!(cannot && ok != Some(true), "the orphaned doctor reported the check ok: {orphan:?}");
    let _ = std::fs::remove_dir_all(&d);
}

/// D10: the start warning fires for a protected folder whose probe fails (the seam's forced
/// EPERM), not for a plain folder, and not under `--inherit-terminal-permissions`.
#[cfg(target_os = "macos")]
#[test]
fn the_privacy_warning_fires_only_for_a_refused_protected_folder() {
    let d = scratch("tcc");
    let (prot, plain) = (d.join("prot"), d.join("plain"));
    std::fs::create_dir_all(&prot).unwrap();
    std::fs::create_dir_all(&plain).unwrap();
    let warned = |cwd: &Path, flags: &[&str]| -> bool {
        let trace = d.join(format!("trace-{}", SEQ.fetch_add(1, Ordering::SeqCst)));
        // an inherit-mode sheepdog runs only under T (its responsible process is T, never the
        // operator's terminal)
        let mut c = if flags.contains(&"--inherit-terminal-permissions") {
            let mut c = Command::new(fixture());
            c.arg("t").arg(d.join(format!("T-{}", SEQ.fetch_add(1, Ordering::SeqCst)))).arg(sheepdog());
            c
        } else {
            Command::new(sheepdog())
        };
        let st = c
            .arg("run")
            .args(flags)
            .args(["--", "/usr/bin/true"])
            .current_dir(cwd)
            .env("SHEEPDOG_TEST_TCC_PROTECTED", &prot)
            .env("SHEEPDOG_TEST_TRACE", &trace)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert_eq!(st.code(), Some(0));
        std::fs::read_to_string(&trace).unwrap_or_default().lines().any(|l| l.starts_with("tcc-warning "))
    };
    assert!(warned(&prot, &[]), "no warning for the refused protected folder");
    assert!(!warned(&plain, &[]), "a warning for a plain folder");
    assert!(!warned(&prot, &["--inherit-terminal-permissions"]), "a warning under --inherit-terminal-permissions");
    let _ = std::fs::remove_dir_all(&d);
}

/// D10: an argument counts only when it is written as a path (it has a `/`): `ssh host ls prot`
/// names a remote folder, and probing the local one could raise a privacy prompt the job never
/// needs. From a plain cwd, a bare `prot` gives no warning; `./prot` and the absolute path do.
#[cfg(target_os = "macos")]
#[test]
fn the_privacy_warning_counts_only_arguments_written_as_paths() {
    let d = scratch("tccargs");
    let prot = d.join("prot");
    std::fs::create_dir_all(&prot).unwrap();
    let warned = |arg: &str| -> bool {
        let trace = d.join(format!("trace-{}", SEQ.fetch_add(1, Ordering::SeqCst)));
        let st = Command::new(sheepdog())
            .args(["run", "--", "/bin/echo", arg])
            .current_dir(&d)
            .env("SHEEPDOG_TEST_TCC_PROTECTED", &prot)
            .env("SHEEPDOG_TEST_TRACE", &trace)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert_eq!(st.code(), Some(0));
        std::fs::read_to_string(&trace).unwrap_or_default().lines().any(|l| l.starts_with("tcc-warning "))
    };
    assert!(!warned("prot"), "a bare word was probed as a local path");
    assert!(warned("./prot"), "control: ./prot is a path");
    assert!(warned(prot.to_str().unwrap()), "control: the absolute path");
    let _ = std::fs::remove_dir_all(&d);
}

/// D10 follows the disclaim, not its read-back: with the responsibility getter broken (seam) the
/// disclaimed re-exec still happens and the job loses the terminal's permissions, so the run
/// warns; with the disclaim symbol absent (seam) there is no re-exec, the job keeps the
/// terminal's permissions, and there is no warning; a plain run warns.
#[cfg(target_os = "macos")]
#[test]
fn the_privacy_warning_follows_the_disclaim() {
    let d = scratch("tccnodisc");
    let prot = d.join("prot");
    std::fs::create_dir_all(&prot).unwrap();
    let warned = |env: &[(&str, &str)]| -> bool {
        let trace = d.join(format!("trace-{}", SEQ.fetch_add(1, Ordering::SeqCst)));
        let st = Command::new(sheepdog())
            .args(["run", "--", "/usr/bin/true"])
            .current_dir(&prot)
            .env("SHEEPDOG_TEST_TCC_PROTECTED", &prot)
            .env("SHEEPDOG_TEST_TRACE", &trace)
            .envs(env.iter().copied())
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert_eq!(st.code(), Some(0));
        std::fs::read_to_string(&trace).unwrap_or_default().lines().any(|l| l.starts_with("tcc-warning "))
    };
    assert!(warned(&[("SHEEPDOG_TEST_SPI", "broken")]), "no warning, though the disclaimed re-exec happened");
    assert!(!warned(&[("SHEEPDOG_TEST_SPI", "nodisclaim")]), "a warning, though there was no disclaim");
    assert!(warned(&[]), "control: a plain run warns");
    let _ = std::fs::remove_dir_all(&d);
}

/// The D10 probe is bounded: a probe that blocks (a dead network volume; seam) gives up in
/// well under a second and the run goes on (it exits 0 at once, with a trace that the probe
/// timed out); the control, a cwd outside the protected folders, has no such trace.
#[cfg(target_os = "macos")]
#[test]
fn a_blocked_privacy_probe_does_not_hold_the_start() {
    let d = scratch("tcchang");
    let (prot, plain) = (d.join("prot"), d.join("plain"));
    std::fs::create_dir_all(&prot).unwrap();
    std::fs::create_dir_all(&plain).unwrap();
    let go = |cwd: &Path| -> (Option<i32>, Duration, String) {
        let trace = d.join(format!("trace-{}", SEQ.fetch_add(1, Ordering::SeqCst)));
        let t0 = Instant::now();
        let mut c = Command::new(sheepdog())
            .args(["run", "--", "/usr/bin/true"])
            .current_dir(cwd)
            .env("SHEEPDOG_TEST_TCC_PROTECTED", &prot)
            .env("SHEEPDOG_TEST_TCC_PROBE_HANG", &prot)
            .env("SHEEPDOG_TEST_TRACE", &trace)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let ended = wait_until(20, || c.try_wait().ok().flatten().is_some());
        if !ended {
            common::send_child(&mut c, libc::SIGKILL);
        }
        let code = c.wait().ok().and_then(|s| s.code());
        (code, t0.elapsed(), std::fs::read_to_string(&trace).unwrap_or_default())
    };
    let (code, took, trace) = go(&prot);
    let (ccode, _, ctrace) = go(&plain);
    assert_eq!(code, Some(0), "the run did not finish ({took:?})");
    assert!(took < Duration::from_secs(5), "the blocked probe held the start {took:?}");
    assert!(trace.lines().any(|l| l == "tcc-probe timed out"), "the seam did not block the probe: {trace}");
    assert_eq!(ccode, Some(0));
    assert!(!ctrace.lines().any(|l| l == "tcc-probe timed out"), "control: {ctrace}");
    let _ = std::fs::remove_dir_all(&d);
}

/// Under the phase-1 opt-out every phase-2 source is disabled outright (PHASE2 §0.1), and
/// inherit mode is one (D1): an inherit-mode run then refuses (125) and runs nothing; the
/// control, the same run without the opt-out, runs its command. Both run under T.
#[cfg(target_os = "macos")]
#[test]
fn inherit_mode_refuses_under_the_phase1_opt_out() {
    let d = scratch("inhp1");
    let go = |name: &str, phase1: bool| -> (Option<i32>, bool) {
        let ran = d.join(format!("ran-{name}"));
        let mut c = Command::new(fixture());
        c.arg("t").arg(d.join(format!("T-{name}"))).args([sheepdog(), "run", "--quiet", "--inherit-terminal-permissions", "--", "/usr/bin/touch", ran.to_str().unwrap()]).stdin(Stdio::null()).stderr(Stdio::null());
        if phase1 {
            c.env("SHEEPDOG_TEST_PHASE1", "1");
        }
        let code = c.status().unwrap().code();
        (code, ran.exists())
    };
    let (code, ran) = go("p1", true);
    let (ccode, cran) = go("ctl", false);
    assert_eq!((code, ran), (Some(125), false), "inherit mode ran under the phase-1 opt-out");
    assert_eq!((ccode, cran), (Some(0), true), "control");
    let _ = std::fs::remove_dir_all(&d);
}

/// `--version` prints this build's version.
#[test]
fn version_prints_the_build_version() {
    let o = Command::new(sheepdog()).arg("--version").output().unwrap();
    assert_eq!(o.status.code(), Some(0));
    let line = String::from_utf8_lossy(&o.stdout).trim().to_string();
    assert!(line == format!("sheepdog {}", env!("CARGO_PKG_VERSION")) || line.starts_with(&format!("sheepdog {} ", env!("CARGO_PKG_VERSION"))), "{line}");
}

/// Linux: `--inherit-terminal-permissions` is accepted and changes nothing (a stray still dies).
#[cfg(target_os = "linux")]
#[test]
fn inherit_is_accepted_on_linux() {
    let m = marker();
    let script = format!("sleep {m} & exit 0");
    let st = Command::new(sheepdog()).args(["run", "--quiet", "--inherit-terminal-permissions", "--", "/bin/sh", "-c", &script]).status().unwrap();
    let left = common::scan(&m, |_| true).unwrap_or_default();
    common::kill_marked(&[&m]);
    assert_eq!(st.code(), Some(0));
    assert!(left.is_empty(), "survivors: {left:?}");
}

fn state(d: &Path) -> PathBuf {
    let s = d.join("state");
    std::fs::create_dir_all(&s).unwrap();
    std::fs::write(s.join(".sheepdog-test"), b"").unwrap();
    s
}

/// The auto-sweep's deferral in inherit mode (under T): an outer job holds a live inner job
/// (both `--inherit-terminal-permissions`, each with its own state); the outer supervisor is
/// SIGKILLed, and the next run's auto-sweep of the outer's journal sends the inner job's escapee
/// nothing: in this mode the escapee is not responsible to the inner supervisor, so only the live
/// journal and the lineage hold it. The control: an explicit `sweep` ends it.
#[cfg(target_os = "macos")]
#[test]
fn the_auto_sweep_defers_an_inherit_mode_inner_job() {
    let d = scratch("defer");
    let s = state(&d);
    let inner_state = state(&d.join("inner"));
    let (r, rr, ip, go) = (d.join("rec"), d.join("root"), d.join("innerpid"), d.join("go"));
    let mut g = Guard { recs: vec![r.clone(), rr.clone()], markers: vec![], children: vec![] };
    // the inner job's escapee escapes only after scans saw its parent (the handshake), so both
    // supervisors hold it by `puniq` (in this mode it is not responsible to them)
    let script = format!(
        r#"SHEEPDOG_TEST_STATE="{}" "$0" run --quiet --inherit-terminal-permissions -- "$1" escape-after "{}" "{}" & echo $! > "{}"; exec "$1" sigcount "{}""#,
        inner_state.display(),
        r.display(),
        go.display(),
        ip.display(),
        rr.display()
    );
    let tr = d.join("T");
    let t_child = Command::new(fixture())
        .arg("t")
        .arg(&tr)
        .args([sheepdog(), "run", "--quiet", "--inherit-terminal-permissions", "--", "/bin/sh", "-c", &script, sheepdog(), fixture()])
        .env("SHEEPDOG_TEST_STATE", &s)
        .stdin(Stdio::null())
        .spawn()
        .unwrap();
    g.children.push(t_child);
    assert!(wait_until(15, || !records(&tr).is_empty() && !records(&r).is_empty() && !records(&rr).is_empty()), "the jobs did not start");
    let t = records(&tr)[0];
    assert_eq!(resp_of(t.0), Some(t.1), "precondition: T is responsible for itself");
    // the handshake: both supervisors' scans have seen C (both journals name it)
    let c = records(&r)[0];
    assert!(wait_until(10, || journaled(&s).contains(&c.0) && journaled(&inner_state).contains(&c.0)), "the supervisors never journaled C");
    std::fs::write(&go, b"").unwrap();
    let rg = PathBuf::from(format!("{}.g", r.display()));
    assert!(wait_until(15, || !records(&rg).is_empty()), "the inner job's escapee did not start");
    let esc = records(&rg)[0];
    let inner: i32 = std::fs::read_to_string(&ip).unwrap_or_default().trim().parse().unwrap_or(0);
    assert!(inner > 1, "no inner supervisor");
    let outer = common::found(parent(records(&rr)[0].0)).expect("the outer supervisor");
    assert_eq!(resp_of(outer.0), Some(t.1), "precondition: the outer supervisor is responsible to T");
    assert_eq!(resp_of(esc.0), Some(t.1), "precondition: the inner job's escapee is responsible to T, not to its supervisor");
    // the outer has journaled the inner supervisor and the escapee (its scans, over a few ticks)
    assert!(wait_until(10, || journaled(&s).contains(&esc.0) && journaled(&s).contains(&inner)), "the outer did not journal the inner job's escapee {esc:?} (inner {inner}, root {:?}): {:?}; recs {:?}", records(&rr), journaled(&s), records(&r));
    common::send(outer.0, outer.1, libc::SIGKILL);
    assert!(wait_until(5, || !common::alive(outer)));
    let ran = d.join("ran");
    let st = Command::new(sheepdog())
        .args(["run", "--quiet", "--", "/usr/bin/touch", ran.to_str().unwrap()])
        .env("SHEEPDOG_TEST_STATE", &s)
        .env("SHEEPDOG_TEST_DEADLINE_MS", "500")
        .stdin(Stdio::null())
        .status()
        .unwrap();
    let sigs = std::fs::read_to_string(format!("{}.g.sig", r.display())).unwrap_or_default().lines().count();
    let kept = common::alive(esc) && sigs == 0;
    let _ = Command::new(sheepdog()).arg("sweep").env("SHEEPDOG_TEST_STATE", &s).status();
    let gone = wait_until(10, || !common::alive(esc));
    assert_eq!(st.code(), Some(0));
    assert!(ran.exists(), "the new command ran");
    assert!(kept, "the auto-sweep reached the live inner job's escapee: {sigs} signal(s), alive {}", common::alive(esc));
    assert!(gone, "control: the explicit sweep ended the inner job");
    drop(g);
    let _ = std::fs::remove_dir_all(&d);
}

/// The wall control for inherit mode (PHASE2.md §0.1, D1): an inherit-mode run is a phase-2
/// source (it tracks by `puniq` and ever-seen facts only), so it turns the latch on; under T, an
/// untagged stray of the job gets `withheld` (its counter stays 0, one `withheld` line names it).
#[cfg(target_os = "macos")]
#[test]
fn an_inherit_mode_run_turns_the_latch_on() {
    let d = scratch("latch");
    let (u, sink) = (d.join("u"), d.join("sink"));
    let mut g = Guard { recs: vec![u.clone()], markers: vec![], children: vec![] };
    // the root exits only after the stray has exec'd the fixture (it records itself then): an
    // exec after the stray was reparented would reset its puniq, and it would be no member
    let script = format!(r#"/usr/bin/env -i "{0}" sigcount "{1}" & while [ ! -s "{1}" ]; do sleep 0.01; done; exit 0"#, fixture(), u.display());
    let tr = d.join("T");
    let mut c = Command::new(fixture());
    c.arg("t").arg(&tr).args([sheepdog(), "run", "--quiet", "--inherit-terminal-permissions", "--", "/bin/sh", "-c", &script]).env("SHEEPDOG_TEST_DEADLINE_MS", "500").stdin(Stdio::null());
    common::cell_sink(&mut c, &sink);
    g.children.push(c.spawn().unwrap());
    assert!(wait_until(15, || !records(&u).is_empty()), "the stray did not start");
    let up = records(&u)[0];
    let ended = wait_until(30, || g.children[0].try_wait().ok().flatten().is_some());
    let sigs = std::fs::read_to_string(format!("{}.sig", u.display())).unwrap_or_default().lines().count();
    let lines = std::fs::read_to_string(&sink).unwrap_or_default();
    assert!(ended, "the job did not end");
    assert!(common::alive(up) && sigs == 0, "the untagged stray got {sigs} signal(s)");
    let mine: Vec<&str> = lines.lines().filter(|l| l.starts_with("withheld ")).collect();
    assert!(!mine.is_empty() && mine.iter().all(|l| l.split_whitespace().nth(1) == Some(&up.0.to_string())), "{lines:?}");
    drop(g);
    let _ = std::fs::remove_dir_all(&d);
}
