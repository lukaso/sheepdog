//! sheepdog, phase-0 spike (PLAN.md §7).
//!
//! `sheepdog run [--mode M] -- cmd...` runs cmd, waits for it, then kills every member of its
//! tree (PLAN.md §3.3: freeze, close, kill, repeat until the tree is empty, or the deadline).
//!
//! Modes (the non-default ones exist so tests can prove the default is what catches escapees):
//!   macOS: `responsible` (default: the supervisor re-execs itself with disclaim and members
//!          are processes whose responsible uniqueid is the supervisor's), `root-disclaim`
//!          (the round-2 design: the root disclaims; loses escapees once the root exits)
//!   Linux: `subreaper` (default: PR_SET_CHILD_SUBREAPER; members are the supervisor's
//!          descendants), `none` (no subreaper; orphans go to PID 1)
//!
//! `#![no_main]`: Rust's runtime sets SIGPIPE to ignored before a normal `main` runs, and the
//! job would inherit that. With our own C `main` the runtime does not touch any signal, so the
//! root inherits the caller's dispositions and mask (PLAN.md §3.1, cell 23), with one stated
//! exception: SIGCHLD is set to default, because a supervisor whose children are reaped
//! automatically cannot wait for them (phase-0 fix review, P1-B).
#![cfg_attr(not(test), no_main)]
#![cfg_attr(test, allow(dead_code))]

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

use sheepdog::ident::same;
use std::collections::HashMap;
use std::ffi::{CString, OsString};
use std::io::Write;
use std::os::raw::c_int;
use std::os::unix::ffi::OsStrExt;
use std::time::{Duration, Instant};

/// Write a line to stderr, ignoring errors: a closed stderr must never become a panic (and a
/// panic must never unwind out of the C `main`).
macro_rules! say {
    ($($t:tt)*) => {{
        let _ = writeln!(std::io::stderr(), $($t)*);
    }};
}
pub(crate) use say;

pub struct Args {
    /// argv exactly as received (raw bytes), for the macOS self re-exec
    pub argv: Vec<OsString>,
    pub mode: Option<String>,
    pub cmd: Vec<OsString>,
    /// §3.3 step 1: how long members get after TERM before SIGKILL (default 2 s)
    pub grace: Duration,
    /// skip the kill when the root exits normally (a TERM still ends the job)
    pub leave_strays: bool,
}

/// A duration: "0", "2" (seconds), "2s", "500ms".
/// `500ms`, `2s` or `2` (seconds, fractions allowed), at most one day. Anything else, including
/// `inf` and `NaN`, is None (a usage error; `Duration::from_secs_f64` would panic on them).
fn parse_duration(s: &str) -> Option<Duration> {
    const MAX: Duration = Duration::from_secs(86_400);
    let d = if let Some(ms) = s.strip_suffix("ms") {
        Duration::from_millis(ms.parse().ok()?)
    } else {
        let secs: f64 = s.strip_suffix('s').unwrap_or(s).parse().ok()?;
        if !(0.0..=MAX.as_secs_f64()).contains(&secs) {
            return None;
        }
        Duration::from_secs_f64(secs)
    };
    (d <= MAX).then_some(d)
}

/// Arguments as C strings, byte for byte (non-UTF-8 arguments pass unchanged). An argument
/// with an interior NUL cannot come from a C argv; if one ever arrives it is an error, never a
/// silently different argv (review round 3, F6).
pub fn cstrings(v: &[OsString]) -> Result<Vec<CString>, String> {
    v.iter()
        .map(|s| CString::new(s.as_bytes()).map_err(|_| format!("argument {:?} contains a NUL byte", s)))
        .collect()
}

fn usage() -> i32 {
    say!("usage: sheepdog run [--grace DURATION] [--leave-strays] [--mode M] -- command [args...]");
    125
}

fn parse(argv: Vec<OsString>) -> Result<Args, i32> {
    let args = &argv[1.min(argv.len())..];
    if args.first().map(|a| a.as_bytes()) != Some(b"run") {
        return Err(usage());
    }
    let sep = args.iter().position(|a| a.as_bytes() == b"--").ok_or_else(usage)?;
    let mut mode = None;
    let mut grace = Duration::from_secs(2);
    let mut leave_strays = false;
    let mut i = 1;
    while i < sep {
        match args[i].as_bytes() {
            b"--mode" if i + 1 < sep => {
                mode = Some(args[i + 1].to_string_lossy().into_owned());
                i += 2;
            }
            b"--grace" if i + 1 < sep => {
                grace = parse_duration(&args[i + 1].to_string_lossy()).ok_or_else(usage)?;
                i += 2;
            }
            b"--leave-strays" => {
                leave_strays = true;
                i += 1;
            }
            _ => return Err(usage()),
        }
    }
    let cmd = args[sep + 1..].to_vec();
    if cmd.is_empty() {
        return Err(usage());
    }
    Ok(Args { argv, mode, cmd, grace, leave_strays })
}

