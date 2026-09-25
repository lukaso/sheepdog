//! sheepdog, phase-0 spike (PLAN.md §7).
//!
//! `sheepdog run [--mode M] -- cmd...` runs cmd, waits for it, then kills every member of its
//! tree (PLAN.md §3.3: freeze, close, kill, repeat until two passes find nothing alive, or the
//! deadline).
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
//! root inherits exactly the caller's dispositions and mask (PLAN.md §3.1, cell 23).
#![cfg_attr(not(test), no_main)]
#![cfg_attr(test, allow(dead_code))]

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

use sheepdog::ident::same;
use std::collections::HashMap;
#[cfg(not(test))]
use std::ffi::CStr;
#[cfg(not(test))]
use std::os::raw::c_char;
use std::os::raw::c_int;
use std::time::{Duration, Instant};

pub struct Args {
    /// argv exactly as received, for the macOS self re-exec
    pub argv: Vec<String>,
    pub mode: Option<String>,
    pub cmd: Vec<String>,
}

fn parse(argv: Vec<String>) -> Args {
    let usage = || -> ! {
        eprintln!("usage: sheepdog run [--mode M] -- command [args...]");
        std::process::exit(125)
    };
    let args = &argv[1.min(argv.len())..];
    if args.first().map(String::as_str) != Some("run") {
        usage();
    }
    let sep = args.iter().position(|a| a == "--").unwrap_or_else(|| usage());
    let mut mode = None;
    let mut i = 1;
    while i < sep {
        match args[i].as_str() {
            "--mode" if i + 1 < sep => {
                mode = Some(args[i + 1].clone());
                i += 2;
            }
            _ => usage(),
        }
    }
    let cmd = args[sep + 1..].to_vec();
    if cmd.is_empty() {
        usage();
    }
    Args { argv, mode, cmd }
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

/// PLAN.md §3.3 steps 2-6 (the spike still omits the TERM grace). `members` returns the live
/// members right now as (pid, identity); `reap` runs every pass (Linux reaps adopted orphans).
///
/// Membership is sticky: once seen, a process stays in `known` until it is confirmed dead
/// (its identity is gone), even if a later scan stops reporting it. A pass counts as empty
/// only when nothing is known alive, so a member that lost its fact is still killed.
/// Returns the pids still alive at the deadline, if any.
pub fn kill_tree(mut members: impl FnMut() -> Vec<(i32, u64)>, mut reap: impl FnMut()) -> Result<(), Vec<i32>> {
    // Test seam (debug builds only): SHEEPDOG_TEST_DEADLINE_MS shortens the 10 s deadline.
    let limit = std::env::var("SHEEPDOG_TEST_DEADLINE_MS")
        .ok()
        .filter(|_| cfg!(debug_assertions))
        .and_then(|v| v.parse().ok())
        .map(Duration::from_millis)
        .unwrap_or(Duration::from_secs(10));
    let deadline = Instant::now() + limit;
    let mut known: HashMap<i32, u64> = HashMap::new();
    // Test seam (debug builds only): SHEEPDOG_TEST_FORGET=1 hides every member from all scans
    // after the first scan that reported it, as if it lost its fact (cell 24-lite).
    let forget = cfg!(debug_assertions) && std::env::var("SHEEPDOG_TEST_FORGET").as_deref() == Ok("1");
    let mut seen: std::collections::HashSet<(i32, u64)> = std::collections::HashSet::new();
    let mut members = move || -> Vec<(i32, u64)> {
        let found = members();
        if !forget {
            return found;
        }
        let fresh: Vec<(i32, u64)> = found.into_iter().filter(|m| !seen.contains(m)).collect();
        seen.extend(fresh.iter().copied());
        fresh
    };
    let mut empty = 0;
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
    loop {
        reap();
        add(&mut known, members());
        known.retain(|&p, &mut id| same(p, id));
        if known.is_empty() {
            empty += 1;
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
        add(&mut known, members());
        for (&p, &id) in &known {
            if !before.contains(&p) {
                signal(p, id, libc::SIGSTOP);
            }
        }
        for (&p, &id) in &known {
            signal(p, id, libc::SIGKILL);
        }
        if Instant::now() > deadline {
            let alive: Vec<i32> = known.iter().filter(|(&p, &id)| same(p, id)).map(|(&p, _)| p).collect();
            if !alive.is_empty() {
                return Err(alive);
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Report a missed deadline (PLAN.md §3.3 step 6) and return exit code 125.
pub fn deadline_missed(alive: &[i32]) -> i32 {
    eprintln!(
        "sheepdog: {} process(es) still alive at the kill deadline: pids {:?}. The tree is NOT clean.",
        alive.len(),
        alive
    );
    125
}

#[cfg(not(test))]
#[no_mangle]
pub extern "C" fn main(argc: c_int, argv: *const *const c_char) -> c_int {
    let argv: Vec<String> = (0..argc as usize)
        .map(|i| unsafe { CStr::from_ptr(*argv.add(i)) }.to_string_lossy().into_owned())
        .collect();
    let args = parse(argv);
    #[cfg(target_os = "macos")]
    let code = macos::run(&args);
    #[cfg(target_os = "linux")]
    let code = linux::run(&args);
    code
}
