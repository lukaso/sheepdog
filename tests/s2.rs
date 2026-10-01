//! Phase 1, step S2 (PHASE1.md): membership while running, the TERM grace, --leave-strays,
//! the scan's cost; macOS: the `puniq` fact and R growing from members (full cell 24).
//! Readiness only; cleanup only by recorded identity or the iteration's marker. Every signal
//! goes through the doors in `common` (identity-checked pid, or a child not yet reaped).

mod common;

use common::{scan, send, send_child};
use sheepdog::ident::same;
use std::process::{Command, Stdio};
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
    /// The lines the escapee's TERM handler wrote (`TERM <pid>`, `EXIT <pid>`).
    fn term_log(&self) -> Vec<String> {
        let p = format!("{}.term", self.rec());
        std::fs::read_to_string(p).map(|s| s.lines().map(String::from).collect()).unwrap_or_default()
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
    /// Processes whose argv carries the marker and that are not sheepdog itself (sheepdog's
    /// argv carries it too), with identities read at discovery.
    fn marked(&self) -> Result<Vec<(i32, u64)>, String> {
        scan(&self.marker, |w| w.len() >= 2 && !w[1].ends_with("sheepdog"))
    }
    /// Live processes of this job: recorded identities still alive, plus the marked processes.
    fn alive(&self) -> Vec<(i32, u64)> {
        let mut v: Vec<(i32, u64)> = self.recorded().into_iter().filter(|&(p, id)| same(p, id)).collect();
        for p in self.marked().expect("ps failed") {
            if !v.iter().any(|q| q.0 == p.0) {
                v.push(p);
            }
        }
        v
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        for (p, id) in self.recorded() {
            send(p, id, libc::SIGKILL);
        }
        for (p, id) in self.marked().unwrap_or_default() {
            send(p, id, libc::SIGKILL);
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

/// The TERM log of a clean shutdown by the escapee `g`: its TERM, then its EXIT 300 ms later.
fn clean_exit(g: i32) -> Vec<String> {
    vec![format!("TERM {g}"), format!("EXIT {g}")]
}

/// §3.3 step 1: a member that exits on TERM during the grace gets no SIGKILL. The escapee's
/// handler records the TERM, takes 300 ms, then records its EXIT: a SIGKILL during those
/// 300 ms (the grace not honoured) leaves no EXIT line.
#[test]
fn s2_a_member_that_exits_on_term_gets_no_kill() {
    let j = Job::new();
    let st = run(&[], &["term-logger", &j.marker, j.rec()]);
    assert_eq!(st.code(), Some(0));
    let g = j.recorded().first().copied().expect("the escapee was not created").0;
    assert_eq!(j.term_log(), clean_exit(g), "the escapee was not given its grace");
    assert!(j.alive().is_empty(), "the escapee survived");
}

/// §3.3 step 1: a member that is stopped still gets its TERM, because the TERM is followed by
/// a CONT (a stopped process does not act on TERM until it runs). The fixture's root exits
/// only once the escapee is stopped.
#[test]
fn s2_a_stopped_member_still_gets_its_term() {
    let j = Job::new();
    let st = run(&[], &["term-logger", &j.marker, j.rec(), "stop"]);
    assert_eq!(st.code(), Some(0), "the fixture's escapee never stopped");
    let g = j.recorded().first().copied().expect("the escapee was not created").0;
    assert_eq!(j.term_log(), clean_exit(g), "the stopped escapee never acted on its TERM (no CONT after TERM?)");
    assert!(j.alive().is_empty());
}

/// §3.3 step 1: the grace ends as soon as every member is gone, not at its end (2 s by
/// default). The escapee takes 300 ms to exit on TERM; the time is measured from its TERM to
/// sheepdog's exit, so a slow start of the job under load does not count.
#[test]
fn s2_the_grace_ends_when_the_members_are_gone() {
    let j = Job::new();
    let mut c = Command::new(sheepdog())
        .args(["run", "--", fixture(), "term-logger", &j.marker, j.rec()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let t = Instant::now();
    while j.term_log().is_empty() {
        assert!(t.elapsed() < Duration::from_secs(20), "the escapee never got its TERM");
        std::thread::sleep(Duration::from_millis(2));
    }
    let termed = Instant::now();
    let st = c.wait().unwrap();
    let took = termed.elapsed();
    assert_eq!(st.code(), Some(0));
    assert!(took < Duration::from_millis(1000), "sheepdog ended {took:?} after the TERM: the grace ran out instead of ending with its members");
}

/// §3.3 step 1: each member gets exactly one TERM (many programs treat a second TERM as "force
/// quit"). The escapee counts its TERMs and ignores them, so the grace runs to its end.
#[test]
fn s2_each_member_gets_one_term() {
    let j = Job::new();
    let st = run(&["--grace", "500ms"], &["term-counter", &j.marker, j.rec()]);
    assert_eq!(st.code(), Some(0));
    let g = j.recorded().first().copied().expect("the escapee was not created").0;
    assert_eq!(j.term_log(), vec![format!("TERM {g}")], "not exactly one TERM");
    assert!(j.alive().is_empty(), "the escapee survived the kill after the grace");
}

/// §3.3: the kill deadline starts after the grace, so a grace longer than the deadline still
/// ends with a kill (debug seam: a 300 ms deadline; the escapee ignores TERM).
#[test]
fn s2_the_deadline_starts_after_the_grace() {
    let j = Job::new();
    let st = Command::new(sheepdog())
        .args(["run", "--grace", "500ms", "--", fixture(), "term-counter", &j.marker, j.rec()])
        .env("SHEEPDOG_TEST_DEADLINE_MS", "300")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert_eq!(j.recorded().len(), 1, "the escapee was not created");
    assert_eq!(st.code(), Some(0), "the deadline ran out during the grace");
    assert!(j.alive().is_empty(), "the escapee survived");
}

/// `--grace 0` goes straight to SIGKILL: no TERM is sent.
#[test]
fn s2_grace_zero_goes_straight_to_kill() {
    let j = Job::new();
    let st = run(&["--grace", "0"], &["term-logger", &j.marker, j.rec()]);
    assert_eq!(st.code(), Some(0));
    assert_eq!(j.recorded().len(), 1);
    assert!(j.term_log().is_empty(), "--grace 0 still sent a TERM");
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

/// `--leave-strays` applies when the command ends by itself. TERM to sheepdog ends the whole
/// job, strays included (PLAN.md §3.3).
#[test]
fn s2_leave_strays_still_kills_on_term() {
    let j = Job::new();
    let script = format!("\"$0\" escape {m} '{r}'; exec /bin/sleep {m}", m = j.marker, r = j.rec());
    let mut c = Command::new(sheepdog())
        .args(["run", "--leave-strays", "--", "sh", "-c", &script, fixture()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let t = Instant::now();
    while j.recorded().is_empty() {
        assert!(t.elapsed() < Duration::from_secs(10), "the escapee was not created");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(send_child(&mut c, libc::SIGTERM), "sheepdog had already ended");
    let st = c.wait().unwrap();
    use std::os::unix::process::ExitStatusExt;
    assert_eq!(st.signal(), Some(libc::SIGTERM));
    std::thread::sleep(Duration::from_millis(100));
    let alive = j.alive();
    assert!(alive.is_empty(), "TERM with --leave-strays left the job running: {alive:?}");
}

/// Linux `--mode none` (no subreaper): a member that was a descendant for a few scans and was
/// then reparented away is still killed, because it was seen while running.
#[cfg(target_os = "linux")]
#[test]
fn s2_mode_none_kills_a_member_seen_while_running() {
    let j = Job::new();
    let st = run(&["--mode", "none"], &["escape-late", &j.marker, j.rec()]);
    assert_eq!(st.code(), Some(0));
    assert_eq!(j.recorded().len(), 1, "the member was not created");
    std::thread::sleep(Duration::from_millis(100));
    let alive = j.alive();
    assert!(alive.is_empty(), "a member seen while running survived: {alive:?}");
}

/// One tick's worth of the macOS scan's system calls (src/macos.rs `members`), timed in this
/// thread's own CPU time, so other tests running in parallel do not count: the process list, the
/// BSD info of every process, and for each of this user's processes the unique ids and the
/// responsible process. The same calls on the same processes at the same load: the reference the
/// scan's cost is measured against.
#[cfg(target_os = "macos")]
fn one_tick_of_scan_calls() -> Duration {
    use std::os::raw::c_void;
    fn tcpu() -> Duration {
        let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut t) };
        Duration::new(t.tv_sec as u64, t.tv_nsec as u32)
    }
    type RespUniq = unsafe extern "C" fn(libc::pid_t) -> u64;
    let name = std::ffi::CString::new("responsibility_get_uniqueid_responsible_for_pid").unwrap();
    let p = unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr()) };
    assert!(!p.is_null(), "no responsibility SPI");
    let resp: RespUniq = unsafe { std::mem::transmute_copy(&p) };
    let uid = unsafe { libc::getuid() };
    let t0 = tcpu();
    let n = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    let mut pids = vec![0 as libc::pid_t; (n.max(0) as usize) * 2 + 64];
    let got = unsafe { libc::proc_listallpids(pids.as_mut_ptr() as *mut c_void, (pids.len() * 4) as i32) };
    pids.truncate(got.max(0) as usize);
    let mut own = 0u64;
    for &pid in pids.iter().filter(|&&p| p > 0) {
        let mut b: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let bn = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
        let r = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, &mut b as *mut _ as *mut c_void, bn) };
        if r == bn && b.pbi_uid == uid {
            own += 1;
            let mut u = [0u8; 56]; // struct proc_uniqidentifierinfo
            unsafe { libc::proc_pidinfo(pid, 17, 0, u.as_mut_ptr() as *mut c_void, 56) };
            unsafe { resp(pid) };
        }
    }
    let d = tcpu() - t0;
    assert!(own > 0 && pids.len() > 1, "the reference read no processes");
    d
}

/// The scan's cost while a job runs: a 3 s job costs at most this much more CPU than an instant
/// job (the difference is the scan; start-up is in both). Its cost is system calls per process
/// per tick, which grows with the number of processes and with the machine's load, so on macOS it
/// is measured against one tick's worth of the same calls made by this test at the same time
/// (the minimum of several, each SCANS ticks' worth summed, taken between the samples): the 3 s
/// job scans SCANS more times than the instant one, and may cost at most SCAN_K times the
/// reference. On Linux (the containers hold tens of
/// processes) the budget is 1 % of one core or 3 % per 1000 of this user's processes, whichever is
/// larger. Minimum of 3 samples each, so contention from other tests cannot inflate the result.
#[test]
fn s2_the_scan_cost_stays_in_its_budget() {
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
    // the reference: SCANS ticks' worth of the calls, summed (so its noise is the scan's)
    #[cfg(target_os = "macos")]
    const SCANS: u32 = 11;
    #[cfg(target_os = "macos")]
    let reference = || (0..SCANS).map(|_| one_tick_of_scan_calls()).sum::<Duration>();
    #[cfg(target_os = "macos")]
    let mut refs = vec![reference()];
    let mut base = Vec::new();
    let mut job = Vec::new();
    for _ in 0..3 {
        base.push(cpu(&["true"]));
        job.push(cpu(&["/bin/sleep", "3"]));
        #[cfg(target_os = "macos")]
        refs.extend([reference(), reference()]);
    }
    let (base, job) = (*base.iter().min().unwrap(), *job.iter().min().unwrap());
    let scan = job.saturating_sub(base);
    #[cfg(target_os = "macos")]
    {
        // the 3 s job scans SCANS more times than the instant one (15 against 4, counted
        // 2026-10-01: each wait restarts after its scan, so 12 timer ticks take a little over 3 s).
        // Measured with about 900 of the user's processes at load averages of 67-98 (the
        // reference the median of 7): 1.31-1.47; a scan doing each tick's work twice 1.90-2.50,
        // a 125 ms tick 2.42-2.63: both over SCAN_K
        const SCAN_K: f64 = 1.8;
        // the median of 7: one fast sample (less contention for a moment) must not set the bar
        refs.sort();
        let r = refs[refs.len() / 2];
        let ratio = scan.as_secs_f64() / r.as_secs_f64();
        assert!(ratio < SCAN_K, "the scan used {scan:?} of CPU in a 3 s run: {ratio:.2} times {SCANS} ticks' worth of its calls ({r:?}), over {SCAN_K} (start-up {base:?})");
    }
    #[cfg(target_os = "linux")]
    {
        let uid = unsafe { libc::getuid() }.to_string();
        let out = Command::new("ps").args(["-A", "-o", "uid="]).output().expect("ps");
        let own = String::from_utf8_lossy(&out.stdout).split_whitespace().filter(|u| *u == uid).count() as u64;
        assert!(own > 0, "ps listed none of this user's processes");
        let budget = Duration::from_millis(std::cmp::max(30, 90 * own / 1000));
        assert!(scan < budget, "the scan used {scan:?} of CPU in a 3 s run, over its budget of {budget:?} ({own} processes of this user; start-up {base:?})");
    }
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
        assert_eq!(st.code(), Some(0), "the fixture failed: C did not become responsible for itself");
        assert_eq!(j.recorded().len(), 1, "GG was not created");
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
        assert_eq!(j.recorded().len(), 1, "D was not created, or it did not become responsible for itself");
        std::thread::sleep(Duration::from_millis(100));
        let alive = j.alive();
        assert!(alive.is_empty(), "a self-disclaimed child survived: {alive:?}");
    }
}

/// S1 fix review P3-2 (macOS): a failed kqueue TERM registration falls back to polling, so
/// TERM still ends the job well under a tick (debug seam fails EVFILT_SIGNAL only).
#[cfg(target_os = "macos")]
#[test]
// Note (review of the trace sweep): the `polling term` trace proves the fallback was chosen, not
// that it ran. On macOS a TERM interrupts `kevent` (EINTR) and wakes the loop even without its
// registration or the polling fallback, so no input found makes the fallback observable here; this
// cell does not count as coverage of the polling itself (the exit fallback's is
// `s1_the_exit_fallback_polls_within_the_tick`).
fn s2_a_failed_term_registration_still_wakes_on_term() {
    let _ = Command::new(sheepdog()).args(["run", "--", "true"]).status(); // warm-up
    // The tick is widened to 900 ms (debug seam). Without the fallback, TERM waits for the next
    // tick; the tick starts with the wait and TERM follows the root's start by about 30 ms, so
    // the mutant takes 700-870 ms every time (measured). With the fallback the loop polls every
    // 50 ms (measured 34-90 ms at load 97). The 400 ms bound sits between the two; five samples
    // guard against a single slow sample under load, not against the tick's phase.
    for _ in 0..5 {
        let j = Job::new();
        let mut c = Command::new(sheepdog())
            .args(["run", "--", "/bin/sleep", &j.marker])
            .env("SHEEPDOG_TEST_KQ_SIG_EINVAL", "1")
            .env("SHEEPDOG_TEST_TICK_MS", "900")
            .env("SHEEPDOG_TEST_SIGNAL_LOG", j.rec.with_extension("log"))
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
        assert!(send_child(&mut c, libc::SIGTERM), "sheepdog had already ended");
        let out = c.wait_with_output().unwrap();
        let took = k.elapsed();
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(out.status.signal(), Some(libc::SIGTERM));
        let log = std::fs::read_to_string(j.rec.with_extension("log")).unwrap_or_default();
        let _ = std::fs::remove_file(j.rec.with_extension("log"));
        assert!(log.lines().any(|l| l == "polling term"), "the seam did not fail the TERM registration: {log:?}");
        assert!(took < Duration::from_millis(400), "TERM took {took:?} after a failed TERM registration");
    }
}

/// Review S2 A-P2-1 (macOS): a caller runs `bg & exec sheepdog run -- ...`. sheepdog keeps the
/// caller's uniqueid across the exec, so when the caller is responsible for itself (a launchd
/// job, an app helper), everything it started earlier is responsible to sheepdog's uniqueid:
/// a live child, and an orphan whose parent is gone. Neither was started by the job; both must
/// survive. Each shape runs from a plain shell and from a self-responsible one.
#[cfg(target_os = "macos")]
#[test]
fn s2_a_callers_earlier_processes_survive() {
    for disclaimed in [false, true] {
        for (shape, bg) in [("child", "/bin/sleep 30 & echo $! >"), ("orphan", "(/bin/sleep 30 & echo $! >")] {
            // the identity is read while the process certainly lives: the script waits for the go
            // file before it execs sheepdog (read after sheepdog ran, a killed process's pid could
            // be reused)
            let pidf = std::env::temp_dir().join(format!("sd-s2-bg-{}-{disclaimed}-{shape}", std::process::id()));
            let go = std::env::temp_dir().join(format!("sd-s2-go-{}-{disclaimed}-{shape}", std::process::id()));
            let _ = std::fs::remove_file(&pidf);
            let _ = std::fs::remove_file(&go);
            let close = if shape == "orphan" { ")" } else { "" };
            let script = format!("{bg} '{}'{close}; n=0; while [ ! -e '{}' ] && [ $n -lt 1000 ]; do /bin/sleep 0.01; n=$((n+1)); done; [ $n -lt 1000 ] || exit 3; exec \"{}\" run -- true", pidf.display(), go.display(), sheepdog());
            let mut c = if disclaimed {
                let mut c = Command::new(fixture());
                c.args(["dspawn", "wait", "/bin/sh", "-c", &script]);
                c
            } else {
                let mut c = Command::new("/bin/sh");
                c.args(["-c", &script]);
                c
            };
            let mut child = c.stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
            let t = Instant::now();
            while !std::fs::read_to_string(&pidf).is_ok_and(|s| s.ends_with('\n')) {
                assert!(t.elapsed() < Duration::from_secs(10), "disclaimed caller={disclaimed}, {shape}: no pid file");
                std::thread::sleep(Duration::from_millis(5));
            }
            let pid: i32 = std::fs::read_to_string(&pidf).unwrap().trim().parse().unwrap();
            let _ = std::fs::remove_file(&pidf);
            let earlier = common::found(pid);
            assert!(earlier.is_some(), "disclaimed caller={disclaimed}, {shape}: control: the identity could not be read while it lived");
            std::fs::write(&go, "").unwrap();
            let st = child.wait().unwrap();
            let _ = std::fs::remove_file(&go);
            let t = Instant::now();
            let mut alive = true;
            while t.elapsed() < Duration::from_millis(500) {
                alive = earlier.is_some_and(common::alive);
                if !alive {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            if let Some((p, id)) = earlier {
                send(p, id, libc::SIGKILL);
            }
            assert_eq!(st.code(), Some(0), "disclaimed caller={disclaimed}, {shape}: the fixture or sheepdog failed");
            assert!(alive, "disclaimed caller={disclaimed}: the caller's earlier {shape} was killed");
        }
    }
}

/// Review S2 A-P2-2 (macOS): the root starts a child with the disclaim and exits at once,
/// before any scan saw it. The child's one fact is its original parent, the root: sheepdog must
/// know the root's identity from its birth.
#[cfg(target_os = "macos")]
#[test]
fn s2_a_fast_roots_disclaimed_child_is_killed() {
    for i in 0..20 {
        let j = Job::new();
        let st = run(&[], &["dspawn", "/bin/sleep", &j.marker]);
        assert_eq!(st.code(), Some(0), "run {i}: the fixture failed");
        std::thread::sleep(Duration::from_millis(100));
        let alive = j.alive();
        assert!(alive.is_empty(), "run {i}: the fast root's disclaimed child survived: {alive:?}");
    }
}

/// Review S2 round 3, A-P3-2 (macOS): a sheepdog that starts responsible for itself but with no
/// history (nothing refers to its uniqueid: a launchd job whose program is sheepdog) does not
/// relay, so the pid its caller holds is the supervisor (a pid-only INT reaches it).
#[cfg(target_os = "macos")]
#[test]
fn s2_a_fresh_self_responsible_start_does_not_relay() {
    let j = Job::new();
    let mut c = Command::new(fixture())
        .args(["dspawn", "wait", sheepdog(), "run", "--", "/bin/sleep", &j.marker])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let t = Instant::now();
    let root = loop {
        let out = Command::new("ps").args(["-Ao", "pid=,ppid=,args="]).output().unwrap();
        let found = String::from_utf8_lossy(&out.stdout).lines().find_map(|l| {
            let w: Vec<&str> = l.split_whitespace().collect();
            (w.len() == 4 && w[2] == "/bin/sleep" && w[3] == j.marker).then(|| (w[0].parse::<i32>().unwrap(), w[1].parse::<i32>().unwrap()))
        });
        // the root's identity, read at discovery
        if let Some((p, ppid)) = found {
            if let Some((_, id)) = common::found(p) {
                break (p, ppid, id);
            }
        }
        assert!(t.elapsed() < Duration::from_secs(10), "the root never started");
        std::thread::sleep(Duration::from_millis(5));
    };
    // the fixture spawned sheepdog; find that pid (the fixture's only child)
    let out = Command::new("ps").args(["-Ao", "pid=,ppid="]).output().unwrap();
    let spawned: Vec<i32> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let w: Vec<&str> = l.split_whitespace().collect();
            (w.len() == 2 && w[1] == c.id().to_string()).then(|| w[0].parse().ok()).flatten()
        })
        .collect();
    send(root.0, root.2, libc::SIGKILL);
    let _ = c.wait();
    assert_eq!(spawned.len(), 1, "expected the fixture's one child (sheepdog): {spawned:?}");
    assert_eq!(root.1, spawned[0], "a fresh self-responsible sheepdog relayed: the root's parent is not the pid its caller holds");
}

/// Review S2 round 2, B-P2-1 (macOS): the root is suspended until its identity is read. A
/// debug seam delays that read by 300 ms; without the suspension the root would run, start its
/// disclaimed child and exit meanwhile, and the child's only fact would be lost.
#[cfg(target_os = "macos")]
#[test]
fn s2_the_root_is_known_before_it_runs() {
    for i in 0..3 {
        let j = Job::new();
        let st = Command::new(sheepdog())
            .args(["run", "--", fixture(), "dspawn", "/bin/sleep", &j.marker])
            .env("SHEEPDOG_TEST_SLEEP_AFTER_SPAWN_MS", "300")
            .status()
            .unwrap();
        assert_eq!(st.code(), Some(0), "run {i}: the fixture failed");
        std::thread::sleep(Duration::from_millis(100));
        let alive = j.alive();
        assert!(alive.is_empty(), "run {i}: the root ran before sheepdog knew it: {alive:?}");
    }
}

/// Review S2 round 2, A-P3-1: sheepdog never sends the root a signal its caller did not send.
/// With the caller blocking SIGCONT, a root resumed by a SIGCONT would start with it pending.
#[test]
fn s2_the_root_starts_with_no_signal_pending() {
    use std::os::unix::process::CommandExt;
    let run = |via_sheepdog: bool| -> String {
        let mut c = if via_sheepdog {
            let mut c = Command::new(sheepdog());
            c.args(["run", "--", fixture(), "print-pending"]);
            c
        } else {
            let mut c = Command::new(fixture());
            c.arg("print-pending");
            c
        };
        let out = unsafe {
            c.pre_exec(|| {
                let mut set: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut set);
                libc::sigaddset(&mut set, libc::SIGCONT);
                libc::sigprocmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
                Ok(())
            })
            .output()
            .unwrap()
        };
        assert!(out.status.success());
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    let direct = run(false);
    let supervised = run(true);
    assert_eq!(supervised, direct, "the root started with a signal pending that its caller never sent");
    // control: the caller's SIGCONT block reaches the root, or a CONT would not stay pending
    // and the cell above would measure nothing
    let mask = {
        let mut c = Command::new(sheepdog());
        c.args(["run", "--", fixture(), "print-mask"]);
        let out = unsafe {
            c.pre_exec(|| {
                let mut set: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut set);
                libc::sigaddset(&mut set, libc::SIGCONT);
                libc::sigprocmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
                Ok(())
            })
            .output()
            .unwrap()
        };
        String::from_utf8_lossy(&out.stdout).to_string()
    };
    assert!(mask.lines().any(|l| l.trim() == libc::SIGCONT.to_string()), "control: SIGCONT is not blocked in the root: {mask:?}");
}

/// Review S2 round 2, A-P3-2 (macOS; Linux never reads it): a `SHEEPDOG_RELAY_PID` inherited from the environment (a stale
/// value, or a relayed sheepdog's) never makes sheepdog believe its relay died. The inherited
/// value names a relay that is gone (a reaped child's pid), so trusting it would end the job.
#[cfg(target_os = "macos")]
#[test]
fn s2_an_inherited_relay_variable_is_ignored() {
    let mut gone = Command::new("true").spawn().unwrap();
    let gone_pid = gone.id();
    gone.wait().unwrap();
    let out = Command::new(sheepdog())
        .args(["run", "--", "/bin/echo", "ran"])
        .env("SHEEPDOG_RELAY_PID", format!("{gone_pid}:1"))
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "ran", "the command never ran");
    assert_eq!(out.status.code(), Some(0), "sheepdog trusted the inherited relay and ended the job: {:?}", out.status);
}

