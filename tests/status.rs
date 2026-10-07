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
    /// a thread reads at most `.0` bytes every `.1` ms, to the newline
    Slow(usize, u64),
    /// a thread waits `.0` ms, then reads to the end of the pipe (all its writers gone)
    Stall(u64),
    /// no reader at all: the child makes the pipe and closes its read end before the exec, so no
    /// other process (another cell's child) can hold that end
    Closed,
}

struct Piped {
    code: Option<i32>,
    signal: Option<i32>,
    stderr: String,
    trace: String,
    /// what the reader got; a `Stuck` reader's is read after sheepr's end, to the end of the pipe
    got: Vec<u8>,
    /// from the spawn to sheepr's end
    ms: u128,
    /// O_NONBLOCK on the test's own copy of the write end (the same open file description),
    /// read after sheepr exited; None when the test kept no copy
    nonblock_after: Option<bool>,
}

/// Fill the pipe whose write end is `w` (non-blocking for the fill only), so the next blocking
/// write to it waits for a reader.
fn fill(w: &std::io::PipeWriter) {
    let fd = std::os::fd::AsRawFd::as_raw_fd(w);
    let fl = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    unsafe { libc::fcntl(fd, libc::F_SETFL, fl | libc::O_NONBLOCK) };
    for size in [65536, 1] {
        let chunk = vec![b'e'; size];
        while unsafe { libc::write(fd, chunk.as_ptr() as *const libc::c_void, size) } > 0 {}
    }
    unsafe { libc::fcntl(fd, libc::F_SETFL, fl) };
}

