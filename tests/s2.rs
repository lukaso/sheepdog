//! Phase 1, step S2 (PHASE1.md): membership while running, the TERM grace, --leave-strays,
//! the scan's cost; macOS: the `puniq` fact and R growing from members (full cell 24).
//! Readiness only; cleanup only by recorded identity or the iteration's marker.

use sheepdog::ident::same;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

static SEQ: AtomicUsize = AtomicUsize::new(0);

fn sheepdog() -> &'static str {
    env!("CARGO_BIN_EXE_sheepdog")
}
fn fixture() -> &'static str {
    env!("CARGO_BIN_EXE_sd-fixture")
}

/// One job's marker and record file; dropping it kills what is left of the job.
struct Job {
    marker: String,
    rec: std::path::PathBuf,
}

impl Job {
    fn new() -> Self {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let marker = format!("21.{}{:06}", std::process::id(), n);
        let rec = std::env::temp_dir().join(format!("sd-s2-{marker}"));
        let _ = std::fs::remove_file(&rec);
        let _ = std::fs::remove_file(rec.with_extension("term"));
        Job { marker, rec }
    }
    fn rec(&self) -> &str {
        self.rec.to_str().unwrap()
    }
    fn term_lines(&self) -> usize {
        let p = format!("{}.term", self.rec());
        std::fs::read_to_string(p).map(|s| s.lines().count()).unwrap_or(0)
    }
    fn recorded(&self) -> Vec<(i32, u64)> {
        std::fs::read_to_string(&self.rec)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| {
                let mut w = l.split_whitespace();
                Some((w.next()?.parse().ok()?, w.next()?.parse().ok()?))
            })
            .collect()
    }
    /// Live processes of this job: recorded identities still alive, plus any process whose
    /// argv carries the marker and is not sheepdog itself (sheepdog's argv carries it too).
    fn alive(&self) -> Vec<i32> {
        let mut v: Vec<i32> = self.recorded().into_iter().filter(|&(p, id)| same(p, id)).map(|(p, _)| p).collect();
        let out = Command::new("ps").args(["-Ao", "pid=,args="]).output().expect("ps");
        assert!(out.status.success(), "ps failed");
        for l in String::from_utf8_lossy(&out.stdout).lines() {
            let w: Vec<&str> = l.split_whitespace().collect();
            if w.len() >= 2 && !w[1].ends_with("sheepdog") && w.iter().any(|x| *x == self.marker) {
                if let Ok(p) = w[0].parse::<i32>() {
                    if !v.contains(&p) {
                        v.push(p);
                    }
                }
            }
        }
        v
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        for (p, id) in self.recorded() {
            if same(p, id) {
                unsafe { libc::kill(p, libc::SIGKILL) };
            }
        }
        if let Ok(out) = Command::new("ps").args(["-Ao", "pid=,args="]).output() {
            for l in String::from_utf8_lossy(&out.stdout).lines() {
                let w: Vec<&str> = l.split_whitespace().collect();
                if w.len() >= 2 && !w[1].ends_with("sheepdog") && w.iter().any(|x| *x == self.marker) {
                    if let Ok(p) = w[0].parse::<i32>() {
                        unsafe { libc::kill(p, libc::SIGKILL) };
                    }
                }
            }
        }
        let _ = std::fs::remove_file(&self.rec);
        let _ = std::fs::remove_file(format!("{}.term", self.rec()));
    }
}

