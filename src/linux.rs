//! Linux: subreaper-based membership (PLAN.md §2, §3.1, §3.2).
//!
//! With PR_SET_CHILD_SUBREAPER, every orphan in the tree is reparented to this supervisor
//! (measured in round 1, also as non-root in a default container), so the tree is exactly
//! this process's live descendants. The supervisor must reap what it adopts (round 1, F5).

use crate::{cstrings, kill_tree, say, Args};
use std::ffi::OsString;
use std::io::Write;
use sheepdog::ident::identity;
use std::collections::HashMap;
use std::ffi::CString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
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

/// The pids in process group `pg` (any state, any user), from /proc.
fn group_pids(pg: i32) -> Vec<i32> {
    let mut v = Vec::new();
    if let Ok(dir) = std::fs::read_dir("/proc") {
        for e in dir.flatten() {
            let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<i32>().ok()) else { continue };
            let Ok(s) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else { continue };
            // after "comm)": state, ppid, pgrp
            let g = s.rfind(')').and_then(|i| s.get(i + 2..)).and_then(|r| r.split_whitespace().nth(2)).and_then(|g| g.parse::<i32>().ok());
            if g == Some(pg) {
                v.push(pid);
            }
        }
    }
    v
}

/// `sheepdog kill`: an inner supervisor that is a subreaper (the default mode) adopts its
/// escapees, so they are its descendants: even if it does not end on the TERM (its caller ignores
/// TERM), `kill` proves them and kills them with its tree, and exits 0. An inner supervisor without
/// its subreaper (`--mode none`, or its prctl failed: degraded) whose caller ignores TERM can
/// leave escapees that were already orphaned out of the tree before `kill` saw them; `kill` then
/// still exits 0, without a report (stated in PHASE1.md, S6 review round 7).
pub const ADOPTS_ESCAPEES: bool = true;

/// Every live (not zombie) process, for `sheepdog kill` (S6). The parent, state and start time
/// come from one read of `/proc/<pid>/stat` (separate reads could mix two processes if the pid
/// were reused in between); the uid is the effective uid from `/proc/<pid>/status`, as on macOS
/// (`pbi_uid`): a setuid program the user runs is not the user's.
pub fn procs() -> Vec<crate::kill::Proc> {
    let mut v = Vec::new();
    if let Ok(dir) = std::fs::read_dir("/proc") {
        for e in dir.flatten() {
            let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<i32>().ok()) else { continue };
            let Ok(s) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else { continue };
            let Some(rest) = s.rfind(')').and_then(|i| s.get(i + 2..)) else { continue };
            let f: Vec<&str> = rest.split_whitespace().collect();
            // after "comm)": state (field 3), ppid (4), ..., start time (22)
            let (Some(st), Some(ppid), Some(id)) = (f.first(), f.get(1).and_then(|x| x.parse().ok()), f.get(22 - 3).and_then(|x| x.parse().ok())) else { continue };
            if *st == "Z" {
                continue;
            }
            let uid = std::fs::read_to_string(format!("/proc/{pid}/status"))
                .ok()
                .and_then(|t| t.lines().find_map(|l| l.strip_prefix("Uid:").and_then(|u| u.split_whitespace().nth(1)?.parse::<u32>().ok())));
            // a process that ended between the two reads, or whose pid was reused, is left out
            if let (Some(uid), true) = (uid, identity(pid) == Some(id)) {
                v.push(crate::kill::Proc { pid, ppid, uid, id, puniq: None });
            }
        }
    }
    v
}

/// The parent pid of `pid`, if it is alive.
pub fn parent(pid: i32) -> Option<i32> {
    stat(pid).map(|(ppid, _)| ppid)
}

/// The file name of `pid`'s executable (` (deleted)` stripped: the binary was replaced). Under
/// a binary translator (Rosetta runs amd64 containers on Apple silicon; qemu-user) the link names
/// the translator, so the program's own name is argv[0]'s file name.
pub fn exe_name(pid: i32) -> Option<String> {
    let p = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?;
    let s = p.to_string_lossy();
    let s = s.strip_suffix(" (deleted)").unwrap_or(&s);
    let name = s.rsplit('/').next()?.to_string();
    if name == "rosetta" || name.starts_with("qemu-") {
        return cmdline(pid).first().and_then(|a| a.rsplit('/').next()).map(String::from);
    }
    Some(name)
}