/// Exit code for a wait status: the command's code, or 128+signal.
pub fn code_of(status: c_int) -> i32 {
    if libc::WIFEXITED(status) {
        libc::WEXITSTATUS(status)
    } else if libc::WIFSIGNALED(status) {
        128 + libc::WTERMSIG(status)
    } else {
        125
    }
}

/// Send `sig` only if `pid` is still the process with identity `id` (PLAN.md §3.3; the
/// remaining window is the time between this check and the kill call).
pub fn signal(pid: i32, id: u64, sig: c_int) -> Sent {
    // Test seam (debug builds only): SHEEPDOG_TEST_NOKILL=1 makes every signal fail, as EPERM
    // would after a member's setuid exec (cells 20 and 24-lite).
    if seam("SHEEPDOG_TEST_NOKILL") {
        return Sent::No;
    }
    // Test seam (debug builds only): SHEEPDOG_TEST_REUSE_PID=<pid> sends to that pid instead,
    // with the member's identity, as if the member's pid had been reused (S3).
    let pid = seam_ms("SHEEPDOG_TEST_REUSE_PID").map_or(pid, |p| p as i32);
    // Test seam (debug builds only): SHEEPDOG_TEST_WRONG_FREEZE=<pid> lets the STOP to that pid
    // skip the identity check, as a STOP does that lands on a pid reused between the check and
    // the kill (S3): it is delivered unpinned, so the rollback must consider it.
    if sig == libc::SIGSTOP && seam_ms("SHEEPDOG_TEST_WRONG_FREEZE") == Some(pid as u64) {
        trace(format!("kill {pid} {sig}"));
        return if unsafe { libc::kill(pid, sig) } == 0 { Sent::Unpinned } else { Sent::No };
    }
    send_checked(pid, id, sig)
}

/// How a signal was delivered (PLAN.md §3.3 step 2).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sent {
    /// not sent: the process is gone or is no longer the member
    No,
    /// sent through a pidfd opened before the identity check: it reached the member
    Pinned,
    /// sent with `kill` after the identity check (and `kill` succeeded): a pid reused in between
    /// would have got it
    Unpinned,
}

/// Debug seam SHEEPDOG_TEST_SIGNAL_LOG: one line per signal decision, so a test can tell which
/// path a signal took.
fn trace(line: String) {
    if cfg!(debug_assertions) {
        if let Ok(p) = std::env::var("SHEEPDOG_TEST_SIGNAL_LOG") {
            if let Ok(mut f) = std::fs::OpenOptions::new().append(true).create(true).open(p) {
                let _ = writeln!(f, "{line}");
            }
        }
    }
}

/// The rollback's CONT to a process that failed the identity check after our STOP (PLAN.md
/// §3.3 step 4). It cannot be identity-checked: the check is what failed.
fn rollback(pid: i32) {
    if seam("SHEEPDOG_TEST_NOKILL") {
        return;
    }
    trace(format!("rollback {pid}"));
    unsafe { libc::kill(pid, libc::SIGCONT) };
}