fn run(extra: &[&str], fixture_args: &[&str]) -> std::process::ExitStatus {
    Command::new(sheepdog())
        .arg("run")
        .args(extra)
        .arg("--")
        .arg(fixture())
        .args(fixture_args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap()
}

/// §3.3 step 1: a member that exits on TERM during the grace gets no SIGKILL. The fixture's
/// escapee catches TERM, records it and exits 0; a SIGKILL first would leave no record.
#[test]
fn s2_a_member_that_exits_on_term_gets_no_kill() {
    let j = Job::new();
    let st = run(&[], &["term-logger", &j.marker, j.rec()]);
    assert_eq!(st.code(), Some(0));
    assert_eq!(j.recorded().len(), 1, "the escapee was not created");
    assert_eq!(j.term_lines(), 1, "the escapee got no TERM before it died (killed outright?)");
    assert!(j.alive().is_empty(), "the escapee survived");
}

/// §3.3 step 1: a member that is stopped still gets its TERM, because the TERM is followed by
/// a CONT (a stopped process does not act on TERM until it runs).
#[test]
fn s2_a_stopped_member_still_gets_its_term() {
    let j = Job::new();
    let st = run(&[], &["term-logger", &j.marker, j.rec(), "stop"]);
    assert_eq!(st.code(), Some(0));
    assert_eq!(j.recorded().len(), 1);
    assert_eq!(j.term_lines(), 1, "the stopped escapee never saw its TERM (no CONT after TERM?)");
    assert!(j.alive().is_empty());
}

/// `--grace 0` goes straight to SIGKILL: no TERM is sent.
#[test]
fn s2_grace_zero_goes_straight_to_kill() {
    let j = Job::new();
    let st = run(&["--grace", "0"], &["term-logger", &j.marker, j.rec()]);
    assert_eq!(st.code(), Some(0));
    assert_eq!(j.recorded().len(), 1);
    assert_eq!(j.term_lines(), 0, "--grace 0 still sent a TERM");
    assert!(j.alive().is_empty());
}

/// `--leave-strays`: the kill at the job's end is skipped; the stray keeps running.
#[test]
fn s2_leave_strays_leaves_the_stray() {
    let j = Job::new();
    let st = run(&["--leave-strays"], &["escape", &j.marker, j.rec()]);
    assert_eq!(st.code(), Some(0));
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(j.alive().len(), 1, "--leave-strays still killed the stray");
}

/// The scan while running costs under 1 % CPU: a 3 s job costs at most 30 ms more CPU than
/// an instant job (the difference is the scan; start-up is in both).
#[test]
fn s2_the_scan_costs_under_one_percent_cpu() {
    fn cpu(args: &[&str]) -> Duration {
        let child = Command::new(sheepdog()).arg("run").arg("--").args(args).spawn().unwrap();
        let pid = child.id() as i32;
        let mut status = 0;
        let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
        let r = unsafe { libc::wait4(pid, &mut status, 0, &mut ru) };
        assert_eq!(r, pid);
        std::mem::forget(child);
        let us = |t: libc::timeval| t.tv_sec as u64 * 1_000_000 + t.tv_usec as u64;
        Duration::from_micros(us(ru.ru_utime) + us(ru.ru_stime))
    }
    let _ = cpu(&["true"]); // warm-up (the first launch is scanned by the OS)
    let base = (0..3).map(|_| cpu(&["true"])).min().unwrap();
    let job = cpu(&["/bin/sleep", "3"]);
    let scan = job.saturating_sub(base);
    assert!(scan < Duration::from_millis(30), "the scan used {scan:?} of CPU in a 3 s run (> 1 %; start-up {base:?})");
}

/// Full cell 24 (macOS, PLAN.md §3.2): a member (C) re-disclaims after it was observed and
/// starts a stray (GG) that execs only after its parent has gone (so its `puniq` is 1). GG is
/// responsible to C, so only R growing from members catches it while C lives; C then exits
/// and GG becomes responsible for itself, so only sticky membership keeps it. 0 survivors.
#[cfg(target_os = "macos")]
#[test]
fn s2_cell24_a_member_that_redisclaims_after_it_was_seen_loses_nothing() {
    for _ in 0..3 {
        let j = Job::new();
        let st = run(&[], &["redisclaim", &j.marker, j.rec()]);
        assert_eq!(st.code(), Some(0));
        std::thread::sleep(Duration::from_millis(100));
        let alive = j.alive();
        assert!(alive.is_empty(), "the stray of a re-disclaimed member survived: {alive:?}");
    }
}

/// macOS `puniq` fact: a member's child that re-disclaims itself at once (responsible for
/// itself, never responsible to the supervisor) is caught through its original parent.
#[cfg(target_os = "macos")]
#[test]
fn s2_a_self_disclaimed_child_is_caught_by_its_original_parent() {
    for _ in 0..3 {
        let j = Job::new();
        let st = run(&[], &["puniq-only", &j.marker, j.rec()]);
        assert_eq!(st.code(), Some(0));
        std::thread::sleep(Duration::from_millis(100));
        let alive = j.alive();
        assert!(alive.is_empty(), "a self-disclaimed child survived: {alive:?}");
    }
}

/// S1 fix review P3-2 (macOS): a failed kqueue TERM registration falls back to polling, so
/// TERM still ends the job well under the 250 ms tick (debug seam fails EVFILT_SIGNAL only).
#[cfg(target_os = "macos")]
#[test]
fn s2_a_failed_term_registration_still_wakes_on_term() {
    use std::time::Instant;
    let _ = Command::new(sheepdog()).args(["run", "--", "true"]).status(); // warm-up
    // five samples: without the fallback, TERM waits for the next 250 ms tick, whose phase is
    // random, so a single sample lands under 100 ms about 40 % of the time
    for _ in 0..5 {
        let j = Job::new();
        let mut c = Command::new(sheepdog())
            .args(["run", "--", "/bin/sleep", &j.marker])
            .env("SHEEPDOG_TEST_KQ_SIG_EINVAL", "1")
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let t = Instant::now();
        while j.alive().is_empty() {
            assert!(t.elapsed() < Duration::from_secs(10), "the root never started");
            std::thread::sleep(Duration::from_millis(5));
        }
        std::thread::sleep(Duration::from_millis(30));
        let k = Instant::now();
        unsafe { libc::kill(c.id() as i32, libc::SIGTERM) };
        let st = c.wait().unwrap();
        let took = k.elapsed();
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(st.signal(), Some(libc::SIGTERM));
        assert!(took < Duration::from_millis(100), "TERM took {took:?} after a failed TERM registration");
    }
}
