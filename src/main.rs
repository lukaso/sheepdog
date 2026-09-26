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
    /// no D9 hint (PLAN.md §3.1)
    pub quiet: bool,
    /// forward an INT to the root as well (callers that signal only the sheepdog pid)
    pub forward_int_to_root: bool,
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
    say!("usage: sheepdog run [--grace DURATION] [--leave-strays] [--quiet] [--forward-int-to-root] [--mode M] -- command [args...]");
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
    let mut quiet = false;
    let mut forward_int_to_root = false;
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
            b"--quiet" => {
                quiet = true;
                i += 1;
            }
            b"--forward-int-to-root" => {
                forward_int_to_root = true;
                i += 1;
            }
            _ => return Err(usage()),
        }
    }
    let cmd = args[sep + 1..].to_vec();
    if cmd.is_empty() {
        return Err(usage());
    }
    Ok(Args { argv, mode, cmd, grace, leave_strays, quiet, forward_int_to_root })
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
    // never a process group or the broadcast: 0 is our own group, -1 every process we may
    // signal (members are real pids; this makes anything else impossible, not just unlikely)
    if pid <= 1 {
        return Sent::No;
    }
    // Test seam (debug builds only): SHEEPDOG_TEST_NOKILL=1 makes every signal fail, as EPERM
    // would after a member's setuid exec (cells 20 and 24-lite).
    if seam("SHEEPDOG_TEST_NOKILL") {
        return Sent::No;
    }
    // Test seam (debug builds only): SHEEPDOG_TEST_REUSE_PID=<pid> sends to that pid instead,
    // with the member's identity, as if the member's pid had been reused (S3).
    let pid = seam_ms("SHEEPDOG_TEST_REUSE_PID").map_or(pid, |p| p as i32);
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

/// An unpinned STOP to member (`p`, `id`) was delivered: read who is at `p` now. If it is still
/// the member, the STOP reached it and there is nothing to roll back, ever. If it is another
/// process, the STOP landed on that process (a pid reused between the check and the kill):
/// record its identity for the rollback (PLAN.md §3.3 step 2). Debug seam
/// SHEEPDOG_TEST_FREEZE_PID_REUSED records the wrong-freeze seam's STOP as landing on another
/// process than the one now at the pid.
fn landed(frozen: &mut Vec<(i32, u64)>, p: i32, id: u64) {
    let mut on = sheepdog::ident::identity(p);
    if id == 0 && seam("SHEEPDOG_TEST_FREEZE_PID_REUSED") {
        on = on.map(|u| u.wrapping_add(1));
    }
    if let Some(on) = on.filter(|&on| on != id) {
        trace(format!("record {p}"));
        frozen.push((p, on));
    }
}