/// Send `sig` to `pid` only if it is the process with identity `id` (PLAN.md §3.3 step 2).
/// Linux: through a pidfd opened before the check, so a pid reused after the check can never
/// get the signal. If the pidfd cannot be opened or used for any reason but a gone process
/// (ENOSYS on an old kernel, EPERM under a seccomp filter, EMFILE; debug seams
/// SHEEPDOG_TEST_PIDFD_ENOSYS for the open, SHEEPDOG_TEST_PIDFD_SEND_ENOSYS for the send),
/// the check and `kill` remain; the window between them is the stated residual, and the
/// freeze's rollback covers a STOP that lands in it.
#[cfg(target_os = "linux")]
fn send_checked(pid: i32, id: u64, sig: c_int) -> Sent {
    let fd = if seam("SHEEPDOG_TEST_PIDFD_ENOSYS") {
        None
    } else {
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) } as i32;
        if fd < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            return Sent::No; // gone
        }
        (fd >= 0).then_some(fd)
    };
    if let Some(fd) = fd {
        trace(format!("pidfd {pid} {sig}"));
        if !same(pid, id) {
            unsafe { libc::close(fd) };
            return Sent::No;
        }
        let (r, err) = if seam("SHEEPDOG_TEST_PIDFD_SEND_ENOSYS") {
            (-1, Some(libc::ENOSYS))
        } else {
            let r = unsafe { libc::syscall(libc::SYS_pidfd_send_signal, fd, sig, std::ptr::null::<libc::siginfo_t>(), 0) };
            (r, std::io::Error::last_os_error().raw_os_error())
        };
        unsafe { libc::close(fd) };
        if r == 0 {
            return Sent::Pinned;
        }
        if err == Some(libc::ESRCH) {
            return Sent::No; // exited after the check
        }
    }
    trace(format!("kill {pid} {sig}"));
    if same(pid, id) && unsafe { libc::kill(pid, sig) } == 0 {
        return Sent::Unpinned;
    }
    Sent::No
}

/// Send `sig` to `pid` only if it is the process with identity `id` (PLAN.md §3.3 step 2).
/// macOS: the uniqueid is re-read just before `kill` (the window between them is the stated
/// residual; the freeze's rollback covers a STOP that lands on a reused pid).
#[cfg(target_os = "macos")]
fn send_checked(pid: i32, id: u64, sig: c_int) -> Sent {
    trace(format!("kill {pid} {sig}"));
    if same(pid, id) && unsafe { libc::kill(pid, sig) } == 0 {
        return Sent::Unpinned;
    }
    Sent::No
}

/// Test seam (debug builds only): sleep for the number of ms in env var `name`. If
/// SHEEPDOG_TEST_READY_FILE is set, first create that file, so a test can act INSIDE the window
/// instead of guessing with a sleep (on macOS the first launch of a freshly built binary is
/// delayed by the security scan, and a sleep-based test then raced the scan, not the window).
pub fn seam_sleep(name: &str) {
    if let Some(ms) = std::env::var(name).ok().filter(|_| cfg!(debug_assertions)).and_then(|v| v.parse().ok()) {
        if let Ok(f) = std::env::var("SHEEPDOG_TEST_READY_FILE") {
            let _ = std::fs::File::create(f);
        }
        std::thread::sleep(Duration::from_millis(ms));
    }
}

/// Test seam (debug builds only): the number of milliseconds in env var `name`, if set.
pub fn seam_ms(name: &str) -> Option<u64> {
    std::env::var(name).ok().filter(|_| cfg!(debug_assertions)).and_then(|v| v.parse().ok())
}

/// Test seam (debug builds only): is env var `name` set to "1"?
pub fn seam_flag(name: &str) -> bool {
    seam(name)
}

fn seam(name: &str) -> bool {
    cfg!(debug_assertions) && std::env::var(name).as_deref() == Ok("1")
}

/// Options of the kill loop. Production: `KillOpts::from_env()` (10 s, no seams). The debug
/// seams exist only in debug builds.
pub struct KillOpts {
    pub deadline: Duration,
    /// §3.3 step 1: TERM (then CONT) every member, and give them this long to exit
    pub grace: Duration,
    /// hide every member from all scans after the first one that reported it (cell 24-lite)
    pub forget: bool,
    /// every emptiness check answers "not empty" (the deadline-bound test)
    pub never_empty: bool,
    /// panic right after the first SIGSTOP pass (the panic-safety test)
    pub panic_after_stop: bool,
    /// debug seam: a pid put into the freeze as if its STOP had landed on a reused pid
    pub wrong_freeze: Option<i32>,
}

impl KillOpts {
    pub fn with_grace(mut self, grace: Duration) -> Self {
        self.grace = grace;
        self
    }

    pub fn from_env() -> Self {
        KillOpts {
            grace: Duration::ZERO,
            deadline: std::env::var("SHEEPDOG_TEST_DEADLINE_MS")
                .ok()
                .filter(|_| cfg!(debug_assertions))
                .and_then(|v| v.parse().ok())
                .map(Duration::from_millis)
                .unwrap_or(Duration::from_secs(10)),
            forget: seam("SHEEPDOG_TEST_FORGET"),
            never_empty: seam("SHEEPDOG_TEST_NEVER_EMPTY"),
            panic_after_stop: seam("SHEEPDOG_TEST_PANIC_AFTER_STOP"),
            wrong_freeze: seam_ms("SHEEPDOG_TEST_WRONG_FREEZE").map(|p| p as i32),
        }
    }
}