/// `pid`'s argv (empty if unreadable).
pub fn cmdline(pid: i32) -> Vec<String> {
    std::fs::read(format!("/proc/{pid}/cmdline"))
        .map(|b| b.split(|&c| c == 0).filter(|a| !a.is_empty()).map(|a| String::from_utf8_lossy(a).into_owned()).collect())
        .unwrap_or_default()
}

/// Linux has no responsible process.
pub fn responsible_pid(_pid: i32) -> Option<i32> {
    None
}

/// Is /proc this process's own pid namespace's? Under `unshare --pid --fork` without
/// `--mount-proc` it is another namespace's: every pid, parent and start time read from it names
/// someone else's process, and a signal sent by that pid reaches whoever has it here.
pub fn proc_is_ours() -> bool {
    std::fs::read_link("/proc/self").ok().and_then(|p| p.to_str()?.parse::<i32>().ok()) == Some(unsafe { libc::getpid() })
}

/// Why /proc cannot be used, for the refusal (None: it is ours). No /proc at all is told apart
/// from another namespace's.
pub fn proc_problem() -> Option<&'static str> {
    if proc_is_ours() {
        None
    } else if std::fs::read_link("/proc/self").is_err() {
        Some("no /proc is mounted")
    } else {
        Some("/proc belongs to another pid namespace")
    }
}