/// A sheepdog inside a relayed sheepdog's job runs normally (a sanity check of the nesting
/// shape; the relay variable is removed before the root is spawned, so it never reaches the
/// inner sheepdog).
#[test]
fn s2_a_nested_sheepdog_under_a_relay_runs() {
    let job = format!("35.{}001", std::process::id());
    let st = Command::new(fixture())
        .args(["bg-then-exec", &job, sheepdog(), "run", "--", sheepdog(), "run", "--", "true"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    // cleanup by marker and identity: the relay's `/bin/sleep <job>`
    for (p, id) in scan(&job, |w| w.len() == 3 && w[1] == "/bin/sleep").unwrap_or_default() {
        send(p, id, libc::SIGKILL);
    }
    assert_eq!(st.code(), Some(0), "the nested sheepdog failed");
}

/// A stated limit (PLAN.md §3.2), not a guarantee: a member that re-disclaims and dies within
/// one scan tick of starting GG loses GG (GG becomes responsible for itself, its `puniq` is 1,
/// and no scan saw it). Run by hand to measure the window: `cargo test -- --ignored`.
#[cfg(target_os = "macos")]
#[test]
#[ignore = "stated limit: R growth has a one-tick window (PLAN.md §3.2)"]
fn s2_limit_a_redisclaimed_member_that_dies_within_one_tick() {
    let j = Job::new();
    let st = Command::new(sheepdog())
        .args(["run", "--", fixture(), "redisclaim", &j.marker, j.rec()])
        .env("SD_C_LIFE_MS", "30")
        .status()
        .unwrap();
    assert_eq!(st.code(), Some(0));
    std::thread::sleep(Duration::from_millis(100));
    let alive = j.alive();
    assert!(alive.is_empty(), "GG survived (the stated one-tick window): {alive:?}");
}