/// Membership while the job runs (PLAN.md §3.2): the sticky map of live members, every
/// identity ever seen as a member (macOS: the `puniq` fact looks parents up here, and it must
/// still hold after a parent has exited), and R, the responsible identities (macOS: the
/// supervisor plus members that became responsible for themselves).
#[derive(Default)]
pub struct Tracker {
    pub known: HashMap<i32, u64>,
    pub ever: std::collections::HashSet<u64>,
    pub r: std::collections::HashSet<u64>,
}

impl Tracker {
    /// Add freshly found members to the sticky map (replacing an entry only if the old
    /// process is gone) and drop members confirmed dead.
    pub fn refresh(&mut self, found: Vec<(i32, u64)>) {
        for (p, id) in found {
            self.ever.insert(id);
            match self.known.get(&p) {
                Some(&old) if same(p, old) => {}
                _ => {
                    self.known.insert(p, id);
                }
            }
        }
        self.known.retain(|&p, &mut id| same(p, id));
    }
}

/// Why the kill did not end clean.
#[derive(Debug, PartialEq)]
pub enum KillError {
    /// members still alive at the deadline (possibly none that a scan could list)
    Deadline(Vec<i32>),
    /// the kill loop panicked; the known members were sent SIGKILL and a message was printed
    Internal,
}

/// Exit code for a kill that did not end clean (PLAN.md §3.3 step 6: 125).
pub fn kill_failed(e: KillError) -> i32 {
    match e {
        KillError::Deadline(alive) => deadline_missed(&alive),
        KillError::Internal => 125,
    }
}

/// PLAN.md §3.3: the TERM grace (step 1), then the freeze-and-kill passes (steps 2-6).
///
/// - `members` returns the live members right now as (pid, identity).
/// - `reap` runs every pass (Linux reaps adopted orphans).
/// - `tree_empty` is an authoritative emptiness check where the platform has one: Linux with
///   the subreaper set answers Some(no children left) (a scan of /proc is not atomic and can
///   miss a tree that moves faster than the scan: review round 2, P1-A). Otherwise None, and
///   the scan-based rule applies (a stated residual, PLAN.md §7.1).
/// - `send` delivers a signal (production: `signal`, which re-checks identity first).
///
/// Membership is sticky: once seen, a process stays in `known` until it is confirmed dead.
/// Without an authoritative check, it takes two consecutive passes with nothing known alive,
/// both at the end of the loop and at the deadline (review round 3, F2). The deadline is
/// checked at the top of EVERY pass. If the loop panics, every known member is killed, so
/// none is left stopped (review round 3, F7).
pub fn kill_tree(
    opts: &KillOpts,
    members: impl FnMut() -> Vec<(i32, u64)>,
    reap: impl FnMut(),
    tree_empty: impl FnMut() -> Option<bool>,
    send: impl FnMut(i32, u64, c_int) -> Sent,
    initial: HashMap<i32, u64>,
) -> Result<(), KillError> {
    let known: std::cell::RefCell<HashMap<i32, u64>> = std::cell::RefCell::new(initial);
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        kill_loop(opts, members, reap, tree_empty, send, &known)
    }));
    match r {
        Ok(result) => result.map_err(KillError::Deadline),
        Err(_) => {
            let known = known.borrow();
            for (&p, &id) in known.iter() {
                if same(p, id) {
                    unsafe { libc::kill(p, libc::SIGKILL) };
                }
            }
            say!("sheepdog: internal error while killing the tree; sent SIGKILL to the {} member(s) it knew. The tree may NOT be clean.", known.len());
            Err(KillError::Internal)
        }
    }
}