/// `sheepr run --quiet --status-fd 3 ARGS -- /bin/sh -c 'exit 7'` with fd 3 the write end of a
/// pipe and the status line padded by `pad` bytes (debug seam), within 20 s (then KILL, code
/// None). The test keeps its own copy of the write end unless the reader is `Closed`. With
/// `full_stderr`, stderr is a full pipe nobody reads (then `stderr` is empty).
fn piped(d: &Path, name: &str, reader: Reader, pad: usize, args: &[&str], env: &[(&str, &str)], full_stderr: bool) -> Piped {
    let trace = d.join(format!("{name}.trace"));
    let err = d.join(format!("{name}.err"));
    let mut cmd = Command::new(sheepr());
    cmd.args(["run", "--quiet", "--status-fd", "3"]).args(args).args(["--", "/bin/sh", "-c", "exit 7"]);
    cmd.env("SHEEPR_TEST_STATUS_PAD", pad.to_string()).env("SHEEPR_TEST_TRACE", &trace);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let held_err = if full_stderr {
        let (er, ew) = std::io::pipe().unwrap();
        fill(&ew);
        cmd.stderr(ew);
        Some(er)
    } else {
        cmd.stderr(std::fs::File::create(&err).unwrap());
        None
    };
    let (r, w) = if reader == Reader::Closed {
        unsafe {
            std::os::unix::process::CommandExt::pre_exec(&mut cmd, || {
                let mut p = [0; 2];
                if libc::pipe(p.as_mut_ptr()) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                libc::close(p[0]);
                if p[1] != 3 {
                    libc::dup2(p[1], 3);
                    libc::close(p[1]);
                }
                Ok(())
            });
        }
        (None, None)
    } else {
        let (r, w) = std::io::pipe().unwrap();
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
        (Some(r), Some(w))
    };
    let t0 = Instant::now();
    let mut c = cmd.spawn().unwrap();
    let keep = w; // the test's own copy of the write end, read for its flags after the end
    let reading = r.map(|mut r| {
        std::thread::spawn(move || {
            let mut got = Vec::new();
            let (step, every, to_end) = match reader {
                Reader::Slow(n, ms) => (n, ms, false),
                Reader::Stuck => return (got, Some(r)),
                Reader::Stall(ms) => {
                    std::thread::sleep(Duration::from_millis(ms));
                    (65536, 0, true)
                }
                _ => (65536, 0, false),
            };
            let mut buf = vec![0u8; step];
            while to_end || !got.contains(&b'\n') {
                match std::io::Read::read(&mut r, &mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => got.extend_from_slice(&buf[..n]),
                }
                std::thread::sleep(Duration::from_millis(every));
            }
            (got, None)
        })
    });
    let end = Instant::now() + Duration::from_secs(20);
    let st = loop {
        if let Some(s) = c.try_wait().unwrap() {
            break Some(s);
        }
        if Instant::now() > end {
            common::send_child(&mut c, libc::SIGKILL);
            let _ = c.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let ms = t0.elapsed().as_millis();
    let nonblock_after = keep.as_ref().map(|k| unsafe { libc::fcntl(std::os::fd::AsRawFd::as_raw_fd(k), libc::F_GETFL) } & libc::O_NONBLOCK != 0);
    drop(keep);
    let (mut got, held) = reading.map(|t| t.join().unwrap()).unwrap_or_default();
    // the stuck reader and the full stderr are released only now, after sheepr's end; what the
    // stuck reader's pipe holds is exactly what sheepr wrote (every writer is gone)
    if let Some(mut r) = held {
        let _ = std::io::Read::read_to_end(&mut r, &mut got);
    }
    drop(held_err);
    Piped {
        code: st.and_then(|s| s.code()),
        signal: st.and_then(|s| std::os::unix::process::ExitStatusExt::signal(&s)),
        stderr: std::fs::read_to_string(&err).unwrap_or_default(),
        trace: std::fs::read_to_string(&trace).unwrap_or_default(),
        got,
        ms,
        nonblock_after,
    }
}

/// The trace's `status-undelivered CAUSE MS SENT/TOTAL` note, if any: (cause, ms).
fn undelivered(p: &Piped) -> Option<(String, u64)> {
    let (cause, ms, _) = undelivered_sent(p)?;
    Some((cause, ms))
}

/// The same note with SENT: (cause, ms, sent).
fn undelivered_sent(p: &Piped) -> Option<(String, u64, usize)> {
    let l = p.trace.lines().find(|l| l.starts_with("status-undelivered "))?;
    let mut w = l.split_whitespace().skip(1);
    Some((w.next()?.to_string(), w.next()?.parse().ok()?, w.next()?.split('/').next()?.parse().ok()?))
}

/// The debug seam that shortens the 10 s idle limit for a cell.
const IDLE_1S: (&str, &str) = ("SHEEPR_TEST_STATUS_IDLE_MS", "1000");

/// The notice: under `--quiet` it is the only line, and it is sheepr's own.
fn one_notice(p: &Piped) -> bool {
    let l: Vec<&str> = p.stderr.lines().collect();
    l.len() == 1 && l[0].starts_with("sheepr: ")
}

/// The whole status line arrived and parses, with the command's code.
fn whole(p: &Piped, pad: usize) -> bool {
    p.got.len() > pad && p.got.ends_with(b"\n") && json::parse(String::from_utf8_lossy(&p.got).trim_end()).ok().and_then(|s| s.get("code").and_then(Json::num)) == Some(7.0)
}

/// Issue #14: the status line never holds sheepr's exit. Against a line larger than any pipe
/// buffer (1 MiB of debug pad) and a reader that never reads, sheepr exits with the command's
/// code once the fd has accepted nothing for its idle limit (a seam makes it 3 s here; 10 s by
/// default), and not much later (a buffer that fills at once is progress: the window starts
/// at the write that waits, so the give-up is one idle, not two); it says so in one stderr line (also under `--quiet`), and it
/// never touches the flags of the write end it was given. What the pipe holds after the end is
/// exactly the count the note gives (review of a19592d, F3: nothing writes after the give-up).
/// The control drains: the whole line arrives, and stderr and the trace say nothing.
#[test]
fn a_reader_that_never_reads_cannot_hold_sheeprs_exit() {
    let d = scratch("stuck");
    let pad = 1 << 20;
    let stuck = piped(&d, "stuck", Reader::Stuck, pad, &[], &[("SHEEPR_TEST_STATUS_IDLE_MS", "3000")], false);
    assert_eq!(stuck.code, Some(7), "sheepr did not exit within 20 s (or not with the command's code); trace:\n{}", stuck.trace);
    let (cause, ms, sent) = undelivered_sent(&stuck).unwrap_or_else(|| panic!("no status-undelivered note:\n{}", stuck.trace));
    assert_eq!(cause, "timeout");
    assert!((3000..5500).contains(&ms), "waited {ms} ms for the reader, not its 3 s");
    assert_eq!(stuck.got.len(), sent, "the note's count is not what the pipe holds");
    assert!(one_notice(&stuck), "stderr: {:?}", stuck.stderr);
    assert_eq!(stuck.nonblock_after, Some(false), "the caller's write end was left non-blocking");
    let drain = piped(&d, "drain", Reader::Drain, pad, &[], &[IDLE_1S], false);
    assert_eq!(drain.code, Some(7), "control");
    assert!(whole(&drain, pad), "control: {} bytes", drain.got.len());
    assert_eq!(undelivered(&drain), None, "control:\n{}", drain.trace);
    assert!(drain.stderr.trim().is_empty(), "control: --quiet printed {}", drain.stderr);
    assert_eq!(drain.nonblock_after, Some(false), "control: the caller's write end was left non-blocking");
    let _ = std::fs::remove_dir_all(&d);
}

/// Review of a1419bc, P2-3: the idle limit counts only the time the fd accepts nothing. A reader
/// that reads slowly but steadily (4 KiB every 25 ms) gets the whole 400 KiB line, which takes
/// it longer than the 1 s idle. A steady reader of a line too large for it is cut at the cap (a
/// seam makes it 3 s here, apart from the idle, review of a19592d F6; 30 s by default); the
/// control, a stuck reader under the same seams, is cut by the idle, before the cap. With a full
/// stderr too, sheepr still ends within the cap, one idle of tick, and the notice's 1 s (F5).
#[test]
fn a_slow_reader_gets_the_line_and_the_cap_still_ends_it() {
    let d = scratch("slow");
    let pad = 400 << 10;
    let cap = ("SHEEPR_TEST_STATUS_CAP_MS", "3000");
    let slow = piped(&d, "slow", Reader::Slow(4096, 25), pad, &[], &[IDLE_1S], false);
    assert_eq!(slow.code, Some(7), "{}", slow.trace);
    assert_eq!(undelivered(&slow), None, "a reader that kept reading was cut:\n{}", slow.trace);
    assert!(whole(&slow, pad), "{} bytes", slow.got.len());
    let capped = piped(&d, "capped", Reader::Slow(4096, 300), 1 << 20, &[], &[IDLE_1S, cap], false);
    assert_eq!(capped.code, Some(7), "sheepr did not exit within 20 s:\n{}", capped.trace);
    let (cause, ms) = undelivered(&capped).unwrap_or_else(|| panic!("no status-undelivered note:\n{}", capped.trace));
    assert_eq!(cause, "cap");
    assert!((3000..6000).contains(&ms), "capped at {ms} ms, not at 3 s");
    assert!(one_notice(&capped), "stderr: {:?}", capped.stderr);
    let stuck = piped(&d, "stuck", Reader::Stuck, 1 << 20, &[], &[IDLE_1S, cap], false);
    let (cause, ms) = undelivered(&stuck).unwrap_or_else(|| panic!("control: no note:\n{}", stuck.trace));
    assert_eq!(cause, "timeout", "control");
    assert!(ms < 3000, "control: {ms} ms, not the idle's 1 s");
    let full = piped(&d, "capped-full", Reader::Slow(4096, 300), 1 << 20, &[], &[IDLE_1S, cap], true);
    assert_eq!(full.code, Some(7), "{}", full.trace);
    assert!(full.ms < 3000 + 1000 + 1000 + 4000, "a capped line and a full stderr took {} ms", full.ms);
    let _ = std::fs::remove_dir_all(&d);
}

/// Issue #14: a reader that has closed its end gets no line, and sheepr says so on stderr (also
/// under `--quiet`) and keeps the command's code. Review of a1419bc, P2-1: a full stderr that
/// nobody reads cannot hold sheepr's exit either (the notice has a limit of its own).
#[test]
fn a_closed_status_reader_is_said_on_stderr() {
    let d = scratch("closed");
    let x = piped(&d, "closed", Reader::Closed, 0, &[], &[], false);
    assert_eq!(x.code, Some(7), "{}", x.trace);
    assert_eq!(undelivered(&x).map(|u| u.0), Some("closed".to_string()), "{}", x.trace);
    assert!(one_notice(&x), "stderr: {:?}", x.stderr);
    let full = piped(&d, "full", Reader::Closed, 0, &[], &[], true);
    assert_eq!(full.code, Some(7), "a full stderr held sheepr; trace:\n{}", full.trace);
    assert_eq!(undelivered(&full).map(|u| u.0), Some("closed".to_string()), "{}", full.trace);
    let _ = std::fs::remove_dir_all(&d);
}

/// Review of a1419bc, P2-2: before sheepr has blocked its signals (a usage error, a panic early
/// in `run`) SIGPIPE still has the caller's action, the default for most callers. A closed
/// status reader there is a closed reader like any other: exit 125, never death by SIGPIPE.
#[test]
fn a_closed_reader_before_the_job_is_not_a_sigpipe_death() {
    let d = scratch("early");
    let usage = piped(&d, "usage", Reader::Closed, 0, &["--timeout", "bad"], &[], false);
    let panic = piped(&d, "panic", Reader::Closed, 0, &[], &[("SHEEPR_TEST_PANIC_IN_RUN", "1")], false);
    for (n, x) in [("usage", &usage), ("panic", &panic)] {
        assert_eq!((x.code, x.signal), (Some(125), None), "{n}: {}", x.stderr);
        assert_eq!(undelivered(x).map(|u| u.0), Some("closed".to_string()), "{n}: {}", x.trace);
    }
    let _ = std::fs::remove_dir_all(&d);
}

/// Review of a19592d, F1: the line needs no new thread, so a process that cannot start one (a
/// pids limit the job used up; here a stack no thread can get) still delivers it whole.
#[test]
fn the_line_needs_no_new_thread() {
    let d = scratch("nothread");
    let pad = 1 << 20;
    let x = piped(&d, "nothread", Reader::Drain, pad, &[], &[("RUST_MIN_STACK", "1125899906842624")], false);
    assert_eq!(x.code, Some(7), "{}", x.trace);
    assert!(whole(&x, pad), "{} bytes; trace:\n{}", x.got.len(), x.trace);
    let _ = std::fs::remove_dir_all(&d);
}

/// Review of a19592d, F3: once sheepr gives up, nothing more is written. A reader that stalls
/// past the idle and then reads to the end of the pipe, while the notice waits on a full stderr,
/// gets exactly the count the note gives (and so no status bytes after the notice, either).
#[test]
fn nothing_is_written_after_the_give_up() {
    let d = scratch("late");
    let pad = 1 << 20;
    let late = piped(&d, "late", Reader::Stall(2500), pad, &[], &[IDLE_1S], true);
    assert_eq!(late.code, Some(7), "{}", late.trace);
    let (cause, _, sent) = undelivered_sent(&late).unwrap_or_else(|| panic!("no status-undelivered note:\n{}", late.trace));
    assert_eq!(cause, "timeout");
    assert_eq!(late.got.len(), sent, "bytes were written after sheepr gave up");
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
