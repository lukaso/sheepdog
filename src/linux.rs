//! Linux: subreaper-based membership (PLAN.md §2, §3.1, §3.2).
//!
//! With PR_SET_CHILD_SUBREAPER, every orphan in the tree is reparented to this supervisor
//! (measured in round 1, also as non-root in a default container), so the tree is exactly
//! this process's live descendants. The supervisor must reap what it adopts (round 1, F5).

use crate::{code_of, cstrings, kill_tree, say, Args};
use std::ffi::OsString;
use std::io::Write;
use sheepdog::ident::identity;
use std::collections::HashMap;
use std::ffi::CString;
use std::os::unix::fs::MetadataExt;

/// (ppid, state) from /proc/<pid>/stat; the command name may contain spaces or ')'.
fn stat(pid: i32) -> Option<(i32, char)> {
    let s = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = s.get(s.rfind(')')? + 2..)?;
    let mut f = rest.split_whitespace();
    let state = f.next()?.chars().next()?;
    let ppid = f.next()?.parse().ok()?;
    Some((ppid, state))
}

/// Live (not zombie) same-uid descendants of `root`, with their identities.
fn descendants(root: i32) -> Vec<(i32, u64)> {
    let uid = unsafe { libc::getuid() };
    let mut kids: HashMap<i32, Vec<i32>> = HashMap::new();
    if let Ok(dir) = std::fs::read_dir("/proc") {
        for e in dir.flatten() {
            let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<i32>().ok()) else {
                continue;
            };
            if e.metadata().map(|m| m.uid()).ok() != Some(uid) {
                continue;
            }
            if let Some((ppid, state)) = stat(pid) {
                if state != 'Z' {
                    kids.entry(ppid).or_default().push(pid);
                }
            }
        }
    }
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(p) = stack.pop() {
        for &c in kids.get(&p).map(Vec::as_slice).unwrap_or(&[]) {
            if !out.contains(&c) {
                out.push(c);
                stack.push(c);
            }
        }
    }
    out.into_iter().filter_map(|p| Some((p, identity(p)?))).collect()
}

/// Reap every adopted orphan that has exited.
fn reap() {
    let mut st = 0;
    while unsafe { libc::waitpid(-1, &mut st, libc::WNOHANG) } > 0 {}
}

/// The authoritative emptiness check (phase-0 fix review, P1-A): with the subreaper set, every
/// live member of the tree has a live ancestor that is this process's child, or is its child
/// itself. So "no children at all" (waitpid reports ECHILD) means the tree is empty, atomically,
/// whatever the tree does between two /proc scans.
fn tree_empty() -> Option<bool> {
    let mut st = 0;
    loop {
        match unsafe { libc::waitpid(-1, &mut st, libc::WNOHANG) } {
            0 => return Some(false),
            r if r > 0 => continue, // reaped one; ask again
            _ => {
                let e = std::io::Error::last_os_error().raw_os_error();
                if e == Some(libc::EINTR) {
                    continue;
                }
                return Some(e == Some(libc::ECHILD));
            }
        }
    }
}

extern "C" {
    static environ: *const *mut libc::c_char;
}

/// Spawn the root with posix_spawnp. Its signal dispositions are the caller's (sheepdog
/// changes none except SIGCHLD, set to default: `#![no_main]`, see main.rs), and its mask is
/// set to the caller's with SETSIGMASK (sheepdog itself runs with TERM and SIGCHLD blocked).
/// PLAN.md §3.1, cell 23. posix_spawn also avoids running Rust code in a forked child.
fn spawn(cmd: &[OsString], caller_mask: &libc::sigset_t) -> i32 {
    let argv: Vec<CString> = cstrings(cmd).unwrap_or_else(|e| {
        say!("sheepdog: {e}");
        std::process::exit(125)
    });
    let mut ptrs: Vec<*mut libc::c_char> = argv.iter().map(|c| c.as_ptr() as *mut libc::c_char).collect();
    ptrs.push(std::ptr::null_mut());
    let mut pid: libc::pid_t = 0;
    let rc = unsafe {
        // the root gets the caller's mask (sheepdog blocks TERM and SIGCHLD for itself)
        let mut attr: libc::posix_spawnattr_t = std::mem::zeroed();
        libc::posix_spawnattr_init(&mut attr);
        libc::posix_spawnattr_setsigmask(&mut attr, caller_mask);
        libc::posix_spawnattr_setflags(&mut attr, libc::POSIX_SPAWN_SETSIGMASK as libc::c_short);
        let rc = libc::posix_spawnp(&mut pid, ptrs[0], std::ptr::null(), &attr, ptrs.as_ptr(), environ);
        libc::posix_spawnattr_destroy(&mut attr);
        rc
    };
    if rc != 0 {
        say!("sheepdog: cannot run {}: {}", cmd[0].to_string_lossy(), std::io::Error::from_raw_os_error(rc));
        std::process::exit(if rc == libc::ENOENT { 127 } else { 126 });
    }
    pid
}

