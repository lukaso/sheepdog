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
    say!("usage: sheepdog run [--mode M] -- command [args...]");
    125
}

fn parse(argv: Vec<OsString>) -> Result<Args, i32> {
    let args = &argv[1.min(argv.len())..];
    if args.first().map(|a| a.as_bytes()) != Some(b"run") {
        return Err(usage());
    }
    let sep = args.iter().position(|a| a.as_bytes() == b"--").ok_or_else(usage)?;
    let mut mode = None;
    let mut i = 1;
    while i < sep {
        match args[i].as_bytes() {
            b"--mode" if i + 1 < sep => {
                mode = Some(args[i + 1].to_string_lossy().into_owned());
                i += 2;
            }
            _ => return Err(usage()),
        }
    }
    let cmd = args[sep + 1..].to_vec();
    if cmd.is_empty() {
        return Err(usage());
    }
    Ok(Args { argv, mode, cmd })
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
pub fn signal(pid: i32, id: u64, sig: c_int) {
    // Test seam (debug builds only): SHEEPDOG_TEST_NOKILL=1 makes every signal fail, as EPERM
    // would after a member's setuid exec (cells 20 and 24-lite).
    if seam("SHEEPDOG_TEST_NOKILL") {
        return;
    }
    if same(pid, id) {
        unsafe { libc::kill(pid, sig) };
    }
}

fn seam(name: &str) -> bool {
    cfg!(debug_assertions) && std::env::var(name).as_deref() == Ok("1")
}

/// Options of the kill loop. Production: `KillOpts::from_env()` (10 s, no seams). The debug
/// seams exist only in debug builds.
pub struct KillOpts {
    pub deadline: Duration,
    /// hide every member from all scans after the first one that reported it (cell 24-lite)
    pub forget: bool,
    /// every emptiness check answers "not empty" (the deadline-bound test)
    pub never_empty: bool,
    /// panic right after the first SIGSTOP pass (the panic-safety test)
    pub panic_after_stop: bool,
}

impl KillOpts {
    pub fn from_env() -> Self {
        KillOpts {
            deadline: std::env::var("SHEEPDOG_TEST_DEADLINE_MS")
                .ok()
                .filter(|_| cfg!(debug_assertions))
                .and_then(|v| v.parse().ok())
                .map(Duration::from_millis)
                .unwrap_or(Duration::from_secs(10)),
            forget: seam("SHEEPDOG_TEST_FORGET"),
            never_empty: seam("SHEEPDOG_TEST_NEVER_EMPTY"),
            panic_after_stop: seam("SHEEPDOG_TEST_PANIC_AFTER_STOP"),
        }
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

/// PLAN.md §3.3 steps 2-6 (the spike still omits the TERM grace).
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
    send: impl FnMut(i32, u64, c_int),
) -> Result<(), KillError> {
    let known: std::cell::RefCell<HashMap<i32, u64>> = Default::default();
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
    mut send: impl FnMut(i32, u64, c_int),
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
    let mut empty = 0;
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
        // freeze what we know, then close over members created meanwhile, then kill all
        let before: Vec<(i32, u64)> = known.borrow().iter().map(|(&p, &id)| (p, id)).collect();
        for &(p, id) in &before {
            send(p, id, libc::SIGSTOP);
        }
        if opts.panic_after_stop {
            panic!("test seam: panic after the freeze");
        }
        refresh(&mut known.borrow_mut(), scan());
        let all: Vec<(i32, u64)> = known.borrow().iter().map(|(&p, &id)| (p, id)).collect();
        for &(p, id) in &all {
            if !before.iter().any(|&(b, _)| b == p) {
                send(p, id, libc::SIGSTOP);
            }
        }
        for &(p, id) in &all {
            send(p, id, libc::SIGKILL);
        }
        std::thread::sleep(Duration::from_millis(1));
    }
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
    // sheepdog must see its children's exits (P1-B); the root inherits this default
    unsafe { libc::signal(libc::SIGCHLD, libc::SIG_DFL) };
    let args = match parse(argv) {
        Ok(a) => a,
        Err(code) => return code,
    };
    #[cfg(target_os = "macos")]
    let code = macos::run(&args);
    #[cfg(target_os = "linux")]
    let code = linux::run(&args);
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
        let opts = KillOpts { deadline: Duration::ZERO, forget: false, never_empty: false, panic_after_stop: false };
        let r = kill_tree(
            &opts,
            || {
                calls += 1;
                if calls <= 2 { vec![] } else { vec![(pid, id)] } // missed twice, then seen
            },
            || {},
            || None,
            |_, _, _| {}, // never signal: the member stays alive
        );
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(r, Err(KillError::Deadline(vec![pid])));
    }
}