/// Stopped by a signal (state T; t is a ptrace stop)?
pub fn stopped(pid: i32) -> bool {
    stat(pid).is_some_and(|(_, st)| st == 'T')
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

/// The root, started through the shim (PHASE2.md §1 decision 7): its pid, the write end of its
/// go pipe (the go byte is sent once the root is journaled), and the read end of its error pipe
/// (an exec failure is reported there; EOF means it exec'd).
pub struct Root {
    pub pid: i32,
    go: i32,
    err: i32,
}

impl Root {
    /// Let the root run its command.
    pub fn go(&mut self) {
        if self.go >= 0 {
            unsafe {
                libc::write(self.go, b"g".as_ptr() as *const libc::c_void, 1);
                libc::close(self.go);
            }
            self.go = -1;
        }
    }
    /// After the root has exited: did the shim report that its command never ran?
    pub fn exec_failed(&mut self) -> bool {
        let mut buf = [0u8; 64];
        let n = unsafe { libc::read(self.err, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
        n > 0 && buf[0] == b'E'
    }
}

/// The path this binary is spawned by for the shim: the literal /proc/self/exe (an in-place
/// upgrade or a removed file does not break it), unless that names a translator (Rosetta, qemu:
/// the emulated leg), then AT_EXECFN made absolute (never argv[0], which the caller controls).
fn self_exe() -> Option<CString> {
    let link = std::fs::read_link("/proc/self/exe").ok()?;
    let name = link.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if name != "rosetta" && !name.starts_with("qemu-") {
        return CString::new("/proc/self/exe").ok();
    }
    let p = unsafe { libc::getauxval(libc::AT_EXECFN) } as *const libc::c_char;
    if p.is_null() {
        return None;
    }
    let s = std::path::PathBuf::from(std::ffi::OsStr::from_bytes(unsafe { std::ffi::CStr::from_ptr(p) }.to_bytes()));
    let abs = if s.is_absolute() { s } else { std::env::current_dir().ok()?.join(s) };
    CString::new(abs.into_os_string().into_vec()).ok()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

/// Read one handshake line from `fd` within `ms`.
fn read_line(fd: i32, ms: i32) -> Option<String> {
    let end = std::time::Instant::now() + std::time::Duration::from_millis(ms as u64);
    let mut out = Vec::new();
    loop {
        let left = end.saturating_duration_since(std::time::Instant::now()).as_millis() as i32;
        if left <= 0 {
            return None;
        }
        let mut p = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        if unsafe { libc::poll(&mut p, 1, left) } <= 0 {
            continue;
        }
        let mut b = [0u8; 1];
        let n = unsafe { libc::read(fd, b.as_mut_ptr() as *mut libc::c_void, 1) };
        if n <= 0 {
            return (!out.is_empty()).then(|| String::from_utf8_lossy(&out).into_owned());
        }
        if b[0] == b'\n' {
            return Some(String::from_utf8_lossy(&out).into_owned());
        }
        out.push(b[0]);
    }
}

/// Start the root through the shim with every signal blocked; it runs its command only after
/// `Root::go`. Its signal dispositions are the caller's (sheepdog changes none except SIGCHLD,
/// `#![no_main]`, see main.rs); the shim restores the caller's mask as its last step before the
/// exec (PLAN.md §3.1, cell 23). posix_spawn also avoids running Rust code in a forked child.
fn spawn(cmd: &[OsString], caller_mask: &libc::sigset_t) -> Result<Root, i32> {
    let argv: Vec<CString> = cstrings(cmd).map_err(|e| {
        say!("sheepdog: {e}");
        125
    })?;
    let Some(exe) = self_exe() else {
        say!("sheepdog: cannot find its own executable to start the command");
        return Err(125);
    };
    let (mut go, mut er) = ([0i32; 2], [0i32; 2]);
    unsafe {
        if libc::pipe2(go.as_mut_ptr(), libc::O_CLOEXEC) != 0 || libc::pipe2(er.as_mut_ptr(), libc::O_CLOEXEC) != 0 {
            say!("sheepdog: cannot make a pipe: {}", std::io::Error::last_os_error());
            return Err(125);
        }
        // the shim's ends cross its exec; the shim sets them CLOEXEC again before the command's
        libc::fcntl(go[0], libc::F_SETFD, 0);
        libc::fcntl(er[1], libc::F_SETFD, 0);
    }
    let mut nonce = [0u8; 8];
    let _ = std::fs::File::open("/dev/urandom").and_then(|mut f| std::io::Read::read_exact(&mut f, &mut nonce));
    let nonce = hex(&nonce);
    let mask = hex(unsafe { std::slice::from_raw_parts(caller_mask as *const _ as *const u8, std::mem::size_of::<libc::sigset_t>()) });
    let me = unsafe { libc::getpid() };
    let mut shim: Vec<CString> = ["sheepdog", "__root", &go[0].to_string(), &er[1].to_string(), &me.to_string(), &mask, &nonce, "--"]
        .iter()
        .map(|s| CString::new(*s).unwrap())
        .collect();
    shim.extend(argv);
    let mut ptrs: Vec<*mut libc::c_char> = shim.iter().map(|c| c.as_ptr() as *mut libc::c_char).collect();
    ptrs.push(std::ptr::null_mut());
    let mut pid: libc::pid_t = 0;
    let rc = unsafe {
        let mut attr: libc::posix_spawnattr_t = std::mem::zeroed();
        libc::posix_spawnattr_init(&mut attr);
        let mut all: libc::sigset_t = std::mem::zeroed();
        libc::sigfillset(&mut all);
        libc::posix_spawnattr_setsigmask(&mut attr, &all);
        libc::posix_spawnattr_setflags(&mut attr, libc::POSIX_SPAWN_SETSIGMASK as libc::c_short);
        let rc = libc::posix_spawn(&mut pid, exe.as_ptr(), std::ptr::null(), &attr, ptrs.as_ptr(), environ);
        libc::posix_spawnattr_destroy(&mut attr);
        libc::close(go[0]);
        libc::close(er[1]);
        rc
    };
    if rc != 0 {
        unsafe {
            libc::close(go[1]);
            libc::close(er[0]);
        }
        say!("sheepdog: cannot start the command: {}", std::io::Error::from_raw_os_error(rc));
        return Err(125);
    }
    // the handshake: the shim is running, is this protocol, and holds its PDEATHSIG and parent
    // (5 s: a cold start under an emulator)
    let line = read_line(er[0], 5000);
    if line.as_deref() != Some(format!("H{nonce} 1").as_str()) {
        unsafe {
            libc::kill(pid, libc::SIGKILL); // raw signal site: the shim this supervisor spawned (PHASE2.md §0.3)
            let mut st = 0;
            libc::waitpid(pid, &mut st, 0);
            libc::close(go[1]);
            libc::close(er[0]);
        }
        let why = match line.as_deref() {
            Some(l) if l.starts_with('F') => l[1..].to_string(),
            Some(_) => "an unexpected answer".to_string(),
            None => "no answer".to_string(),
        };
        say!("sheepdog: the command could not be started ({why}); it did not run");
        return Err(125);
    }
    unsafe { libc::fcntl(er[0], libc::F_SETFL, libc::O_NONBLOCK) };
    Ok(Root { pid, go: go[1], err: er[0] })
}

/// `sheepdog __root GO ERR SUP MASK NONCE -- cmd...` (PHASE2.md §1 decision 7): the root before
/// its exec. Every signal is blocked on entry (the supervisor's spawn attribute). It sets
/// PR_SET_PDEATHSIG(SIGKILL), checks that its parent is the supervisor, answers the handshake,
/// waits for the go byte (EOF: the supervisor is gone, so it never execs), closes its pipe ends
/// on exec, restores the caller's mask as its last step, then searches PATH as phase 1's
/// posix_spawnp did (measured on musl and glibc: no /bin/sh fallback; ENOENT, ENOTDIR, ESTALE,
/// ENODEV and ETIMEDOUT go on to the next entry; EACCES is remembered and goes on; any other error
/// stops; an empty entry is the working directory; an unset PATH is the libc's default). An exec
/// failure is written to the error pipe and ends it with 127 (ENOENT) or 126.
pub fn root_shim(a: &[OsString]) -> i32 {
    let arg = |i: usize| a.get(i).and_then(|s| s.to_str()).unwrap_or("");
    let (Ok(go), Ok(err), Ok(sup)) = (arg(2).parse::<i32>(), arg(3).parse::<i32>(), arg(4).parse::<i32>()) else { return 125 };
    let Some(mask) = unhex(arg(5)).filter(|m| m.len() == std::mem::size_of::<libc::sigset_t>()) else { return 125 };
    let nonce = arg(6).to_string();
    if arg(7) != "--" || a.len() < 9 {
        return 125;
    }
    let cmd = &a[8..];
    let tell = |s: &str| unsafe {
        libc::write(err, s.as_ptr() as *const libc::c_void, s.len());
    };
    let fail = |what: &str| -> ! {
        tell(&format!("F{what}\n"));
        unsafe { libc::_exit(125) }
    };
    let no_pdeath = crate::seam_flag("SHEEPDOG_TEST_SHIM_NO_PDEATHSIG");
    if crate::seam_flag("SHEEPDOG_TEST_SHIM_PRCTL_FAIL") || (!no_pdeath && unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0) } != 0) {
        fail("cannot set PR_SET_PDEATHSIG");
    }
    if crate::seam_flag("SHEEPDOG_TEST_SHIM_WRONG_PARENT") || unsafe { libc::getppid() } != sup {
        fail("its parent is not the supervisor");
    }
    tell(&format!("H{nonce} 1\n"));
    let mut b = [0u8; 1];
    loop {
        let n = unsafe { libc::read(go, b.as_mut_ptr() as *mut libc::c_void, 1) };
        if n == 1 {
            break;
        }
        if n < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
            continue;
        }
        unsafe { libc::_exit(125) }; // EOF: the supervisor is gone
    }
    unsafe {
        libc::close(go);
        libc::fcntl(err, libc::F_SETFD, libc::FD_CLOEXEC);
    }
    crate::seam_hold("SHEEPDOG_TEST_HOLD_SHIM");
    let argv = match cstrings(cmd) {
        Ok(v) => v,
        Err(_) => unsafe { libc::_exit(125) },
    };
    let mut ptrs: Vec<*const libc::c_char> = argv.iter().map(|c| c.as_ptr()).collect();
    ptrs.push(std::ptr::null());
    let name = cmd[0].as_bytes();
    let candidates: Vec<Vec<u8>> = if name.contains(&b'/') {
        vec![name.to_vec()]
    } else {
        let path = std::env::var_os("PATH").map(|p| p.into_vec()).unwrap_or_else(default_path);
        path.split(|&c| c == b':')
            .map(|dir| if dir.is_empty() { name.to_vec() } else { [dir, b"/", name].concat() })
            .collect()
    };
    let cands: Vec<CString> = candidates.into_iter().filter_map(|c| CString::new(c).ok()).collect();
    let mut eacces = false;
    let mut last = libc::ENOENT;
    unsafe {
        // the caller's mask, as the last step: a signal pending until now is delivered here
        let mut m: libc::sigset_t = std::mem::zeroed();
        std::ptr::copy_nonoverlapping(mask.as_ptr(), &mut m as *mut _ as *mut u8, mask.len());
        libc::sigprocmask(libc::SIG_SETMASK, &m, std::ptr::null_mut());
        for c in &cands {
            libc::execve(c.as_ptr(), ptrs.as_ptr(), environ as *const *const libc::c_char);
            let e = *libc::__errno_location();
            match e {
                libc::ENOENT | libc::ENOTDIR | libc::ESTALE | libc::ENODEV | libc::ETIMEDOUT => {}
                libc::EACCES => eacces = true,
                _ => {
                    last = e;
                    break;
                }
            }
            last = e;
        }
    }
    if eacces && matches!(last, libc::ENOENT | libc::ENOTDIR | libc::ESTALE | libc::ENODEV | libc::ETIMEDOUT) {
        last = libc::EACCES;
    }
    say!("sheepdog: cannot run {}: {}", cmd[0].to_string_lossy(), std::io::Error::from_raw_os_error(last));
    tell(&format!("E{last}\n"));
    unsafe { libc::_exit(if last == libc::ENOENT { 127 } else { 126 }) }
}

/// The libc's default search path for an unset PATH.
fn default_path() -> Vec<u8> {
    let mut buf = vec![0u8; 256];
    let n = unsafe { libc::confstr(libc::_CS_PATH, buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
    if n == 0 || n > buf.len() {
        return b"/bin:/usr/bin".to_vec();
    }
    buf.truncate(n - 1);
    buf
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
/// Returns Err(code) in the relay (or on a failed fork), Ok(Some(relay pid)) in a supervisor
/// that has a relay, Ok(None) without one.
fn relay_if_needed(sig: &crate::Signals) -> Result<Option<i32>, i32> {
    if !crate::has_children() {
        return Ok(None);
    }
    // A TERM that is pending now would stay in the relay (pending signals are not inherited
    // across fork) and reach the supervisor only later, possibly after it started the root.
    // So it ends sheepdog here, before any fork or root (S1 review, P2-1). Remaining window,
    // accepted: a TERM that reaches the relay after this check and before the fork is
    // forwarded, and if it reaches the supervisor after its own pre-spawn check, the root may
    // start and is then killed at once, never left running (the same as a TERM arriving just
    // after the pre-spawn check without a relay).
    if sig.watch_term && crate::term_pending() {
        return Err(crate::die_by_term(143));
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
                Ok(Some(relay))
            }
            -1 => {
                libc::sigprocmask(libc::SIG_SETMASK, &old, std::ptr::null_mut());
                say!("sheepdog: fork failed: {}", std::io::Error::last_os_error());
                Err(125)
            }
            sup => {
                // The relay keeps the stop signals blocked (it never stops on its own: a TSTP to
                // it alone stops nothing) and mirrors the supervisor instead: when the supervisor
                // has stopped the job and itself, the relay stops with the same signal, so the
                // shell, which waits on the relay, sees the job stopped exactly then (S5 review).
                // The supervisor's stop wakes it at once: SIGCHLD is on its signalfd.
                // the relay's own loop on a signalfd (PHASE1.md S1), created after the fork
                let fd = signal_fd(&set);
                crate::seam_sleep("SHEEPDOG_TEST_SLEEP_RELAY_BEFORE_FORWARD_MS");
                loop {
                    let mut st = 0;
                    if libc::waitpid(sup, &mut st, libc::WNOHANG | libc::WUNTRACED) == sup {
                        if libc::WIFSTOPPED(st) {
                            crate::seam_sleep("SHEEPDOG_TEST_SLEEP_RELAY_BEFORE_MIRROR_MS");
                            // a TERM (or HUP as leader) already pending is forwarded first, on the
                            // next pass, instead of stopping with it pending (TERM+CONT from
                            // `timeout` in this window would otherwise leave both stopped)
                            // (a TERM the caller ignored may still show as pending while blocked:
                            // only a watched one counts). Stated: a TERM+CONT that arrives in the
                            // instant between this check and the raise still leaves both stopped.
                            let hup = libc::getsid(0) == relay && crate::pending(libc::SIGHUP);
                            // and only while the supervisor is still stopped: continued meanwhile (and
                            // maybe already gone), there is nothing left to mirror (phase-1 review)
                            if !(sig.watch_term && crate::pending(libc::SIGTERM)) && !hup && stopped(sup) {
                                // debug seam: hold between that check and the raise (the supervisor
                                // can be continued here; the level-triggered continue then frees it)
                                crate::seam_sleep("SHEEPDOG_TEST_SLEEP_RELAY_BEFORE_RAISE_MS");
                                crate::self_stop(libc::WSTOPSIG(st));
                                // resumed: the supervisor too, if it is still stopped (a CONT to
                                // the relay's pid alone); never a CONT to a running supervisor,
                                // which could call off a new stop
                                if stopped(sup) {
                                    libc::kill(sup, libc::SIGCONT); // raw signal site: the relay's own child, the supervisor (PHASE2.md §0.3)
                                }
                            }
                            continue;
                        }
                        if fd >= 0 {
                            libc::close(fd);
                        }
                        return Err(crate::die_like(st));
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
                            // a stopped supervisor must wake to act on it (TERM+CONT to the relay,
                            // as `timeout` sends, must end a stopped job)
                            // the job is ending: continue the supervisor unconditionally (a stop it
                            // may be starting does not matter any more)
                            // only a TERM the caller did not ignore (Linux still delivers a blocked,
                            // ignored TERM on the signalfd; forwarding it with a CONT would call
                            // off a stop in progress)
                            libc::SIGTERM if sig.watch_term => {
                                libc::kill(sup, libc::SIGTERM); // raw signal site: the relay's own child, the supervisor (PHASE2.md §0.3)
                                libc::kill(sup, libc::SIGCONT); // raw signal site: the relay's own child, the supervisor (PHASE2.md §0.3)
                            }
                            libc::SIGHUP if libc::getsid(0) == relay => {
                                libc::kill(sup, libc::SIGHUP); // raw signal site: the relay's own child, the supervisor (PHASE2.md §0.3)
                                if stopped(sup) {
                                    libc::kill(sup, libc::SIGCONT); // raw signal site: the relay's own child, the supervisor (PHASE2.md §0.3)
                                }
                            }
                            // SIGCHLD, INT (never forwarded, also with --forward-int-to-root: a late
                            // copy would reach the escapees twice), a HUP we are not the leader for
                            _ => {}
                        }
                    }
                }
            }
        }
    }
}

pub fn run(a: &Args, sig: &crate::Signals) -> i32 {
    if let Some(why) = proc_problem() {
        say!("sheepdog: {why}, so sheepdog cannot tell which processes are this job's. Mount a /proc for this pid namespace (for example unshare --mount-proc). Nothing was started.");
        return 125;
    }
    // PHASE2.md §1 decision 7: an inherited PR_SET_PDEATHSIG (this run is another run's command)
    // becomes SIGTERM, so the outer's death is an orderly kill of this job, never a SIGKILL that
    // leaves it; a run that inherited none sets none
    unsafe {
        let mut cur: libc::c_int = 0;
        if libc::prctl(libc::PR_GET_PDEATHSIG, &mut cur as *mut libc::c_int, 0, 0, 0) == 0 && cur != 0 {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM, 0, 0, 0);
        }
    }
    let relay = match relay_if_needed(sig) {
        Ok(r) => r,
        Err(code) => return code,
    };
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
        crate::trace("degraded".into());
    }
    let me = unsafe { libc::getpid() };
    if let Some(code) = crate::term_before_spawn(sig) {
        return code;
    }
    let journal = std::cell::RefCell::new(crate::journal::Journal::open(&a.owner, &a.argv));
    let mut shim = match spawn(&a.cmd, &sig.caller_mask) {
        Ok(r) => r,
        Err(code) => {
            journal.into_inner().finish(true);
            return code;
        }
    };
    let root = shim.pid;
    crate::status::set_root("signaled"); // until the root's own end is known
    // the root is journaled before it runs: the shim waits for the go byte
    if let Some(id) = sheepdog::ident::identity(root) {
        journal.borrow_mut().record_root(root, id, &a.cmd);
    }
    crate::seam_hold("SHEEPDOG_TEST_HOLD_BEFORE_GO");
    shim.go();
    let scan = || {
        let found = descendants(me);
        journal.borrow_mut().record(&found);
        found
    };
    // The event loop's wait (PHASE1.md §1): a signalfd on CHLD and the watched signals (all
    // blocked by setup_signals), drained on every wake so each signal is consumed exactly once;
    // the waitpid(-1, WNOHANG) loop reaps the root and adopted orphans. One fixed order when
    // events coincide (round-7 P3-F4): TERM, then the root's exit, then INT/HUP. No signalfd:
    // poll every 50 ms (a blocked signal stays pending, so nothing is lost).
    let fd = signal_fd(&sig.wait_set());
    crate::seam_sleep("SHEEPDOG_TEST_SLEEP_AFTER_REGISTER_MS");
    let mut exited: Option<libc::c_int> = None;
    // membership while running (PLAN.md §3.2): a member seen on any tick is killed at the end
    // even if it is no longer a descendant by then (sticky, by identity)
    let mut tracker = crate::Tracker::default();
    let mut ints = crate::Interrupts::new(a, relay);
    let mut jobs = crate::JobControl::new(relay);
    let tick = crate::tick_ms() as i32;
    let status = loop {
        tracker.refresh(scan());
        let got: Vec<i32> = if fd >= 0 {
            drain(fd)
        } else {
            [
                (sig.watch_term, libc::SIGTERM),
                (sig.watch_int, libc::SIGINT),
                (sig.watch_hup, libc::SIGHUP),
                (sig.watch_quit, libc::SIGQUIT),
                (sig.watch_stop[0], libc::SIGTSTP),
                (sig.watch_stop[1], libc::SIGTTIN),
                (sig.watch_stop[2], libc::SIGTTOU),
                (sig.watch_cont, libc::SIGCONT),
            ]
            .into_iter()
                .filter(|&(on, s)| on && crate::consume(s))
                .map(|(_, s)| s)
                .collect()
        };
        loop {
            let mut st = 0;
            let r = unsafe { libc::waitpid(-1, &mut st, libc::WNOHANG) };
            if r == root && exited.is_none() {
                exited = Some(st);
            }
            if r <= 0 {
                break;
            }
        }
        if got.contains(&libc::SIGTERM) {
            break None;
        }
        if let Some(st) = exited {
            for s in [libc::SIGINT, libc::SIGHUP, libc::SIGQUIT] {
                if got.contains(&s) {
                    ints.note(s);
                }
            }
            break Some(st);
        }
        for s in [libc::SIGINT, libc::SIGHUP, libc::SIGQUIT] {
            if got.contains(&s) {
                ints.forward(s, root, &mut || {
                    tracker.refresh(scan());
                    tracker.known.iter().map(|(&p, &id)| (p, id)).collect()
                });
            }
        }
        if got.contains(&libc::SIGCONT) {
            jobs.continue_relay(); // continued on its own: the relay must not stay stopped
        }
        // job control after INT/HUP (the fixed order); all stop signals of one wake are one stop
        if let Some(&s) = crate::STOPS.iter().find(|s| got.contains(s)) {
            jobs.stop(s, sig, root, &mut || {
                tracker.refresh(scan());
                tracker.known.iter().map(|(&p, &id)| (p, id)).collect()
            }, stopped);
        }
        jobs.keep_relay_running(stopped);
        ints.tick(&mut |pg| crate::only_ours(&group_pids(pg), relay, &tracker.known));
        crate::seam_sleep("SHEEPDOG_TEST_SLEEP_BEFORE_WAIT_MS");
        if fd >= 0 {
            poll_fd(fd, tick);
        } else {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    };
    if fd >= 0 {
        unsafe { libc::close(fd) };
    }
    if status.is_some() && shim.exec_failed() {
        crate::status::set_root_final("not-started"); // the shim could not exec the command
    }
    if a.leave_strays && status.is_some() {
        let mut j = journal.into_inner();
        j.mark_leave_strays();
        j.finish(false);
        crate::release_relay(relay, stopped);
        return crate::finish(status, Ok(()), &mut ints, sig);
    }
    // ECHILD is authoritative only when every orphan comes back here (review round 3, F5)
    let opts = crate::KillOpts::from_env().with_grace(a.grace);
    let initial = tracker.known;
    let result = if is_subreaper {
        kill_tree(&opts, scan, reap, tree_empty, crate::signal, initial)
    } else {
        kill_tree(&opts, scan, reap, || None, crate::signal, initial)
    };
    reap();
    journal.into_inner().finish(result.is_ok());
    crate::release_relay(relay, stopped);
    crate::finish(status, result, &mut ints, sig)
}
