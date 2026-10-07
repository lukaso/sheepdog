//! Phase-2 P2: `--status-fd` (PLAN.md §3.1; PHASE2.md §1 decision 8), the exit-code order, the
//! kill report (PLAN.md §10.4) and `--quiet`. Cells read the status JSON, exit codes, pids on
//! stderr and the trace; never the report's wording.

mod common;

use common::json::{self, Json};
use std::path::{Path, PathBuf};
use std::process::Command;
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
    let d = std::env::temp_dir().join(format!("sr-st-{name}-{}", std::process::id()));
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

/// `sheepr run ARGS` with `--status-fd 3` pointed at a file (through a shell, `exec`), within
/// 30 s; `pre` runs with the child (e.g. to send it TERM).
fn run(d: &Path, name: &str, args: &str, env: &[(&str, &str)], pre: impl FnOnce(&mut std::process::Child)) -> Run {
    let out = d.join(format!("{name}.status"));
    let err = d.join(format!("{name}.err"));
    let mut cmd = Command::new("/bin/sh");
    cmd.args(["-c", &format!(r#"exec "$SD" run --status-fd 3 {args} 3>"{}" 2>"{}""#, out.display(), err.display())])
        .env("SD", sheepr())
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
        &[("SHEEPR_TEST_NOKILL", "1"), ("SHEEPR_TEST_DEADLINE_MS", "500")],
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

/// A command that cannot be run: exit 127 (126 when it is not executable), `root`
/// "not-started" and an `error` that names the failure; the control, a command that runs and
/// exits 127 by itself, has no error.
#[test]
fn a_command_that_cannot_run_names_the_error() {
    let d = scratch("spawnerr");
    let missing = run(&d, "missing", "-- /nonexistent/x", &[], |_| {});
    let noexec = d.join("noexec");
    std::fs::write(&noexec, "#!/bin/sh\n").unwrap();
    let denied = run(&d, "denied", &format!("-- {}", noexec.display()), &[], |_| {});
    let own = run(&d, "own", r#"-- /bin/sh -c 'exit 127'"#, &[], |_| {});
    for (x, code) in [(&missing, 127), (&denied, 126)] {
        assert_eq!(x.code, Some(code), "{:?}", x.status);
        assert_eq!(field(x, "root").and_then(Json::str), Some("not-started"), "{:?}", x.status);
        assert!(field(x, "error").and_then(Json::str).is_some_and(|e| e.contains("cannot run")), "{:?}", x.status);
    }
    assert_eq!(own.code, Some(127));
    assert_eq!(field(&own, "error"), Some(&Json::Null), "control: {:?}", own.status);
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
    let mut cmd = Command::new(sheepr());
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
    assert_eq!(std::fs::read_to_string(&out).unwrap_or_default().lines().count(), 1, "control: sheepr wrote its line to fd 57");
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
    let full = run(&d, "full", &format!(r#"--grace 0 -- "$FX" escape {m} "{}""#, r.display()), &[("SHEEPR_TEST_TRACE", trace.to_str().unwrap())], |_| {});
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
            &[("SHEEPR_TEST_SPI", "broken"), ("SHEEPR_TEST_TRACE", trace2.to_str().unwrap())],
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

#[derive(Clone, Copy, PartialEq)]
enum Reader {
    /// the test holds the read end and never reads it
    Stuck,
    /// a thread reads to the newline
    Drain,
    /// the read end is closed before sheepr starts
    Closed,
}

struct Piped {
    code: Option<i32>,
    stderr: String,
    trace: String,
    got: Vec<u8>,
    /// O_NONBLOCK on the test's own copy of the write end (the same open file description),
    /// read after sheepr exited; None when the test kept no copy
    nonblock_after: Option<bool>,
}

/// `sheepr run --quiet --status-fd 3 -- /bin/sh -c 'exit 7'` with fd 3 the write end of a pipe the
/// test owns and the status line padded by `pad` bytes (debug seam), within 20 s (then KILL, code
/// None). The test keeps its own copy of the write end unless the reader is `Closed`.
fn piped(d: &Path, name: &str, reader: Reader, pad: usize) -> Piped {
    let (r, w) = std::io::pipe().unwrap();
    let keep = (reader != Reader::Closed).then(|| w.try_clone().unwrap());
    let r = if reader == Reader::Closed {
        drop(r);
        None
    } else {
        Some(r)
    };
    let trace = d.join(format!("{name}.trace"));
    let err = d.join(format!("{name}.err"));
    let mut cmd = Command::new(sheepr());
    cmd.args(["run", "--quiet", "--status-fd", "3", "--", "/bin/sh", "-c", "exit 7"])
        .env("SHEEPR_TEST_STATUS_PAD", pad.to_string())
        .env("SHEEPR_TEST_TRACE", &trace)
        .stderr(std::fs::File::create(&err).unwrap());
    let raw = std::os::fd::AsRawFd::as_raw_fd(&w);
    unsafe {
        std::os::unix::process::CommandExt::pre_exec(&mut cmd, move || {
            // dup2 onto itself would keep CLOEXEC
            let ok = if raw == 3 { libc::fcntl(3, libc::F_SETFD, 0) } else { libc::dup2(raw, 3) };
            if ok < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut c = cmd.spawn().unwrap();
    drop(w);
    let drain = match (reader, r) {
        (Reader::Drain, Some(mut r)) => Some(std::thread::spawn(move || {
            let mut got = Vec::new();
            let mut buf = [0u8; 65536];
            while !got.contains(&b'\n') {
                match std::io::Read::read(&mut r, &mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => got.extend_from_slice(&buf[..n]),
                }
            }
            (got, None)
        })),
        (_, r) => Some(std::thread::spawn(move || (Vec::new(), r))),
    };
    let end = Instant::now() + Duration::from_secs(20);
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
    let nonblock_after = keep.as_ref().map(|k| unsafe { libc::fcntl(std::os::fd::AsRawFd::as_raw_fd(k), libc::F_GETFL) } & libc::O_NONBLOCK != 0);
    drop(keep);
    let (got, held) = drain.map(|t| t.join().unwrap()).unwrap_or_default();
    drop(held); // the stuck reader is released only now, after sheepr's end
    Piped { code, stderr: std::fs::read_to_string(&err).unwrap_or_default(), trace: std::fs::read_to_string(&trace).unwrap_or_default(), got, nonblock_after }
}

/// The trace's `status-undelivered CAUSE MS SENT/TOTAL` note, if any.
fn undelivered(p: &Piped) -> Option<(String, u64)> {
    let l = p.trace.lines().find(|l| l.starts_with("status-undelivered "))?;
    let mut w = l.split_whitespace().skip(1);
    Some((w.next()?.to_string(), w.next()?.parse().ok()?))
}

/// Issue #14: the status line never holds sheepr's exit. Against a line larger than any pipe
/// buffer (1 MiB of debug pad) and a reader that never reads, sheepr exits with the command's
/// code after waiting its 1 s for the reader and no more, says on stderr (also under `--quiet`)
/// that the line was not delivered, and leaves the caller's write end blocking as it found it.
/// The control drains: the whole line arrives, and stderr and the trace say nothing.
#[test]
fn a_reader_that_never_reads_cannot_hold_sheeprs_exit() {
    let d = scratch("stuck");
    let pad = 1 << 20;
    let stuck = piped(&d, "stuck", Reader::Stuck, pad);
    assert_eq!(stuck.code, Some(7), "sheepr did not exit within 20 s (or not with the command's code); trace:\n{}", stuck.trace);
    let (cause, ms) = undelivered(&stuck).unwrap_or_else(|| panic!("no status-undelivered note:\n{}", stuck.trace));
    assert_eq!(cause, "timeout");
    assert!((1000..5000).contains(&ms), "waited {ms} ms for the reader, not its 1 s");
    assert!(!stuck.stderr.trim().is_empty(), "nothing on stderr says the status was lost");
    assert_eq!(stuck.nonblock_after, Some(false), "the caller's write end was left non-blocking");
    let drain = piped(&d, "drain", Reader::Drain, pad);
    assert_eq!(drain.code, Some(7), "control");
    let line = String::from_utf8_lossy(&drain.got);
    let st = json::parse(line.trim_end()).unwrap_or_else(|e| panic!("control: the line does not parse ({e}); {} bytes", drain.got.len()));
    assert_eq!(st.get("code").and_then(Json::num), Some(7.0), "control");
    assert!(drain.got.len() > pad, "control: {} bytes arrived", drain.got.len());
    assert_eq!(undelivered(&drain), None, "control:\n{}", drain.trace);
    assert!(drain.stderr.trim().is_empty(), "control: --quiet printed {}", drain.stderr);
    assert_eq!(drain.nonblock_after, Some(false), "control: the caller's write end was left non-blocking");
    let _ = std::fs::remove_dir_all(&d);
}

/// Issue #14: a reader that has closed its end gets no line, and sheepr says so on stderr (also
/// under `--quiet`) and keeps the command's code; a closed pipe is never a SIGPIPE death.
#[test]
fn a_closed_status_reader_is_said_on_stderr() {
    let d = scratch("closed");
    let x = piped(&d, "closed", Reader::Closed, 0);
    assert_eq!(x.code, Some(7), "{}", x.trace);
    assert_eq!(undelivered(&x).map(|u| u.0), Some("closed".to_string()), "{}", x.trace);
    assert!(!x.stderr.trim().is_empty(), "nothing on stderr says the status was lost");
    let _ = std::fs::remove_dir_all(&d);
}

/// Review P2-5: `--status-fd` 0, 1 or 2 would take the command's own stream (it is set
/// close-on-exec): a usage error (125), and the command does not run.
#[test]
fn the_status_fd_is_never_a_standard_stream() {
    let d = scratch("stdfd");
    for fd in ["0", "1", "2"] {
        let ran = d.join(format!("ran{fd}"));
        let st = Command::new(sheepr()).args(["run", "--status-fd", fd, "--", "/bin/sh", "-c", &format!(r#"touch "{}""#, ran.display())]).stderr(std::process::Stdio::null()).status().unwrap();
        assert_eq!(st.code(), Some(125), "--status-fd {fd}");
        assert!(!ran.exists(), "--status-fd {fd}: the command ran");
    }
    let _ = std::fs::remove_dir_all(&d);
}