/// A signalfd for `set` (PHASE1.md §1.1): CLOEXEC, so it never leaks into the root, and
/// non-blocking, so draining it never blocks. Returns -1 if unavailable (then: poll).
fn signal_fd(set: &libc::sigset_t) -> i32 {
    unsafe { libc::signalfd(-1, set, libc::SFD_CLOEXEC | libc::SFD_NONBLOCK) }
}

/// Read every pending signal from `fd` (each is consumed exactly once) and return them.
fn drain(fd: i32) -> Vec<i32> {
    let mut got = Vec::new();
    loop {
        let mut info: libc::signalfd_siginfo = unsafe { std::mem::zeroed() };
        let n = unsafe {
            libc::read(fd, &mut info as *mut _ as *mut libc::c_void, std::mem::size_of::<libc::signalfd_siginfo>())
        };
        if n != std::mem::size_of::<libc::signalfd_siginfo>() as isize {
            return got;
        }
        got.push(info.ssi_signo as i32);
    }
}

/// Wait until `fd` is readable or `ms` passed.
fn poll_fd(fd: i32, ms: i32) {
    let mut p = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
    unsafe { libc::poll(&mut p, 1, ms) };
}

/// Does this process already have children (for example a shell's background job before it
/// `exec`ed sheepdog)? waitid with WNOWAIT answers atomically without /proc and without reaping
/// anything (review round 4, P3-3).
fn has_children() -> bool {
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    let r = unsafe { libc::waitid(libc::P_ALL, 0, &mut info, libc::WEXITED | libc::WNOHANG | libc::WNOWAIT) };
    r == 0
}

/// Die the way the supervisor died: death by signal N stays death by signal N for our caller,
/// which shells rely on (for example to stop a loop on ctrl-C: review round 4, P2-1).
fn die_like(status: libc::c_int) -> i32 {
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

/// If sheepdog starts with children it did not create, it must not become their subreaper, or
/// it adopts them and their orphans as members (review round 3). So it forks once, before
/// anything else: the child is a fresh supervisor with no children; this process becomes the
/// relay. Returns Some(code) in the relay, None in the supervisor.
///
/// The relay (PLAN.md §3.1):
/// - forwards TERM to the supervisor (a TERM that also reached the supervisor through the
///   process group is harmless: it means "end the job" twice);
/// - forwards HUP only when it is the session leader (then only it gets HUP when the terminal
///   closes); otherwise the terminal's HUP reaches the supervisor directly;
/// - never forwards INT: a terminal INT reaches the whole process group, so forwarding would
///   deliver it twice (review round 4, P2-2). INT is consumed, so the relay does not die of it;
/// - waits on SIGCHLD, so the exit costs no polling delay (review round 4, P3-1);
/// - dies the way the supervisor died (P2-1).
/// The supervisor gets PR_SET_PDEATHSIG(SIGTERM): if the relay is killed, the supervisor is
/// told to end the job instead of running on unseen (review round 4, P3-2).
fn relay_if_needed() -> Option<i32> {
    if !has_children() {
        return None;
    }
    unsafe {
        let relay = libc::getpid();
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        for s in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP, libc::SIGCHLD] {
            libc::sigaddset(&mut set, s);
        }
        let mut old: libc::sigset_t = std::mem::zeroed();
        libc::sigprocmask(libc::SIG_BLOCK, &set, &mut old);
        match libc::fork() {
            0 => {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM, 0, 0, 0);
                if libc::getppid() != relay {
                    libc::raise(libc::SIGTERM); // the relay died before PDEATHSIG was set
                }
                // the supervisor: restore the caller's mask, so the root inherits it
                libc::sigprocmask(libc::SIG_SETMASK, &old, std::ptr::null_mut());
                None
            }
            -1 => {
                libc::sigprocmask(libc::SIG_SETMASK, &old, std::ptr::null_mut());
                say!("sheepdog: fork failed: {}", std::io::Error::last_os_error());
                Some(125)
            }
            sup => {
                // the relay's own loop on a signalfd (PHASE1.md S1), created after the fork
                let fd = signal_fd(&set);
                loop {
                    let mut st = 0;
                    if libc::waitpid(sup, &mut st, libc::WNOHANG) == sup {
                        if fd >= 0 {
                            libc::close(fd);
                        }
                        return Some(die_like(st));
                    }
                    let got = if fd >= 0 {
                        poll_fd(fd, 1000);
                        drain(fd)
                    } else {
                        let ts = libc::timespec { tv_sec: 0, tv_nsec: 50_000_000 };
                        let s = libc::sigtimedwait(&set, std::ptr::null_mut(), &ts);
                        if s > 0 { vec![s] } else { vec![] }
                    };
                    for s in got {
                        match s {
                            libc::SIGTERM => {
                                libc::kill(sup, libc::SIGTERM);
                            }
                            libc::SIGHUP if libc::getsid(0) == relay => {
                                libc::kill(sup, libc::SIGHUP);
                            }
                            _ => {} // SIGCHLD, INT, a HUP we are not the leader for
                        }
                    }
                }
            }
        }
    }
}

