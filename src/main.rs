//! sheepdog, phase-0 spike (PLAN.md §7).
//!
//! `sheepdog run [--mode M] -- cmd...` runs cmd, waits for it, then kills every member of its
//! tree (PLAN.md §3.3: freeze, close, kill, repeat until two scans are empty or the deadline).
//!
//! Modes (the non-default ones exist so tests can prove the default is what catches escapees):
//!   macOS: `responsible` (default: the supervisor re-execs itself with disclaim and members
//!          are processes whose responsible uniqueid is the supervisor's), `root-disclaim`
//!          (the round-2 design: the root disclaims; loses escapees once the root exits)
//!   Linux: `subreaper` (default: PR_SET_CHILD_SUBREAPER; members are the supervisor's
//!          descendants), `none` (no subreaper; orphans go to PID 1)

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

use std::time::{Duration, Instant};

pub struct Args {
    pub mode: Option<String>,
    pub cmd: Vec<String>,
}

fn parse() -> Args {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = || -> ! {
        eprintln!("usage: sheepdog run [--mode M] -- command [args...]");
        std::process::exit(125)
    };
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
    Args { mode, cmd }
}

/// Exit code for a wait status: the command's code, or 128+signal.
pub fn code_of(status: libc::c_int) -> i32 {
    if libc::WIFEXITED(status) {
        libc::WEXITSTATUS(status)
    } else if libc::WIFSIGNALED(status) {
        128 + libc::WTERMSIG(status)
    } else {
        125
    }
}

/// PLAN.md §3.3 steps 2-6 (the spike omits the TERM grace and the identity re-check).
/// `members` must return the live members right now; `reap` is called every pass (Linux
/// reaps adopted orphans there). Returns false if members were still alive at the deadline.
pub fn kill_tree(mut members: impl FnMut() -> Vec<i32>, mut reap: impl FnMut()) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut empty = 0;
    loop {
        reap();
        let first = members();
        if first.is_empty() {
            empty += 1;
            if empty >= 2 {
                return true;
            }
            std::thread::sleep(Duration::from_millis(1));
            continue;
        }
        empty = 0;
        for &p in &first {
            unsafe { libc::kill(p, libc::SIGSTOP) };
        }
        let mut all = first;
        for p in members() {
            if !all.contains(&p) {
                unsafe { libc::kill(p, libc::SIGSTOP) };
                all.push(p);
            }
        }
        for &p in &all {
            unsafe { libc::kill(p, libc::SIGKILL) };
        }
        if Instant::now() > deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn main() {
    let args = parse();
    #[cfg(target_os = "macos")]
    let code = macos::run(&args);
    #[cfg(target_os = "linux")]
    let code = linux::run(&args);
    std::process::exit(code);
}
