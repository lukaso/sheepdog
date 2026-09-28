//! Phase-2 P2: `--status-fd` (PLAN.md §3.1; PHASE2.md §1 decision 8), the exit-code order, the
//! kill report (PLAN.md §10.4) and `--quiet`. Cells read the status JSON, exit codes, pids on
//! stderr and the trace; never the report's wording.

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
    let d = std::env::temp_dir().join(format!("sd-st-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn marker() -> String {
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos();
    format!("29.{:09}", n)
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

struct Run {
    code: Option<i32>,
    status: Option<Json>,
    lines: usize,
    stderr: String,
}

/// `sheepdog run ARGS` with `--status-fd 3` pointed at a file (through a shell, `exec`), within
/// 30 s; `pre` runs with the child (e.g. to send it TERM).
fn run(d: &Path, name: &str, args: &str, env: &[(&str, &str)], pre: impl FnOnce(&mut std::process::Child)) -> Run {
    let out = d.join(format!("{name}.status"));
    let err = d.join(format!("{name}.err"));
    let mut cmd = Command::new("/bin/sh");
    cmd.args(["-c", &format!(r#"exec "$SD" run --status-fd 3 {args} 3>"{}" 2>"{}""#, out.display(), err.display())])
        .env("SD", sheepdog())
        .env("FX", fixture());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut c = cmd.spawn().unwrap();
    pre(&mut c);
    let end = Instant::now() + Duration::from_secs(30);
    let code = loop {
        if let Some(s) = c.try_wait().unwrap() {
            break s.code();
        }
        if Instant::now() > end {
            common::send_child(&mut c, libc::SIGKILL);
            let _ = c.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let text = std::fs::read_to_string(&out).unwrap_or_default();
    Run {
        code,
        status: text.lines().next().and_then(|l| json::parse(l).ok()),
        lines: text.lines().count(),
        stderr: std::fs::read_to_string(&err).unwrap_or_default(),
    }
}

fn field<'a>(r: &'a Run, k: &str) -> Option<&'a Json> {
    r.status.as_ref().and_then(|s| s.get(k))
}

/// Cell 26: three different status lines for three runs that all exit 125 (a command that
/// exits 125 by itself, a usage error, and a deadline with a live member), and an external TERM
/// gives `trigger: "term"`; a command that exits 143 by itself does not.
#[test]
fn cell_26_the_status_line_tells_the_125s_apart() {
    let d = scratch("c26");
    let by_itself = run(&d, "self", r#"-- /bin/sh -c 'exit 125'"#, &[], |_| {});
    let usage = run(&d, "usage", r#"--no-such-flag -- /bin/sh -c 'exit 0'"#, &[], |_| {});
    let m = marker();
    let r = d.join("rec");
    let deadline = run(
        &d,
        "deadline",
        &format!(r#"--grace 0 -- "$FX" escape {m} "{}""#, r.display()),
        &[("SHEEPDOG_TEST_NOKILL", "1"), ("SHEEPDOG_TEST_DEADLINE_MS", "500")],
        |_| {},
    );
    let g = records(&r, 1);
    for p in &g {
        common::send(p.0, p.1, libc::SIGKILL);
    }
    for (n, x) in [("self", &by_itself), ("usage", &usage), ("deadline", &deadline)] {
        assert_eq!(x.code, Some(125), "{n}");
        assert_eq!(x.lines, 1, "{n}: one status line");
        assert_eq!(field(x, "code").and_then(Json::num), Some(125.0), "{n}");
    }
    assert_eq!(field(&by_itself, "error"), Some(&Json::Null));
    assert_eq!(field(&by_itself, "deadline_missed"), Some(&Json::Bool(false)));
    assert_eq!(field(&by_itself, "root").and_then(Json::str), Some("exited"));
    assert!(field(&usage, "error").and_then(Json::str).is_some(), "the usage error is named");
    assert_eq!(field(&deadline, "deadline_missed"), Some(&Json::Bool(true)));
    let survivors: Vec<i64> = field(&deadline, "survivors").and_then(Json::arr).unwrap_or(&[]).iter().filter_map(Json::num).map(|n| n as i64).collect();
    assert!(g.iter().all(|p| survivors.contains(&(p.0 as i64))), "survivors {survivors:?}, escapee {g:?}");
    // TERM from outside, against a command that exits 143 by itself
    let ready = d.join("ready");
    let termed = run(&d, "term", &format!(r#"-- /bin/sh -c 'touch "{}"; sleep 30'"#, ready.display()), &[], |c| {
        let end = Instant::now() + Duration::from_secs(20);
        while !ready.exists() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(5));
        }
        common::send_child(c, libc::SIGTERM);
    });
    let own143 = run(&d, "own143", r#"-- /bin/sh -c 'exit 143'"#, &[], |_| {});
    assert_eq!(field(&termed, "trigger").and_then(Json::str), Some("term"));
    assert!(field(&termed, "trigger_at").and_then(Json::num).is_some(), "trigger_at in ms");
    assert_eq!(own143.code, Some(143));
    assert_eq!(field(&own143, "trigger"), Some(&Json::Null), "a command's own 143 is no trigger");
    let _ = std::fs::remove_dir_all(&d);
}

/// Cell 26: the status fd is set CLOEXEC, so the root does not hold it. The fd is 57, far above
/// the low fds a translator keeps open (Rosetta holds 3 and up on the emulated leg).
#[test]
fn the_root_does_not_hold_the_status_fd() {
    let d = scratch("cloexec");
    let (flag, out) = (d.join("open"), d.join("status57"));
    // fd 57 is set up here, not by a shell: dash redirects single-digit fds only
    let file = std::fs::File::create(&out).unwrap();
    let raw = std::os::fd::AsRawFd::as_raw_fd(&file);
    let mut cmd = Command::new(sheepdog());
    cmd.args(["run", "--status-fd", "57", "--", "/bin/sh", "-c", &format!(r#"if [ -e /dev/fd/57 ]; then touch "{}"; fi"#, flag.display())]);
    unsafe {
        std::os::unix::process::CommandExt::pre_exec(&mut cmd, move || {
            if libc::dup2(raw, 57) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let code = cmd.status().unwrap().code();
    drop(file);
    assert_eq!(code, Some(0));
    assert_eq!(std::fs::read_to_string(&out).unwrap_or_default().lines().count(), 1, "control: sheepdog wrote its line to fd 57");
    assert!(!flag.exists(), "the root holds the status fd");
    let _ = std::fs::remove_dir_all(&d);
}

/// `killed[].escaped` names how each killed process escaped: `setsid` for a new session,
/// `reparented` for a double fork in the same session; the report on stderr names every killed
/// escapee's pid.
#[test]
fn killed_processes_say_how_they_escaped() {
    let d = scratch("escaped");
    let (m1, m2) = (marker(), marker());
    let (r1, r2) = (d.join("r1"), d.join("r2"));
    let x = run(
        &d,
        "esc",
        &format!(r#"--grace 0 -- /bin/sh -c '"$FX" escape {m1} "{}"; "$FX" doublefork {m2} "{}"'"#, r1.display(), r2.display()),
        &[],
        |_| {},
    );
    let (s, f) = (records(&r1, 1), records(&r2, 1));
    for p in s.iter().chain(f.iter()) {
        common::send(p.0, p.1, libc::SIGKILL);
    }
    assert_eq!(x.code, Some(0));
    let killed = field(&x, "killed").and_then(Json::arr).unwrap_or(&[]).to_vec();
    let how = |pid: i32| killed.iter().find(|k| k.get("pid").and_then(Json::num) == Some(pid as f64)).and_then(|k| k.get("escaped").and_then(Json::str).map(String::from));
    assert_eq!(how(s[0].0), Some("setsid".to_string()), "{killed:?}");
    assert_eq!(how(f[0].0), Some("reparented".to_string()), "{killed:?}");
    for p in [s[0].0, f[0].0] {
        assert!(x.stderr.split(|c: char| !c.is_ascii_digit()).any(|w| w == p.to_string()), "the report does not name pid {p}:\n{}", x.stderr);
    }
    let _ = std::fs::remove_dir_all(&d);
}

/// `tracking` names the mechanism and `degraded` is null when tracking is complete; the report's
/// last line says "clean" only then (the trace records which ending was printed). macOS: with
/// the responsibility SPI forced missing, `degraded` names why and the report never says clean.
#[test]
fn the_report_says_clean_only_when_tracking_was_complete() {
    let d = scratch("clean");
    let m = marker();
    let r = d.join("rec");
    let trace = d.join("trace");
    let full = run(&d, "full", &format!(r#"--grace 0 -- "$FX" escape {m} "{}""#, r.display()), &[("SHEEPDOG_TEST_TRACE", trace.to_str().unwrap())], |_| {});
    for p in records(&r, 1) {
        common::send(p.0, p.1, libc::SIGKILL);
    }
    let expect = if cfg!(target_os = "macos") { "responsibility" } else { "subreaper" };
    assert_eq!(field(&full, "tracking").and_then(Json::str), Some(expect));
    assert_eq!(field(&full, "degraded"), Some(&Json::Null));
    let notes = std::fs::read_to_string(&trace).unwrap_or_default();
    assert!(notes.lines().any(|l| l == "report clean"), "{notes}");
    if cfg!(target_os = "macos") {
        let m = marker();
        let r = d.join("rec2");
        let trace2 = d.join("trace2");
        // a background child of the root: the degraded tracking (puniq) still sees it, so the
        // kill has something to report (a setsid escapee would not be seen at all)
        let _ = r;
        let deg = run(
            &d,
            "degraded",
            &format!(r#"--grace 0 -- /bin/sh -c '/bin/sleep {m} & exit 0'"#),
            &[("SHEEPDOG_TEST_SPI", "broken"), ("SHEEPDOG_TEST_TRACE", trace2.to_str().unwrap())],
            |_| {},
        );
        common::kill_marked(&[&m]);
        let killed = deg.status.as_ref().and_then(|s| s.get("killed")).and_then(Json::arr).map_or(0, |a| a.len());
        assert!(killed > 0, "control: the degraded run killed something");
        assert!(field(&deg, "degraded").and_then(Json::str).is_some(), "degraded is named");
        let notes = std::fs::read_to_string(&trace2).unwrap_or_default();
        assert!(notes.lines().any(|l| l == "report degraded"), "{notes}");
        assert!(!notes.lines().any(|l| l == "report clean"), "{notes}");
    }
    let _ = std::fs::remove_dir_all(&d);
}

/// `--quiet`: a kill with escapees prints nothing on stderr (the machine reads `--status-fd`);
/// the control run without `--quiet` prints the report.
#[test]
fn quiet_suppresses_the_report() {
    let d = scratch("quiet");
    let mut stderr = Vec::new();
    for quiet in [false, true] {
        let m = marker();
        let r = d.join(format!("rec-{quiet}"));
        let flag = if quiet { "--quiet " } else { "" };
        let x = run(&d, &format!("q{quiet}"), &format!(r#"{flag}--grace 0 -- "$FX" escape {m} "{}""#, r.display()), &[], |_| {});
        for p in records(&r, 1) {
            common::send(p.0, p.1, libc::SIGKILL);
        }
        assert_eq!(x.code, Some(0));
        stderr.push(x.stderr);
    }
    assert!(!stderr[0].trim().is_empty(), "control: the report is printed without --quiet");
    assert!(stderr[1].trim().is_empty(), "--quiet printed: {}", stderr[1]);
    let _ = std::fs::remove_dir_all(&d);
}

/// Review P2-5: `--status-fd` 0, 1 or 2 would take the command's own stream (it is set
/// close-on-exec): a usage error (125), and the command does not run.
#[test]
fn the_status_fd_is_never_a_standard_stream() {
    let d = scratch("stdfd");
    for fd in ["0", "1", "2"] {
        let ran = d.join(format!("ran{fd}"));
        let st = Command::new(sheepdog()).args(["run", "--status-fd", fd, "--", "/bin/sh", "-c", &format!(r#"touch "{}""#, ran.display())]).stderr(std::process::Stdio::null()).status().unwrap();
        assert_eq!(st.code(), Some(125), "--status-fd {fd}");
        assert!(!ran.exists(), "--status-fd {fd}: the command ran");
    }
    let _ = std::fs::remove_dir_all(&d);
}
