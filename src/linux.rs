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
                                    libc::kill(sup, libc::SIGCONT);
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
                                libc::kill(sup, libc::SIGTERM);
                                libc::kill(sup, libc::SIGCONT);
                            }
                            libc::SIGHUP if libc::getsid(0) == relay => {
                                libc::kill(sup, libc::SIGHUP);
                                if stopped(sup) {
                                    libc::kill(sup, libc::SIGCONT);
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
    let root = spawn(&a.cmd, &sig.caller_mask);
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
        tracker.refresh(descendants(me));
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
                    tracker.refresh(descendants(me));
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
                tracker.refresh(descendants(me));
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
    if a.leave_strays && status.is_some() {
        crate::release_relay(relay, stopped);
        return crate::finish(status, Ok(()), &mut ints, sig);
    }
    // ECHILD is authoritative only when every orphan comes back here (review round 3, F5)
    let opts = crate::KillOpts::from_env().with_grace(a.grace);
    let initial = tracker.known;
    let result = if is_subreaper {
        kill_tree(&opts, || descendants(me), reap, tree_empty, crate::signal, initial)
    } else {
        kill_tree(&opts, || descendants(me), reap, || None, crate::signal, initial)
    };
    reap();
    crate::release_relay(relay, stopped);
    crate::finish(status, result, &mut ints, sig)
}