pub fn run(a: &Args, sig: &crate::Signals) -> i32 {
    if let Some(code) = relay_if_needed() {
        return code;
    }
    let subreaper = match a.mode.as_deref() {
        None | Some("subreaper") => true,
        Some("none") => false,
        Some(m) => {
            say!("sheepdog: unknown mode {m}");
            return 125;
        }
    };
    let is_subreaper = subreaper && unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) } == 0;
    if subreaper && !is_subreaper {
        say!("sheepdog: cannot become a subreaper; tracking is degraded");
    }
    let me = unsafe { libc::getpid() };
    if let Some(code) = crate::term_before_spawn(sig) {
        return code;
    }
    let root = spawn(&a.cmd, &sig.caller_mask);
    // The event loop's wait (PHASE1.md §1): a signalfd on {CHLD, TERM} (both blocked by
    // setup_signals), drained on every wake so each signal is consumed exactly once; the
    // waitpid(-1, WNOHANG) loop reaps the root and adopted orphans. One fixed order when events
    // coincide (round-7 P3-F4): TERM, then the root's exit. No signalfd: poll every 50 ms (a
    // blocked TERM stays pending, so nothing is lost).
    let mut waitset: libc::sigset_t = unsafe { std::mem::zeroed() };
    unsafe {
        libc::sigemptyset(&mut waitset);
        libc::sigaddset(&mut waitset, libc::SIGCHLD);
        if sig.watch_term {
            libc::sigaddset(&mut waitset, libc::SIGTERM);
        }
    }
    let fd = signal_fd(&waitset);
    crate::seam_sleep("SHEEPDOG_TEST_SLEEP_AFTER_REGISTER_MS");
    let mut exited: Option<i32> = None;
    let code = loop {
        let term = if fd >= 0 {
            drain(fd).contains(&libc::SIGTERM)
        } else {
            sig.watch_term && crate::consume(libc::SIGTERM)
        };
        loop {
            let mut st = 0;
            let r = unsafe { libc::waitpid(-1, &mut st, libc::WNOHANG) };
            if r == root && exited.is_none() {
                exited = Some(code_of(st));
            }
            if r <= 0 {
                break;
            }
        }
        if term {
            break None;
        }
        if let Some(c) = exited {
            break Some(c);
        }
        crate::seam_sleep("SHEEPDOG_TEST_SLEEP_BEFORE_WAIT_MS");
        if fd >= 0 {
            poll_fd(fd, 250);
        } else {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    };
    if fd >= 0 {
        unsafe { libc::close(fd) };
    }
    // ECHILD is authoritative only when every orphan comes back here (review round 3, F5)
    let opts = crate::KillOpts::from_env();
    let result = if is_subreaper {
        kill_tree(&opts, || descendants(me), reap, tree_empty, crate::signal)
    } else {
        kill_tree(&opts, || descendants(me), reap, || None, crate::signal)
    };
    reap();
    if code.is_none() && result.is_ok() {
        return crate::die_by_term(143);
    }
    match result {
        Ok(()) => code.unwrap_or(143),
        Err(e) => crate::kill_failed(e),
    }
}
