//! Linux: subreaper-based membership (PLAN.md §2, §3.1, §3.2).
//!
//! With PR_SET_CHILD_SUBREAPER, every orphan in the tree is reparented to this supervisor
//! (measured in round 1, also as non-root in a default container), so the tree is exactly
//! this process's live descendants. The supervisor must reap what it adopts (round 1, F5).

use crate::{code_of, cstrings, deadline_missed, kill_tree, say, Args};
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

/// Spawn the root with posix_spawnp. Its signal dispositions and mask are left exactly as the
/// caller set them (PLAN.md §3.1, cell 23): sheepdog changes none (`#![no_main]`, see main.rs)
/// and passes no SETSIGDEF/SETSIGMASK. posix_spawn also avoids running Rust code in a forked
/// child.
fn spawn(cmd: &[OsString]) -> i32 {
    let argv: Vec<CString> = cstrings(cmd).unwrap_or_else(|e| {
        say!("sheepdog: {e}");
        std::process::exit(125)
    });
    let mut ptrs: Vec<*mut libc::c_char> = argv.iter().map(|c| c.as_ptr() as *mut libc::c_char).collect();
    ptrs.push(std::ptr::null_mut());
    let mut pid: libc::pid_t = 0;
    let rc = unsafe {
        libc::posix_spawnp(&mut pid, ptrs[0], std::ptr::null(), std::ptr::null(), ptrs.as_ptr(), environ)
    };
    if rc != 0 {
        say!("sheepdog: cannot run {}: {}", cmd[0].to_string_lossy(), std::io::Error::from_raw_os_error(rc));
        std::process::exit(if rc == libc::ENOENT { 127 } else { 126 });
    }
    pid
}

/// Children this process already has (for example a shell's background job before it
/// `exec`ed sheepdog). They are not part of the command's tree.
fn preexisting_children() -> Vec<i32> {
    let me = unsafe { libc::getpid() };
    let Ok(dir) = std::fs::read_dir("/proc") else { return Vec::new() };
    dir.flatten()
        .filter_map(|e| e.file_name().to_str()?.parse::<i32>().ok())
        .filter(|&p| stat(p).map_or(false, |(ppid, _)| ppid == me))
        .collect()
}

/// Review round 3: if sheepdog starts with children it did not create, it must not become
/// their subreaper, or it adopts them and their orphans as members. So it forks once, before
/// anything else: the child is a fresh supervisor with no children, and this process only
/// relays TERM, INT and HUP to it and returns its exit code. Returns Some(code) in the relay,
/// None in the supervisor.
fn relay_if_needed() -> Option<i32> {
    if preexisting_children().is_empty() {
        return None;
    }
    unsafe {
        let mut fwd: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut fwd);
        for s in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
            libc::sigaddset(&mut fwd, s);
        }
        let mut old: libc::sigset_t = std::mem::zeroed();
        libc::sigprocmask(libc::SIG_BLOCK, &fwd, &mut old);
        match libc::fork() {
            0 => {
                // the supervisor: restore the caller's mask, so the root inherits it
                libc::sigprocmask(libc::SIG_SETMASK, &old, std::ptr::null_mut());
                None
            }
            -1 => {
                libc::sigprocmask(libc::SIG_SETMASK, &old, std::ptr::null_mut());
                say!("sheepdog: fork failed: {}", std::io::Error::last_os_error());
                Some(125)
            }
            sup => loop {
                let mut st = 0;
                if libc::waitpid(sup, &mut st, libc::WNOHANG) == sup {
                    return Some(code_of(st));
                }
                let ts = libc::timespec { tv_sec: 0, tv_nsec: 50_000_000 };
                let sig = libc::sigtimedwait(&fwd, std::ptr::null_mut(), &ts);
                if sig > 0 {
                    libc::kill(sup, sig);
                }
            },
        }
    }
}

pub fn run(a: &Args) -> i32 {
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
    let root = spawn(&a.cmd);
    let mut code = 125;
    loop {
        let mut st = 0;
        let r = unsafe { libc::waitpid(-1, &mut st, 0) };
        if r == root {
            code = code_of(st);
            break;
        }
        if r < 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            break;
        }
    }
    // ECHILD is authoritative only when every orphan comes back here (review round 3, F5)
    let opts = crate::KillOpts::from_env();
    let result = if is_subreaper {
        kill_tree(&opts, || descendants(me), reap, tree_empty, crate::signal)
    } else {
        kill_tree(&opts, || descendants(me), reap, || None, crate::signal)
    };
    reap();
    match result {
        Ok(()) => code,
        Err(alive) => deadline_missed(&alive),
    }
}
