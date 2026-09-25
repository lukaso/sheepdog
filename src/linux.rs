//! Linux: subreaper-based membership (PLAN.md §2, §3.1, §3.2).
//!
//! With PR_SET_CHILD_SUBREAPER, every orphan in the tree is reparented to this supervisor
//! (measured in round 1, also as non-root in a default container), so the tree is exactly
//! this process's live descendants. The supervisor must reap what it adopts (round 1, F5).

use crate::{code_of, kill_tree, Args};
use std::collections::HashMap;
use std::ffi::CString;
use std::os::unix::fs::MetadataExt;

/// (ppid, state) from /proc/<pid>/stat; the command name may contain spaces or ')'.
fn stat(pid: i32) -> Option<(i32, char)> {
    let s = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = &s[s.rfind(')')? + 2..];
    let mut f = rest.split_whitespace();
    let state = f.next()?.chars().next()?;
    let ppid = f.next()?.parse().ok()?;
    Some((ppid, state))
}

/// Live (not zombie) same-uid descendants of `root`.
fn descendants(root: i32) -> Vec<i32> {
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
    out
}

/// Reap every adopted orphan that has exited.
fn reap() {
    let mut st = 0;
    while unsafe { libc::waitpid(-1, &mut st, libc::WNOHANG) } > 0 {}
}

/// Fork and exec the root with default signal dispositions and an empty mask (PLAN.md §3.1).
fn spawn(cmd: &[String]) -> i32 {
    let argv: Vec<CString> = cmd.iter().map(|s| CString::new(s.as_str()).unwrap()).collect();
    let mut ptrs: Vec<*const libc::c_char> = argv.iter().map(|c| c.as_ptr()).collect();
    ptrs.push(std::ptr::null());
    unsafe {
        let pid = libc::fork();
        if pid == 0 {
            for sig in 1..32 {
                libc::signal(sig, libc::SIG_DFL);
            }
            let mut none: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut none);
            libc::sigprocmask(libc::SIG_SETMASK, &none, std::ptr::null_mut());
            libc::execvp(ptrs[0], ptrs.as_ptr());
            let e = *libc::__errno_location();
            libc::_exit(if e == libc::ENOENT { 127 } else { 126 });
        }
        if pid < 0 {
            eprintln!("sheepdog: fork failed: {}", std::io::Error::last_os_error());
            std::process::exit(125);
        }
        pid
    }
}

pub fn run(a: &Args) -> i32 {
    let subreaper = match a.mode.as_deref() {
        None | Some("subreaper") => true,
        Some("none") => false,
        Some(m) => {
            eprintln!("sheepdog: unknown mode {m}");
            return 125;
        }
    };
    if subreaper && unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) } != 0 {
        eprintln!("sheepdog: cannot become a subreaper; tracking is degraded");
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
    if !kill_tree(|| descendants(me), reap) {
        return 125;
    }
    reap();
    code
}