/// The rollback's CONT to the process a STOP of ours landed on by mistake (PLAN.md §3.3 step
/// 4), guarded by that process's own identity, the same way as any signal (on Linux through a
/// pidfd, so a pid that changes hands again cannot get it).
fn rollback(pid: i32, landed_on: u64) {
    if seam("SHEEPDOG_TEST_NOKILL") {
        return;
    }
    if same(pid, landed_on) {
        trace(format!("rollback {pid}"));
        let _ = send_checked(pid, landed_on, libc::SIGCONT);
    }
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
    if pid <= 1 {
        return Sent::No; // every door refuses a group or the broadcast (the rollback enters here)
    }
    let race = wrong_freeze_race(pid, sig);
    let fd = if race || seam("SHEEPDOG_TEST_PIDFD_ENOSYS") {
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
    if (race || same(pid, id)) && unsafe { libc::kill(pid, sig) } == 0 {
        return Sent::Unpinned;
    }
    Sent::No
}

/// Test seam (debug builds only): SHEEPDOG_TEST_WRONG_FREEZE=<pid> makes the identity check of
/// the STOP to that pid pass on the kill path, as a pid reused between the check and the kill
/// would (S3): the STOP lands on a process that is not the member.
fn wrong_freeze_race(pid: i32, sig: c_int) -> bool {
    sig == libc::SIGSTOP && seam_ms("SHEEPDOG_TEST_WRONG_FREEZE") == Some(pid as u64)
}

/// Send `sig` to `pid` only if it is the process with identity `id` (PLAN.md §3.3 step 2).
/// macOS: the uniqueid is re-read just before `kill` (the window between them is the stated
/// residual; the freeze's rollback covers a STOP that lands on a reused pid).
#[cfg(target_os = "macos")]
fn send_checked(pid: i32, id: u64, sig: c_int) -> Sent {
    if pid <= 1 {
        return Sent::No; // every door refuses a group or the broadcast (the rollback enters here)
    }
    trace(format!("kill {pid} {sig}"));
    if (wrong_freeze_race(pid, sig) || same(pid, id)) && unsafe { libc::kill(pid, sig) } == 0 {
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
                if p > 1 && same(p, id) {
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
        // freeze what we know, then close over members created meanwhile, then kill all. After a
        // STOP delivered unpinned (a pid reused after its check would have got it), `landed`
        // records the process it landed on if that is not the member (§3.3 step 2), for the
        // rollback; a pinned STOP reached the member and has nothing to roll back.
        let mut before: Vec<(i32, u64)> = known.borrow().iter().map(|(&p, &id)| (p, id)).collect();
        if let Some(d) = wrong_freeze.take() {
            before.push((d, 0)); // debug seam: its STOP lands as on a reused pid (see signal())
        }
        let mut frozen: Vec<(i32, u64)> = Vec::new();
        for &(p, id) in &before {
            if send(p, id, libc::SIGSTOP) == Sent::Unpinned {
                landed(&mut frozen, p, id);
            }
        }
        if opts.panic_after_stop {
            panic!("test seam: panic after the freeze");
        }
        refresh(&mut known.borrow_mut(), scan());
        let all: Vec<(i32, u64)> = known.borrow().iter().map(|(&p, &id)| (p, id)).collect();
        for &(p, id) in &all {
            // by pid AND identity: a new member at a pid the first pass stopped for another process
            // gets its own STOP
            if !before.iter().any(|&(b, bid)| b == p && bid == id) {
                if send(p, id, libc::SIGSTOP) == Sent::Unpinned {
                    landed(&mut frozen, p, id);
                }
            }
        }
        // §3.3 step 4, verify: the process our STOP landed on, when it was not the member, gets
        // SIGCONT if it is still that very process (a later process at the pid is left as it
        // is). Whether it was stopped before our STOP cannot be known (its pid changed hands
        // after our check), so it is always resumed: leaving it stopped for good is the worse
        // error. It need not show as stopped yet: SIGCONT also discards a STOP still pending.
        // A recorded process that is a member now (another member's pid came to it) stays
        // frozen until its KILL: a resumed member could fork.
        for &(p, landed_on) in &frozen {
            if !all.iter().any(|&(q, qid)| q == p && qid == landed_on) {
                rollback(p, landed_on);
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
    /// INT and HUP, each watched only if the caller left it at its default (S4)
    pub watch_int: bool,
    pub watch_hup: bool,
}

impl Signals {
    /// The signal set the event loop waits on: CHLD plus every watched signal.
    pub fn wait_set(&self) -> libc::sigset_t {
        unsafe {
            let mut set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            libc::sigaddset(&mut set, libc::SIGCHLD);
            for (on, s) in [(self.watch_term, libc::SIGTERM), (self.watch_int, libc::SIGINT), (self.watch_hup, libc::SIGHUP)] {
                if on {
                    libc::sigaddset(&mut set, s);
                }
            }
            set
        }
    }
}

fn setup_signals() -> Signals {
    unsafe {
        // sheepdog must see its children's exits (review round 2, P1-B); the root inherits it
        libc::signal(libc::SIGCHLD, libc::SIG_DFL);
        let at_default = |s: c_int| {
            let mut a: libc::sigaction = std::mem::zeroed();
            libc::sigaction(s, std::ptr::null(), &mut a);
            a.sa_sigaction == libc::SIG_DFL
        };
        let sig = Signals {
            caller_mask: std::mem::zeroed(),
            watch_term: at_default(libc::SIGTERM),
            watch_int: at_default(libc::SIGINT),
            watch_hup: at_default(libc::SIGHUP),
        };
        let block = sig.wait_set();
        let mut caller_mask: libc::sigset_t = std::mem::zeroed();
        libc::sigprocmask(libc::SIG_BLOCK, &block, &mut caller_mask);
        Signals { caller_mask, ..sig }
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

/// The scan tick of the event loop: 250 ms. Debug seam SHEEPDOG_TEST_TICK_MS (under 1 s) widens
/// it, so a cell can tell "acted at the event" from "acted at the next tick" by a wide margin.
pub fn tick_ms() -> u64 {
    seam_ms("SHEEPDOG_TEST_TICK_MS").filter(|&ms| ms < 1000).unwrap_or(250)
}

/// A wait status for a plain exit with `code` (the encoding both OSes use).
pub fn exit_status(code: i32) -> c_int {
    (code & 0xff) << 8
}

/// INT and HUP while the job runs (PLAN.md §3.1, PHASE1.md §1.3 and S4). They never end the
/// job. Each one is forwarded only to members outside sheepdog's own process group: a signal
/// from the terminal, or one sent to the group, already reached every member inside it, so
/// this gives exactly one delivery in every case. Two exceptions: HUP goes to every member when
/// sheepdog (or its relay) is the session leader, since the kernel then sends the terminal's
/// HUP to the leader only; and `--forward-int-to-root` also sends INT to the root.
pub struct Interrupts {
    got_int: bool,
    got_hup: bool,
    leader: bool,
    /// sheepdog's process group is its own: sheepdog or its relay leads it (a shell job, a
    /// terminal). Otherwise it is the caller's group (a harness that did not make a new one).
    own_group: bool,
    int_to_root: bool,
    quiet: bool,
    /// the D9 hint: when it is due, and for which signal
    hint: Option<(Instant, c_int)>,
    hinted: bool,
}

impl Interrupts {
    /// `relay` is the relay's pid when this supervisor has one (it keeps the pid from the fork;
    /// the parent pid changes once the relay dies). They share a session, so the test is exact.
    pub fn new(a: &Args, relay: Option<i32>) -> Self {
        let sid = unsafe { libc::getsid(0) };
        let leader = sid == unsafe { libc::getpid() } || relay.is_some_and(|r| sid == r);
        let pg = unsafe { libc::getpgrp() };
        let own_group = pg == unsafe { libc::getpid() } || relay == Some(pg);
        Interrupts { got_int: false, got_hup: false, leader, own_group, int_to_root: a.forward_int_to_root, quiet: a.quiet, hint: None, hinted: false }
    }

    /// No forwarding and no hint (the root-disclaim mode).
    pub fn none() -> Self {
        Interrupts { got_int: false, got_hup: false, leader: false, own_group: false, int_to_root: false, quiet: true, hint: None, hinted: true }
    }

    /// Record a consumed INT or HUP (it decides death by signal at the end).
    pub fn note(&mut self, sig: c_int) {
        self.got_int |= sig == libc::SIGINT;
        self.got_hup |= sig == libc::SIGHUP;
    }

    /// One INT or HUP consumed while the job runs: rescan, forward, and repeat the scan until
    /// it finds no new target (fresh membership at the event, PHASE1.md §1.3; never the last
    /// timed scan alone). `members` rescans and returns the live members.
    pub fn forward(&mut self, sig: c_int, root: i32, members: &mut dyn FnMut() -> Vec<(i32, u64)>) {
        self.note(sig);
        let own = unsafe { libc::getpgrp() };
        let all = sig == libc::SIGHUP && self.leader;
        let with_root = sig == libc::SIGINT && self.int_to_root;
        let mut sent: std::collections::HashSet<(i32, u64)> = std::collections::HashSet::new();
        let mut root_got = false;
        for _ in 0..32 {
            let fresh: Vec<(i32, u64)> = members()
                .into_iter()
                .filter(|m| !sent.contains(m))
                .filter(|&(p, _)| {
                    let g = unsafe { libc::getpgid(p) };
                    all || (with_root && p == root) || (g >= 0 && g != own)
                })
                .collect();
            if fresh.is_empty() {
                break;
            }
            for (p, id) in fresh {
                if signal(p, id, sig) != Sent::No && p == root {
                    root_got = true;
                }
                sent.insert((p, id));
            }
        }
        // In its own group and in the terminal's foreground, the INT most likely came from the
        // terminal and reached the root too: no hint. In the caller's group (a harness in a
        // terminal) the foreground says nothing about who sent it (S4 review round 2).
        if !root_got && !self.hinted && self.hint.is_none() && !(self.own_group && in_foreground()) {
            let ms = seam_ms("SHEEPDOG_TEST_HINT_MS").unwrap_or(3000);
            self.hint = Some((Instant::now() + Duration::from_millis(ms), sig));
        }
    }

    /// The D9 hint: once per job, when it is due. Only called while the job runs: the root was
    /// running at this pass's check (it can exit in the instant after; stated, harmless).
    /// `group_is_ours(pg)` answers, at that moment, whether every process in group `pg` is
    /// sheepdog, its relay or a member: only then is the group named.
    pub fn tick(&mut self, group_is_ours: &mut dyn FnMut(i32) -> bool) {
        if let Some((due, sig)) = self.hint {
            if Instant::now() >= due {
                self.hint = None;
                self.hinted = true;
                if !self.quiet {
                    let pg = unsafe { libc::getpgrp() };
                    let named = (self.own_group && pg > 1 && group_is_ours(pg)).then_some(pg);
                    say!("{}", hint_text(sig, named));
                }
            }
        }
    }

    /// Consume an INT or HUP that is still pending at the end (one that came during the kill):
    /// it counts too ("consumed at any point before it exits").
    pub fn drain_pending(&mut self, sig: &Signals) {
        if sig.watch_int && consume(libc::SIGINT) {
            self.got_int = true;
        }
        if sig.watch_hup && consume(libc::SIGHUP) {
            self.got_hup = true;
        }
    }
}

/// Is every process in `group` (the pids of one process group) sheepdog, its relay, or a known
/// member? A caller's process in the group (a background job started before sheepdog with no
/// job control, an orphan the caller left behind) means `kill -INT -<pgid>` would reach it, so
/// the hint must not name the group (S4 review round 3). An empty list (the scan failed) is no.
pub fn only_ours(group: &[i32], relay: Option<i32>, known: &HashMap<i32, u64>) -> bool {
    let me = unsafe { libc::getpid() };
    !group.is_empty()
        && group.iter().all(|&p| p == me || Some(p) == relay || known.get(&p).is_some_and(|&id| same(p, id)))
}

/// The D9 hint for `sig`, naming the process group `pg`.
/// `own` is sheepdog's own process group, or None when the group is the caller's (signalling it
/// would reach the caller too). A group of 1 or 0 is never named either: `kill -INT -1` is the
/// broadcast (every process the reader may signal) and `-0` the reader's own group; that
/// happens as PID 1 in a container.
fn hint_text(sig: c_int, own: Option<i32>) -> String {
    let (name, flag) = if sig == libc::SIGINT { ("SIGINT", "INT") } else { ("SIGHUP", "HUP") };
    let head = format!("sheepdog: got {name}; the command is still running. A signal sent only to sheepdog's pid does not reach it: send TERM to end the job");
    if let Some(pg) = own.filter(|&pg| pg > 1) {
        format!("{head}, or signal the process group (kill -{flag} -{pg}).")
    } else {
        format!("{head}.")
    }
}

/// Is sheepdog's process group the foreground group of its controlling terminal? Then an INT
/// or HUP came from that terminal (or from someone signalling the group), and it reached the
/// root too, so the pid-only hint would be false (a REPL or an editor that handles ctrl-C).
fn in_foreground() -> bool {
    unsafe {
        let fd = libc::open(b"/dev/tty\0".as_ptr() as *const libc::c_char, libc::O_RDONLY | libc::O_NOCTTY | libc::O_CLOEXEC | libc::O_NONBLOCK);
        if fd < 0 {
            return false;
        }
        let fg = libc::tcgetpgrp(fd);
        libc::close(fd);
        fg >= 0 && fg == libc::getpgrp()
    }
}

/// How the supervisor ends once the job is over (PHASE1.md §1.3): a TERM means death by TERM;
/// a kill that did not end clean means 125; a root that died of INT or HUP, when sheepdog also
/// got that signal, means death by that signal (so a shell loop stops on ctrl-C); otherwise the
/// root's exit code. `status` is the root's wait status, None when TERM ended the job.
pub fn finish(status: Option<c_int>, result: Result<(), KillError>, ints: &mut Interrupts, sig: &Signals) -> i32 {
    ints.drain_pending(sig);
    match (status, result) {
        (_, Err(e)) => kill_failed(e),
        (None, Ok(())) => die_by_term(143),
        (Some(st), Ok(())) => {
            let same_signal = libc::WIFSIGNALED(st)
                && ((libc::WTERMSIG(st) == libc::SIGINT && ints.got_int) || (libc::WTERMSIG(st) == libc::SIGHUP && ints.got_hup));
            if same_signal {
                die_like(st)
            } else {
                code_of(st)
            }
        }
    }
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

    /// S4 review (A-P1-1, round 2 A-P2-2): the hint names only sheepdog's own group, and
    /// never a group of 1 or 0. As PID 1 in a container the group is 1, and `kill -INT -1`
    /// signals every process the reader may signal; 0 is the reader's own group; a caller's
    /// group (None) would reach the caller. A real own group (control) is named.
    #[test]
    fn the_hint_names_only_a_safe_own_group() {
        let targets = |own: Option<i32>| -> Vec<i64> {
            hint_text(libc::SIGINT, own)
                .split(|c: char| c.is_whitespace() || "(),.".contains(c))
                .filter_map(|w| w.strip_prefix('-')?.parse::<i64>().ok())
                .collect()
        };
        assert_eq!(targets(Some(4242)), vec![4242], "control: the hint names a real own group");
        for own in [Some(0), Some(1), None] {
            assert!(targets(own).is_empty(), "{own:?}: the hint named a target: {:?}", targets(own));
        }
    }

    /// No signal ever goes to pid 1 or lower: 0 is sheepdog's own process group and -1 is
    /// every process the user owns (the 2026-09-26 host incident was a kill(-1) elsewhere).
    /// Signal 0 and the real identities, so only the guard can say No.
    #[test]
    fn no_signal_reaches_pid_one_or_below() {
        for p in [-1, 0, 1] {
            let id = sheepdog::ident::identity(p).unwrap_or(0);
            assert_eq!(signal(p, id, 0), Sent::No, "pid {p} (identity {id}) was signalled");
        }
    }

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

    fn proc_state(pid: i32) -> char {
        std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|s| s.rfind(')').and_then(|i| s[i + 1..].split_whitespace().next().and_then(|f| f.chars().next())))
            .unwrap_or('?')
    }

    /// PLAN.md §3.3 steps 2 and 4, the real race (Linux, needs --privileged for ns_last_pid; run
    /// with SD_REUSE_TEST=1, as the Linux matrix does): the member's pid is reused by a stranger
    /// between the identity check and the kill (the WRONG_FREEZE seam makes the check pass, as
    /// that race does), so the STOP lands on the stranger. The identity is read AFTER the STOP,
    /// so the stranger is recorded, and it is resumed whatever its state was: SD_MEMBER_STOPPED
    /// (the user had stopped the member, the stranger runs), SD_OTHER_STOPPED (another actor had
    /// stopped the stranger: resumed too, the stated cost of an unknowable prior state), neither
    /// (both running).
    #[cfg(target_os = "linux")]
    #[test]
    fn a_stop_that_lands_on_a_reused_pid_is_rolled_back() {
        if std::env::var("SD_REUSE_TEST").is_err() {
            return;
        }
        std::env::set_var("SHEEPDOG_TEST_PIDFD_ENOSYS", "1");
        let log = std::env::temp_dir().join(format!("sd-race-log-{}", std::process::id()));
        std::env::set_var("SHEEPDOG_TEST_SIGNAL_LOG", &log);
        for leg in ["member-stopped", "other-stopped", "both-running", "stranger-is-member"] {
            let _ = std::fs::remove_file(&log);
            let mut m = std::process::Command::new("/bin/sleep").arg("300").spawn().unwrap();
            let mp = m.id() as i32;
            std::thread::sleep(Duration::from_millis(50));
            let mid = sheepdog::ident::identity(mp).unwrap();
            if leg == "member-stopped" {
                unsafe { libc::kill(mp, libc::SIGSTOP) };
                while proc_state(mp) != 'T' {
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
            let stranger: std::cell::RefCell<Option<std::process::Child>> = std::cell::RefCell::new(None);
            let sent_stop: std::cell::Cell<Option<Sent>> = std::cell::Cell::new(None);
            let mut calls = 0;
            let opts = KillOpts { deadline: Duration::from_secs(2), grace: Duration::ZERO, forget: false, never_empty: false, panic_after_stop: false, wrong_freeze: None };
            let r = kill_tree(
                &opts,
                || {
                    calls += 1;
                    if calls == 1 {
                        vec![(mp, mid)]
                    } else if leg == "stranger-is-member" && stranger.borrow().is_some() {
                        // the stranger is itself a member of the job (another member forked it)
                        sheepdog::ident::identity(mp).map(|sid| vec![(mp, sid)]).unwrap_or_default()
                    } else {
                        vec![]
                    }
                },
                || {},
                || None,
                |p, id, sig| {
                    if sig == libc::SIGSTOP && p == mp && stranger.borrow().is_none() {
                        // the identity check passed (the member was there); before the kill the
                        // member dies and its pid goes to a stranger
                        unsafe { libc::kill(mp, libc::SIGKILL) };
                        let _ = m.wait();
                        std::fs::write("/proc/sys/kernel/ns_last_pid", format!("{}", mp - 1)).unwrap();
                        let s = std::process::Command::new("/bin/sleep").arg("301").spawn().unwrap();
                        let (spid, got_pid) = (s.id() as i32, s.id() as i32 == mp);
                        // kept before the controls, so a failed control cannot leak it (the
                        // cleanup below kills it)
                        *stranger.borrow_mut() = Some(s);
                        if !got_pid {
                            unsafe { libc::kill(spid, libc::SIGKILL) };
                        }
                        assert!(got_pid, "control: the stranger did not get the member's pid");
                        std::thread::sleep(Duration::from_millis(20));
                        if sheepdog::ident::same(mp, mid) {
                            unsafe { libc::kill(mp, libc::SIGKILL) };
                        }
                        assert!(!sheepdog::ident::same(mp, mid), "control: the stranger shares the member's start tick");
                        if leg == "other-stopped" {
                            unsafe { libc::kill(mp, libc::SIGSTOP) };
                            while proc_state(mp) != 'T' {
                                std::thread::sleep(Duration::from_millis(1));
                            }
                        }
                        std::env::set_var("SHEEPDOG_TEST_WRONG_FREEZE", mp.to_string());
                        let sent = signal(p, id, sig);
                        std::env::remove_var("SHEEPDOG_TEST_WRONG_FREEZE");
                        sent_stop.set(Some(sent));
                        return sent;
                    }
                    signal(p, id, sig)
                },
                HashMap::new(),
            );
            std::thread::sleep(Duration::from_millis(100));
            let state = proc_state(mp);
            let mut s = stranger.borrow_mut().take().expect("control: the reuse was never staged");
            let alive = matches!(s.try_wait(), Ok(None));
            let _ = s.kill();
            let _ = s.wait();
            let trace = std::fs::read_to_string(&log).unwrap_or_default();
            assert_eq!(sent_stop.get(), Some(Sent::Unpinned), "{leg}: control: the STOP went by kill");
            assert_eq!(r, Ok(()), "{leg}");
            if leg == "stranger-is-member" {
                // a member is killed, never resumed on the way
                assert!(!alive, "{leg}: the member was not killed");
                assert!(!trace.contains("rollback "), "{leg}: a member was resumed before its KILL: {trace}");
            } else {
                assert!(alive, "{leg}: the stranger our STOP landed on was killed: {trace}");
                assert!(matches!(state, 'S' | 'R'), "{leg}: the stranger our STOP landed on was left in state {state}");
            }
        }
        let _ = std::fs::remove_file(&log);
    }
}