fn kill_loop(
    opts: &KillOpts,
    mut members: impl FnMut() -> Vec<(i32, u64)>,
    mut reap: impl FnMut(),
    mut tree_empty: impl FnMut() -> Option<bool>,
    mut send: impl FnMut(i32, u64, c_int) -> Sent,
    known: &std::cell::RefCell<HashMap<i32, u64>>,
) -> Result<(), Vec<i32>> {
    let deadline = Instant::now() + opts.deadline;
    let mut seen: std::collections::HashSet<(i32, u64)> = std::collections::HashSet::new();
    let forget = opts.forget;
    let mut scan = move || -> Vec<(i32, u64)> {
        let found = members();
        if !forget {
            return found;
        }
        let fresh: Vec<(i32, u64)> = found.into_iter().filter(|m| !seen.contains(m)).collect();
        seen.extend(fresh.iter().copied());
        fresh
    };
    let never_empty = opts.never_empty;
    let mut empty_check = move || if never_empty { Some(false) } else { tree_empty() };
    let refresh = |known: &mut HashMap<i32, u64>, found: Vec<(i32, u64)>| {
        for (p, id) in found {
            match known.get(&p) {
                Some(&old) if same(p, old) => {}
                _ => {
                    known.insert(p, id);
                }
            }
        }
        known.retain(|&p, &mut id| same(p, id));
    };
    // §3.3 step 1, the grace: TERM then CONT every member (a stopped member acts on TERM only
    // once it runs), newly found ones too, until all are gone or the grace is over
    if !opts.grace.is_zero() {
        let grace_end = Instant::now() + opts.grace;
        let mut termed: std::collections::HashSet<(i32, u64)> = std::collections::HashSet::new();
        loop {
            reap();
            refresh(&mut known.borrow_mut(), scan());
            let now: Vec<(i32, u64)> = known.borrow().iter().map(|(&p, &id)| (p, id)).collect();
            for &(p, id) in &now {
                if termed.insert((p, id)) {
                    let _ = send(p, id, libc::SIGTERM);
                    let _ = send(p, id, libc::SIGCONT);
                }
            }
            if now.is_empty() && empty_check() != Some(false) {
                break;
            }
            if Instant::now() > grace_end {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    let deadline = deadline + opts.grace;
    let mut empty = 0;
    let mut wrong_freeze = opts.wrong_freeze;
    loop {
        reap();
        refresh(&mut known.borrow_mut(), scan());
        if Instant::now() > deadline {
            // let the last SIGKILLs take effect, then require the same evidence as the normal
            // end: authoritative empty, or two consecutive passes with nothing known alive
            std::thread::sleep(Duration::from_millis(100));
            let mut empties = 0;
            for _ in 0..2 {
                reap();
                refresh(&mut known.borrow_mut(), scan());
                match empty_check() {
                    Some(true) => return Ok(()),
                    Some(false) => {}
                    None if known.borrow().is_empty() => empties += 1,
                    None => {}
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            if empties == 2 {
                return Ok(());
            }
            return Err(known.borrow().keys().copied().collect());
        }
        let authoritative = empty_check();
        if authoritative == Some(true) {
            return Ok(());
        }
        if known.borrow().is_empty() {
            // an authoritative "not empty" never counts as empty
            empty = if authoritative.is_none() { empty + 1 } else { 0 };
            if empty >= 2 {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(1));
            continue;
        }
        empty = 0;
        // freeze what we know, then close over members created meanwhile, then kill all. Each
        // STOP delivered unpinned (a pid reused after its check would have got it) records
        // whether its process was already stopped (§3.3 step 2), for the rollback; a pinned STOP
        // reached the member and has nothing to roll back.
        let mut before: Vec<(i32, u64)> = known.borrow().iter().map(|(&p, &id)| (p, id)).collect();
        if let Some(d) = wrong_freeze.take() {
            before.push((d, 0)); // debug seam: its STOP lands as on a reused pid (see signal())
        }
        // With each recorded STOP goes the moment it was sent: a process at that pid that started
        // later cannot have got it (the member died and its pid was reused), so it is never
        // resumed. Debug seam SHEEPDOG_TEST_FREEZE_STAMP_ZERO records the wrong-freeze seam's STOP
        // as sent before anything started.
        let stamp_zero = seam("SHEEPDOG_TEST_FREEZE_STAMP_ZERO");
        let mut frozen: Vec<(i32, u64, bool, u64)> = Vec::new();
        for &(p, id) in &before {
            let was_stopped = sheepdog::ident::stopped(p);
            if send(p, id, libc::SIGSTOP) == Sent::Unpinned {
                let at = if stamp_zero && id == 0 { 0 } else { sheepdog::ident::now_stamp() };
                frozen.push((p, id, was_stopped, at));
            }
        }
        if opts.panic_after_stop {
            panic!("test seam: panic after the freeze");
        }
        refresh(&mut known.borrow_mut(), scan());
        let all: Vec<(i32, u64)> = known.borrow().iter().map(|(&p, &id)| (p, id)).collect();
        for &(p, id) in &all {
            if !before.iter().any(|&(b, _)| b == p) {
                let was_stopped = sheepdog::ident::stopped(p);
                if send(p, id, libc::SIGSTOP) == Sent::Unpinned {
                    frozen.push((p, id, was_stopped, sheepdog::ident::now_stamp()));
                }
            }
        }
        // §3.3 step 4, verify: a frozen process that is no longer the member (its STOP may have
        // landed on a reused pid) gets SIGCONT, but only if it was not stopped before our STOP,
        // it already existed when our STOP was sent, and it is stopped now (a stranger stopped
        // by someone else stays stopped).
        for &(p, id, was_stopped, at) in &frozen {
            let existed = sheepdog::ident::start_stamp(p).is_some_and(|s| s <= at);
            if !was_stopped && !same(p, id) && existed && sheepdog::ident::stopped(p) {
                rollback(p);
            }
        }
        for &(p, id) in &all {
            let _ = send(p, id, libc::SIGKILL);
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Signal set-up of a run (PLAN.md §3.1). sheepdog never catches TERM or SIGCHLD: it BLOCKS
/// them and waits for them synchronously (Linux: sigtimedwait; macOS: kqueue plus a sigpending
/// check). A blocked signal stays pending until the wait takes it, across the macOS self
/// re-exec and across any gap between a check and a blocking call, so no TERM can be lost
/// (review round 6: a caught TERM was lost across the re-exec, and between the flag check and
/// the blocking wait).
pub struct Signals {
    /// the mask sheepdog was started with: the root gets exactly this
    pub caller_mask: libc::sigset_t,
    /// false when the caller ignored TERM: then TERM stays ignored, for sheepdog and the root
    pub watch_term: bool,
}

fn setup_signals() -> Signals {
    unsafe {
        // sheepdog must see its children's exits (review round 2, P1-B); the root inherits it
        libc::signal(libc::SIGCHLD, libc::SIG_DFL);
        let mut term: libc::sigaction = std::mem::zeroed();
        libc::sigaction(libc::SIGTERM, std::ptr::null(), &mut term);
        let watch_term = term.sa_sigaction == libc::SIG_DFL;
        let mut block: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut block);
        libc::sigaddset(&mut block, libc::SIGCHLD);
        if watch_term {
            libc::sigaddset(&mut block, libc::SIGTERM);
        }
        let mut caller_mask: libc::sigset_t = std::mem::zeroed();
        libc::sigprocmask(libc::SIG_BLOCK, &block, &mut caller_mask);
        Signals { caller_mask, watch_term }
    }
}

/// Consume `sig` if it is pending (PHASE1.md §1.1): discard it with the SIG_IGN/SIG_DFL
/// toggle (POSIX discards a pending signal whose action becomes SIG_IGN, even while it is
/// blocked) and return true. Never `sigwait`: on macOS a signal that `sigpending` showed can
/// vanish before a `sigwait` (a CONT removes a pending TSTP, and back), and `sigwait` then
/// blocks forever (phase-1 plan review, rounds 2 and 3, probed). Never for SIGCHLD: SIG_IGN
/// on SIGCHLD turns on automatic reaping.
pub fn consume(sig: c_int) -> bool {
    assert_ne!(sig, libc::SIGCHLD, "SIGCHLD must never be toggled");
    unsafe {
        let mut p: libc::sigset_t = std::mem::zeroed();
        libc::sigpending(&mut p);
        if libc::sigismember(&p, sig) != 1 {
            return false;
        }
        libc::signal(sig, libc::SIG_IGN);
        libc::signal(sig, libc::SIG_DFL);
        true
    }
}

/// Does this process already have children (for example a shell's background job before it
/// `exec`ed sheepdog)? waitid with WNOWAIT answers atomically without /proc and without reaping
/// anything (review round 4, P3-3).
pub fn has_children() -> bool {
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    let r = unsafe { libc::waitid(libc::P_ALL, 0, &mut info, libc::WEXITED | libc::WNOHANG | libc::WNOWAIT) };
    r == 0
}

/// Die the way the supervisor died: death by signal N stays death by signal N for our caller,
/// which shells rely on (for example to stop a loop on ctrl-C: review round 4, P2-1).
pub fn die_like(status: libc::c_int) -> i32 {
    if libc::WIFSIGNALED(status) {
        let sig = libc::WTERMSIG(status);
        unsafe {
            let no_core = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
            libc::setrlimit(libc::RLIMIT_CORE, &no_core);
            libc::signal(sig, libc::SIG_DFL);
            let mut one: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut one);
            libc::sigaddset(&mut one, sig);
            libc::sigprocmask(libc::SIG_UNBLOCK, &one, std::ptr::null_mut());
            libc::raise(sig);
        }
    }
    code_of(status)
}

/// Before the root is spawned: a TERM that is already pending (the caller blocked TERM and it
/// arrived) ends sheepdog now, so the root never runs (round-7 P3-F2). Returns the exit code
/// if sheepdog must end.
pub fn term_before_spawn(sig: &Signals) -> Option<i32> {
    seam_sleep("SHEEPDOG_TEST_SLEEP_BEFORE_SPAWN_MS");
    (sig.watch_term && term_pending()).then(|| die_by_term(143))
}

/// Is a TERM pending (blocked, not yet taken by a wait)?
pub fn term_pending() -> bool {
    unsafe {
        let mut p: libc::sigset_t = std::mem::zeroed();
        libc::sigpending(&mut p);
        libc::sigismember(&p, libc::SIGTERM) == 1
    }
}

/// After the tree was killed for a TERM: die of SIGTERM, so the caller sees death by signal.
pub fn die_by_term(fallback: i32) -> i32 {
    unsafe {
        libc::signal(libc::SIGTERM, libc::SIG_DFL);
        let mut one: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut one);
        libc::sigaddset(&mut one, libc::SIGTERM);
        libc::sigprocmask(libc::SIG_UNBLOCK, &one, std::ptr::null_mut());
        libc::raise(libc::SIGTERM);
    }
    fallback
}

/// Report a missed deadline (PLAN.md §3.3 step 6) and return exit code 125.
pub fn deadline_missed(alive: &[i32]) -> i32 {
    if alive.is_empty() {
        say!("sheepdog: members are still alive at the kill deadline, but none could be listed. The tree is NOT clean.");
    } else {
        say!(
            "sheepdog: {} process(es) still alive at the kill deadline: pids {:?}. The tree is NOT clean.",
            alive.len(),
            alive
        );
    }
    125
}

fn run(argv: Vec<OsString>) -> i32 {
    let args = match parse(argv) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let sig = setup_signals();
    #[cfg(target_os = "macos")]
    let code = macos::run(&args, &sig);
    #[cfg(target_os = "linux")]
    let code = linux::run(&args, &sig);
    code
}

#[cfg(not(test))]
#[no_mangle]
pub extern "C" fn main(argc: c_int, argv: *const *const std::os::raw::c_char) -> c_int {
    use std::os::unix::ffi::OsStringExt;
    let argv: Vec<OsString> = (0..argc.max(0) as usize)
        .map(|i| OsString::from_vec(unsafe { std::ffi::CStr::from_ptr(*argv.add(i)) }.to_bytes().to_vec()))
        .collect();
    // a panic must not unwind out of an extern "C" fn (undefined behaviour before Rust 1.81)
    std::panic::catch_unwind(|| run(argv)).unwrap_or(125)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review round 3, F2: with no authoritative check, the deadline must not call the tree
    /// clean after ONE empty scan. The scan misses a live member in the pass that reaches the
    /// deadline and in the first deadline scan, then sees it in the second: the result must be
    /// Err. (Calling it clean after one empty scan returns Ok here.)
    #[test]
    fn the_deadline_needs_two_empty_scans_without_an_authoritative_check() {
        let mut child = std::process::Command::new("/bin/sleep").arg("5").spawn().unwrap();
        let pid = child.id() as i32;
        let id = sheepdog::ident::identity(pid).unwrap();
        let mut calls = 0;
        let opts = KillOpts { deadline: Duration::ZERO, grace: Duration::ZERO, forget: false, never_empty: false, panic_after_stop: false, wrong_freeze: None };
        let r = kill_tree(
            &opts,
            || {
                calls += 1;
                if calls <= 2 { vec![] } else { vec![(pid, id)] } // missed twice, then seen
            },
            || {},
            || None,
            |_, _, _| Sent::No, // never signal: the member stays alive
            HashMap::new(),
        );
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(r, Err(KillError::Deadline(vec![pid])));
    }
}
