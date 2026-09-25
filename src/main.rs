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

/// Arguments as C strings, byte for byte (non-UTF-8 arguments pass unchanged).
pub fn cstrings(v: &[OsString]) -> Vec<CString> {
    v.iter().map(|s| CString::new(s.as_bytes()).unwrap_or_default()).collect()
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
fn signal(pid: i32, id: u64, sig: c_int) {
    // Test seam (debug builds only): SHEEPDOG_TEST_NOKILL=1 makes every signal fail, as EPERM
    // would after a member's setuid exec (cells 20 and 24-lite).
    if cfg!(debug_assertions) && std::env::var("SHEEPDOG_TEST_NOKILL").as_deref() == Ok("1") {
        return;
    }
    if same(pid, id) {
        unsafe { libc::kill(pid, sig) };
    }
}

fn seam(name: &str) -> bool {
    cfg!(debug_assertions) && std::env::var(name).as_deref() == Ok("1")
}

/// PLAN.md §3.3 steps 2-6 (the spike still omits the TERM grace).
///
/// - `members` returns the live members right now as (pid, identity).
/// - `reap` runs every pass (Linux reaps adopted orphans).
/// - `tree_empty` is an AUTHORITATIVE emptiness check where the platform has one: Linux
///   answers Some(no children left) (the subreaper is the parent of the topmost live member,
///   so ECHILD means the tree is empty; a scan of /proc is not atomic and can miss a tree that
///   moves faster than the scan: phase-0 fix review, P1-A). macOS has none and answers None;
///   the scan-based rule applies there (a stated residual, PLAN.md §7.1).
///
/// Membership is sticky: once seen, a process stays in `known` until it is confirmed dead.
/// Without an authoritative check, two consecutive passes with nothing known alive end it.
/// The deadline is checked at the top of EVERY pass, so it bounds the runtime whatever the
/// tree does. Returns the pids still alive at the deadline, if any (possibly none that a scan
/// can list, when only the authoritative check says the tree is not empty).
pub fn kill_tree(
    members: impl FnMut() -> Vec<(i32, u64)>,
    mut reap: impl FnMut(),
    mut tree_empty: impl FnMut() -> Option<bool>,
) -> Result<(), Vec<i32>> {
    // Test seam (debug builds only): SHEEPDOG_TEST_DEADLINE_MS shortens the 10 s deadline.
    let limit = std::env::var("SHEEPDOG_TEST_DEADLINE_MS")
        .ok()
        .filter(|_| cfg!(debug_assertions))
        .and_then(|v| v.parse().ok())
        .map(Duration::from_millis)
        .unwrap_or(Duration::from_secs(10));
    let deadline = Instant::now() + limit;
    // Test seams (debug builds only): SHEEPDOG_TEST_FORGET=1 hides every member from all scans
    // after the first one that reported it (cell 24-lite); SHEEPDOG_TEST_NEVER_EMPTY=1 makes
    // every emptiness check answer "not empty" (the deadline-bound test).
    let forget = seam("SHEEPDOG_TEST_FORGET");
    let never_empty = seam("SHEEPDOG_TEST_NEVER_EMPTY");
    let mut seen: std::collections::HashSet<(i32, u64)> = std::collections::HashSet::new();
    let mut members = members;
    let mut scan = move || -> Vec<(i32, u64)> {
        let found = members();
        if !forget {
            return found;
        }
        let fresh: Vec<(i32, u64)> = found.into_iter().filter(|m| !seen.contains(m)).collect();
        seen.extend(fresh.iter().copied());
        fresh
    };
    let mut empty_check = move || if never_empty { Some(false) } else { tree_empty() };

    let mut known: HashMap<i32, u64> = HashMap::new();
    let add = |known: &mut HashMap<i32, u64>, found: Vec<(i32, u64)>| {
        for (p, id) in found {
            match known.get(&p) {
                Some(&old) if same(p, old) => {}
                _ => {
                    known.insert(p, id);
                }
            }
        }
    };
    let mut empty = 0;
    loop {
        reap();
        add(&mut known, scan());
        known.retain(|&p, &mut id| same(p, id));
        if Instant::now() > deadline {
            // give SIGKILLs sent in the last pass a moment to take effect, then decide
            std::thread::sleep(Duration::from_millis(100));
            reap();
            let alive: Vec<i32> = known.iter().filter(|(&p, &id)| same(p, id)).map(|(&p, _)| p).collect();
            return match (alive.is_empty(), empty_check()) {
                (true, Some(true)) | (true, None) => Ok(()),
                _ => Err(alive),
            };
        }
        let authoritative = empty_check();
        if authoritative == Some(true) {
            return Ok(());
        }
        if known.is_empty() {
            // no authoritative answer: two consecutive empty passes; an authoritative
            // "not empty" never counts as empty (members exist that this scan did not see)
            empty = if authoritative.is_none() { empty + 1 } else { 0 };
            if empty >= 2 {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(1));
            continue;
        }
        empty = 0;
        // freeze what we know, then close over members created meanwhile, then kill all
        for (&p, &id) in &known {
            signal(p, id, libc::SIGSTOP);
        }
        let before: Vec<i32> = known.keys().copied().collect();
        add(&mut known, scan());
        for (&p, &id) in &known {
            if !before.contains(&p) {
                signal(p, id, libc::SIGSTOP);
            }
        }
        for (&p, &id) in &known {
            signal(p, id, libc::SIGKILL);
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
