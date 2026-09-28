//! macOS: responsibility-based membership (PLAN.md §2, §3.1, §3.2).

use crate::{cstrings, kill_tree, say, Args};
use std::io::Write;
use std::ffi::OsString;
use sheepdog::ident::identity;
use libc::{c_char, c_int, c_void, pid_t, posix_spawnattr_t};
use std::ffi::CString;
use std::mem::{size_of, zeroed};

const PROC_PIDUNIQIDENTIFIERINFO: c_int = 17;
const SZOMB: u32 = 5;
const SSTOP: u32 = 4;

/// Stopped by a signal?
pub fn stopped(pid: pid_t) -> bool {
    bsd(pid).is_some_and(|b| b.pbi_status == SSTOP)
}
/// Set only across the self re-exec; removed before the root starts, so a nested sheepdog
/// still disclaims.
const REEXEC_MARK: &str = "SHEEPDOG_REEXEC_PID";

#[repr(C)]
struct UniqInfo {
    uuid: [u8; 16],
    uniqueid: u64,
    puniqueid: u64,
    idversion: i32,
    orig_ppidversion: i32,
    reserve2: u64,
    reserve3: u64,
}

extern "C" {
    fn _NSGetEnviron() -> *mut *const *const c_char;
}

/// Returns the responsible process's uniqueid, or u64::MAX (-1) when there is no answer
/// (measured 2026-09-25: it is a return value, not an out-parameter).
type RespUniq = unsafe extern "C" fn(pid_t) -> u64;
type Disclaim = unsafe extern "C" fn(*mut posix_spawnattr_t, c_int) -> c_int;

fn sym<T: Copy>(name: &str) -> Option<T> {
    let c = CString::new(name).unwrap();
    let p = unsafe { libc::dlsym(libc::RTLD_DEFAULT, c.as_ptr()) };
    if p.is_null() {
        None
    } else {
        Some(unsafe { std::mem::transmute_copy::<*mut c_void, T>(&p) })
    }
}

/// This process's (uniqueid, original parent's uniqueid), or None (gone, zombie, not ours).
pub fn uniq(pid: pid_t) -> Option<(u64, u64)> {
    let mut u: UniqInfo = unsafe { zeroed() };
    let n = size_of::<UniqInfo>() as c_int;
    let r = unsafe {
        libc::proc_pidinfo(pid, PROC_PIDUNIQIDENTIFIERINFO, 0, &mut u as *mut _ as *mut c_void, n)
    };
    (r == n).then_some((u.uniqueid, u.puniqueid))
}

/// The uniqueid of the process responsible for `pid`; None means "no fact" (PLAN.md §3.2).
pub fn resp_uniq(pid: pid_t) -> Option<u64> {
    // Test seam (debug builds only): SHEEPDOG_TEST_SPI=broken makes the SPI answer "no fact".
    // Both are resolved once: the scan calls this for every process on every tick.
    static F: std::sync::OnceLock<Option<RespUniq>> = std::sync::OnceLock::new();
    let f = *F.get_or_init(|| {
        if cfg!(debug_assertions) && std::env::var("SHEEPDOG_TEST_SPI").as_deref() == Ok("broken") {
            return None;
        }
        sym("responsibility_get_uniqueid_responsible_for_pid")
    });
    let v = unsafe { f?(pid) };
    (v != 0 && v != u64::MAX).then_some(v)
}

fn bsd(pid: pid_t) -> Option<libc::proc_bsdinfo> {
    let mut b: libc::proc_bsdinfo = unsafe { zeroed() };
    let n = size_of::<libc::proc_bsdinfo>() as c_int;
    let r = unsafe {
        libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, &mut b as *mut _ as *mut c_void, n)
    };
    (r == n).then_some(b)
}

/// The pids in process group `pg`.
fn group_pids(pg: pid_t) -> Vec<pid_t> {
    const PROC_PGRP_ONLY: u32 = 2; // <sys/proc_info.h>
    let n = unsafe { libc::proc_listpids(PROC_PGRP_ONLY, pg as u32, std::ptr::null_mut(), 0) };
    let mut buf = vec![0 as pid_t; (n.max(0) as usize) / size_of::<pid_t>() + 64];
    let bytes = (buf.len() * size_of::<pid_t>()) as c_int;
    let got = unsafe { libc::proc_listpids(PROC_PGRP_ONLY, pg as u32, buf.as_mut_ptr() as *mut c_void, bytes) };
    buf.truncate((got.max(0) as usize) / size_of::<pid_t>());
    buf.retain(|&p| p > 0);
    buf
}

fn all_pids() -> Vec<pid_t> {
    let n = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    let mut buf = vec![0 as pid_t; (n.max(0) as usize) * 2 + 64];
    let bytes = (buf.len() * size_of::<pid_t>()) as c_int;
    let got = unsafe { libc::proc_listallpids(buf.as_mut_ptr() as *mut c_void, bytes) };
    buf.truncate(got.max(0) as usize);
    buf
}

struct Info {
    pid: pid_t,
    uniq: u64,
    puniq: u64,
    resp: Option<u64>,
}

/// The members of the job right now (PLAN.md §3.2, macOS), updating the tracker: a live,
/// same-uid process (not a zombie, not sheepdog) is a member if its responsible uniqueid is in
/// R, or its original parent's uniqueid (`puniq`) is one ever seen as a member. Iterated to a
/// fixed point within one scan, so a chain found in one scan counts at once. A member that is
/// responsible for itself (it re-disclaimed: an inner sheepdog, an app helper started inside
/// the job) joins R, so its own children are members through responsibility too.
pub fn members(t: &mut crate::Tracker) -> Vec<(pid_t, u64)> {
    let me = unsafe { libc::getpid() };
    let uid = unsafe { libc::getuid() };
    let infos: Vec<Info> = all_pids()
        .into_iter()
        .filter(|&p| p != me && p > 0)
        // bsd() fails for a zombie or a gone process, so it also filters those
        .filter(|&p| bsd(p).map_or(false, |b| b.pbi_uid == uid))
        .filter_map(|p| {
            let (u, pu) = uniq(p)?;
            Some(Info { pid: p, uniq: u, puniq: pu, resp: resp_uniq(p) })
        })
        .collect();
    loop {
        let mut changed = false;
        for i in &infos {
            let member = t.ever.contains(&i.uniq)
                || i.resp.map_or(false, |r| t.r.contains(&r))
                || t.ever.contains(&i.puniq);
            if member {
                changed |= t.ever.insert(i.uniq);
                if i.resp == Some(i.uniq) {
                    changed |= t.r.insert(i.uniq);
                }
            }
        }
        if !changed {
            break;
        }
    }
    infos.iter().filter(|i| t.ever.contains(&i.uniq)).map(|i| (i.pid, i.uniq)).collect()
}

/// `sheepdog kill`: an inner supervisor's escapees are tied to it only by responsibility, which
/// `kill` does not use as proof: if it does not end on the TERM (its caller ignores TERM), they
/// can survive, so `kill` reports that supervisor and exits 125.
pub const ADOPTS_ESCAPEES: bool = false;

/// Every live (not zombie) process, for `sheepdog kill` (S6).
pub fn procs() -> Vec<crate::kill::Proc> {
    all_pids()
        .into_iter()
        .filter(|&p| p > 0)
        .filter_map(|p| {
            let b = bsd(p)?; // fails for a zombie or a gone process
            let (id, pu) = uniq(p)?;
            Some(crate::kill::Proc { pid: p, ppid: b.pbi_ppid as i32, uid: b.pbi_uid, id, puniq: Some(pu) })
        })
        .collect()
}

/// The parent pid of `pid`, if it is alive, for a process of any uid: `kinfo_proc` from
/// sysctl KERN_PROC_PID, as `ps` reads it (PROC_PIDTBSDINFO is refused for another user's
/// process, and sheepdog's chain of ancestors passes through a root-owned `login` in a terminal).
/// The struct is not in the libc crate: 648 bytes, `kp_eproc.e_ppid` at offset 560 (measured
/// with the SDK's headers, 2026-09-27; a unit cell checks it against getppid and `ps`).
pub fn parent(pid: pid_t) -> Option<pid_t> {
    const SIZE: usize = 648;
    const E_PPID: usize = 560;
    let mut mib = [libc::CTL_KERN, libc::KERN_PROC, libc::KERN_PROC_PID, pid];
    let mut buf = [0u8; SIZE];
    let mut len = SIZE;
    let r = unsafe { libc::sysctl(mib.as_mut_ptr(), 4, buf.as_mut_ptr() as *mut c_void, &mut len, std::ptr::null_mut(), 0) };
    if r != 0 || len != SIZE {
        return None; // gone (the call succeeds with length 0)
    }
    Some(i32::from_ne_bytes(buf[E_PPID..E_PPID + 4].try_into().ok()?))
}

/// The file name of `pid`'s executable.
pub fn exe_name(pid: pid_t) -> Option<String> {
    let mut buf = vec![0u8; 4096];
    let n = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr() as *mut c_void, buf.len() as u32) };
    if n <= 0 {
        return None;
    }
    let s = String::from_utf8_lossy(&buf[..n as usize]).into_owned();
    Some(s.rsplit('/').next()?.to_string())
}

/// `pid`'s argv (empty if unreadable), from KERN_PROCARGS2: argc, the exec path, NUL padding,
/// then argv.
pub fn cmdline(pid: pid_t) -> Vec<String> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let mut buf = vec![0u8; 256 * 1024];
    let mut len = buf.len();
    let r = unsafe { libc::sysctl(mib.as_mut_ptr(), 3, buf.as_mut_ptr() as *mut c_void, &mut len, std::ptr::null_mut(), 0) };
    if r != 0 || len < 4 {
        return Vec::new();
    }
    let argc = i32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]).max(0) as usize;
    let rest = &buf[4..len];
    let Some(end) = rest.iter().position(|&c| c == 0) else { return Vec::new() };
    let rest = &rest[end..];
    let Some(start) = rest.iter().position(|&c| c != 0) else { return Vec::new() };
    rest[start..].split(|&c| c == 0).take(argc).map(|a| String::from_utf8_lossy(a).into_owned()).collect()
}

/// The pid of the process responsible for `pid` (PLAN.md §4.3: resolved with dlsym).
pub fn responsible_pid(pid: pid_t) -> Option<pid_t> {
    type RespPid = unsafe extern "C" fn(pid_t) -> pid_t;
    let f: RespPid = sym("responsibility_get_pid_responsible_for_pid")?;
    let r = unsafe { f(pid) };
    (r > 0).then_some(r)
}

/// Live, same-uid processes (not zombies) other than this one, whose responsible uniqueid is
/// `r`, with their identities.
fn responsible_to(r: u64) -> Vec<(pid_t, u64)> {
    let me = unsafe { libc::getpid() };
    let uid = unsafe { libc::getuid() };
    all_pids()
        .into_iter()
        .filter(|&p| p != me && p > 0)
        .filter(|&p| bsd(p).map_or(false, |b| b.pbi_uid == uid && b.pbi_status != SZOMB))
        .filter(|&p| resp_uniq(p) == Some(r))
        .filter_map(|p| Some((p, identity(p)?)))
        .collect()
}


/// PLAN.md §3.1: re-exec this process with SETEXEC + disclaim, so it becomes responsible for
/// itself and for everything it spawns. Returns false when the SPI is unavailable (fallback).
fn become_responsible(argv0: &[OsString], caller_mask: &libc::sigset_t) -> bool {
    let me = unsafe { libc::getpid() };
    let mine = match uniq(me) {
        Some((u, _)) => u,
        None => return false,
    };
    if resp_uniq(me) == Some(mine) {
        return true; // already re-exec'd
    }
    // At most one attempt (PLAN.md §3.1): the re-exec carries this pid in REEXEC_MARK. If the
    // mark is ours, the attempt already happened and did not take effect: fall back, never loop.
    if std::env::var(REEXEC_MARK).ok().as_deref() == Some(me.to_string().as_str()) {
        return false;
    }
    std::env::set_var(REEXEC_MARK, me.to_string());
    let disclaim: Disclaim = match sym("responsibility_spawnattrs_setdisclaim") {
        Some(f) => f,
        None => return false,
    };
    let mut path = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let n = unsafe { libc::proc_pidpath(me, path.as_mut_ptr() as *mut c_void, path.len() as u32) };
    if n <= 0 {
        return false;
    }
    path.truncate(n as usize);
    let path = CString::new(path).unwrap();
    let argv = match cstrings(argv0) {
        Ok(v) => v,
        Err(_) => return false,
    };
    let mut ptrs: Vec<*mut c_char> = argv.iter().map(|c| c.as_ptr() as *mut c_char).collect();
    ptrs.push(std::ptr::null_mut());
    crate::seam_sleep("SHEEPDOG_TEST_SLEEP_BEFORE_REEXEC_MS");
    unsafe {
        let mut attr: posix_spawnattr_t = zeroed();
        libc::posix_spawnattr_init(&mut attr);
        disclaim(&mut attr, 1);
        // the new image starts with the caller's mask: a TERM that is pending from before the
        // re-exec is delivered at once and ends sheepdog before any root exists (round 6, P2-1)
        libc::posix_spawnattr_setsigmask(&mut attr, caller_mask);
        libc::posix_spawnattr_setflags(&mut attr, (libc::POSIX_SPAWN_SETEXEC | libc::POSIX_SPAWN_SETSIGMASK) as i16);
        let env = *_NSGetEnviron() as *const *mut c_char;
        libc::posix_spawn(std::ptr::null_mut(), path.as_ptr(), std::ptr::null(), &attr, ptrs.as_ptr(), env);
    }
    false // only reached if the re-exec failed
}

/// Spawn the root. Its signal dispositions are the caller's (sheepdog changes none except
/// SIGCHLD, set to default: `#![no_main]`, see main.rs), and its mask is set to the caller's
/// with SETSIGMASK (sheepdog itself runs with TERM and SIGCHLD blocked). PLAN.md §3.1, cell 23.
/// Spawn the root with the caller's mask. `suspended`: it starts stopped before its first
/// instruction, so its identity can be read before it can start or leave anything.
fn spawn(cmd: &[OsString], disclaim_root: bool, suspended: bool, caller_mask: &libc::sigset_t) -> Result<pid_t, i32> {
    let argv = cstrings(cmd).map_err(|e| {
        say!("sheepdog: {e}");
        125
    })?;
    let mut ptrs: Vec<*mut c_char> = argv.iter().map(|c| c.as_ptr() as *mut c_char).collect();
    ptrs.push(std::ptr::null_mut());
    let mut pid: pid_t = 0;
    let rc = unsafe {
        let mut attr: posix_spawnattr_t = zeroed();
        libc::posix_spawnattr_init(&mut attr);
        // the root gets the caller's mask (sheepdog blocks TERM and SIGCHLD for itself)
        libc::posix_spawnattr_setsigmask(&mut attr, caller_mask);
        let mut flags = libc::POSIX_SPAWN_SETSIGMASK;
        if suspended {
            flags |= libc::POSIX_SPAWN_START_SUSPENDED;
        }
        libc::posix_spawnattr_setflags(&mut attr, flags as i16);
        if disclaim_root {
            if let Some(d) = sym::<Disclaim>("responsibility_spawnattrs_setdisclaim") {
                d(&mut attr, 1);
            }
        }
        let env = *_NSGetEnviron() as *const *mut c_char;
        libc::posix_spawnp(&mut pid, ptrs[0], std::ptr::null(), &attr, ptrs.as_ptr(), env)
    };
    if rc != 0 {
        say!("sheepdog: cannot run {}: {}", cmd[0].to_string_lossy(), std::io::Error::from_raw_os_error(rc));
        return Err(if rc == libc::ENOENT { 127 } else { 126 });
    }
    Ok(pid)
}

/// The macOS `puniq` of `pid` (its original parent's uniqueid), for the journal.
pub fn puniq(pid: pid_t) -> Option<u64> {
    uniq(pid).map(|u| u.1)
}

/// Env var that carries `<relay pid>:<supervisor pid>` across the supervisor's re-exec; removed
/// before the root is spawned. Honoured only when the supervisor pid is this process, so a value
/// inherited from anywhere else (a relayed sheepdog's job, a stale environment) is ignored.
const RELAY_PID: &str = "SHEEPDOG_RELAY_PID";

/// The relay this process was forked by, if the environment says so for this very process.
fn my_relay() -> Option<pid_t> {
    let v = std::env::var(RELAY_PID).ok()?;
    let (relay, sup) = v.split_once(':')?;
    (sup.parse::<pid_t>().ok()? == unsafe { libc::getpid() }).then(|| relay.parse().ok()).flatten()
}

/// If sheepdog starts with a history, it forks once, first: the child is a fresh supervisor,
/// and this process becomes the relay. The reason on macOS is identity: a shell's
/// `bg & exec sheepdog` keeps the shell's uniqueid, so what the caller started earlier refers to
/// that uniqueid. Two ways (S2 review, A-P2-1 and round 2):
/// - it has children: they are children of (`puniq`) that uniqueid;
/// - it is already responsible for itself (the caller was a launchd job or an app helper, and
///   this is not sheepdog's own re-exec): everything the caller started, orphans included, is
///   responsible to that uniqueid, which would be R.
/// The fresh supervisor has a uniqueid nothing else refers to. Returns Some(code) in the relay,
/// None in the supervisor.
///
/// The relay, as on Linux (PLAN.md §3.1): forwards TERM; forwards HUP only when it is the
/// session leader; never forwards INT (a terminal INT reaches the whole group); dies the way
/// the supervisor died. macOS has no PDEATHSIG, so the supervisor watches the relay itself and
/// raises TERM when it dies (`relay_gone`).
fn relay_if_needed(sig: &crate::Signals) -> Option<i32> {
    if my_relay().is_some() {
        return None; // the supervisor a relay forked, after its re-exec
    }
    let me = unsafe { libc::getpid() };
    let own_reexec = std::env::var(REEXEC_MARK).ok().as_deref() == Some(me.to_string().as_str());
    let self_responsible = !own_reexec && uniq(me).map(|u| u.0).is_some_and(|u| resp_uniq(me) == Some(u) && referred_to(me, u));
    if !crate::has_children() && !self_responsible {
        return None;
    }
    // a TERM pending now would stay in the relay (pending signals are not inherited across fork)
    if sig.watch_term && crate::term_pending() {
        return Some(crate::die_by_term(143));
    }
    unsafe {
        let relay = libc::getpid();
        let mut set: libc::sigset_t = zeroed();
        libc::sigemptyset(&mut set);
        for s in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP, libc::SIGCHLD] {
            libc::sigaddset(&mut set, s);
        }
        let mut old: libc::sigset_t = zeroed();
        libc::sigprocmask(libc::SIG_BLOCK, &set, &mut old);
        match libc::fork() {
            0 => {
                std::env::set_var(RELAY_PID, format!("{relay}:{}", libc::getpid()));
                libc::sigprocmask(libc::SIG_SETMASK, &old, std::ptr::null_mut());
                None
            }
            -1 => {
                libc::sigprocmask(libc::SIG_SETMASK, &old, std::ptr::null_mut());
                say!("sheepdog: fork failed: {}", std::io::Error::last_os_error());
                Some(125)
            }
            sup => {
                crate::status::disable(); // the supervisor writes the status line
                Some(relay_loop(sup, relay, sig.watch_term))
            }
        }
    }
}

/// Is any live process other than `me` responsible to uniqueid `u`? A self-responsible
/// sheepdog with no such history needs no relay (a fresh launchd job, S2 review round 3):
/// relaying would only cost the pid-only INT. (A live process whose original parent is `me` is
/// a child of `me`, which `has_children` already covers.)
fn referred_to(me: pid_t, u: u64) -> bool {
    all_pids().into_iter().filter(|&p| p != me && p > 0).any(|p| resp_uniq(p) == Some(u))
}

/// Has the relay `r` already exited? Its NOTE_EXIT is refused with ESRCH (also for a zombie).
fn relay_exited(r: pid_t) -> bool {
    unsafe {
        let kq = libc::kqueue();
        if kq < 0 {
            return false;
        }
        let mut ev: libc::kevent = zeroed();
        ev.ident = r as usize;
        ev.filter = libc::EVFILT_PROC;
        ev.flags = libc::EV_ADD;
        ev.fflags = libc::NOTE_EXIT;
        let gone = libc::kevent(kq, &ev, 1, std::ptr::null_mut(), 0, std::ptr::null()) != 0
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
        libc::close(kq);
        gone
    }
}

/// The relay's loop: a kqueue on the supervisor's exit and on TERM/HUP/INT (all blocked; a
/// kqueue signal event is only a wake-up, the signal is taken with `consume`).
fn relay_loop(sup: pid_t, relay: pid_t, watch_term: bool) -> i32 {
    unsafe {
        let kq = libc::kqueue();
        if kq >= 0 {
            let mut ev: libc::kevent = zeroed();
            ev.ident = sup as usize;
            ev.filter = libc::EVFILT_PROC;
            ev.flags = libc::EV_ADD;
            ev.fflags = libc::NOTE_EXIT;
            libc::kevent(kq, &ev, 1, std::ptr::null_mut(), 0, std::ptr::null());
            for s in [libc::SIGTERM, libc::SIGHUP, libc::SIGINT] {
                let mut ev: libc::kevent = zeroed();
                ev.ident = s as usize;
                ev.filter = libc::EVFILT_SIGNAL;
                ev.flags = libc::EV_ADD;
                libc::kevent(kq, &ev, 1, std::ptr::null_mut(), 0, std::ptr::null());
            }
        }
        crate::seam_sleep("SHEEPDOG_TEST_SLEEP_RELAY_BEFORE_FORWARD_MS");
        loop {
            let mut st = 0;
            // mirror the supervisor's stop (it stopped the job and itself): the shell waits on
            // the relay; the relay never stops on its own (its stop signals stay blocked). Seen
            // at the next pass, within a tick.
            // (one waitpid: WUNTRACED also reaps an exited supervisor, so a second call would
            // find nothing and the relay would never see the exit)
            if libc::waitpid(sup, &mut st, libc::WNOHANG | libc::WUNTRACED) == sup {
                if libc::WIFSTOPPED(st) {
                    crate::seam_sleep("SHEEPDOG_TEST_SLEEP_RELAY_BEFORE_MIRROR_MS");
                    // a TERM (or HUP as leader) already pending is forwarded first, on the next
                    // pass, instead of stopping with it pending (TERM+CONT from `timeout` in this
                    // window would otherwise leave both stopped)
                    // (only a watched TERM counts). Stated: a TERM+CONT that arrives in the instant
                    // between this check and the raise still leaves both stopped.
                    let hup = libc::getsid(0) == relay && crate::pending(libc::SIGHUP);
                    // and only while the supervisor is still stopped: continued meanwhile (and maybe
                    // already gone), there is nothing left to mirror (phase-1 review)
                    if !(watch_term && crate::pending(libc::SIGTERM)) && !hup && stopped(sup) {
                        // debug seam: hold between that check and the raise (the supervisor can be
                        // continued here; the level-triggered continue then frees the relay)
                        crate::seam_sleep("SHEEPDOG_TEST_SLEEP_RELAY_BEFORE_RAISE_MS");
                        crate::self_stop(libc::WSTOPSIG(st));
                        // resumed: the supervisor too, if it is still stopped (a CONT to the
                        // relay's pid alone); never a CONT to a running supervisor, which could
                        // call off a new stop
                        if stopped(sup) {
                            libc::kill(sup, libc::SIGCONT); // raw signal site: the relay's own child, the supervisor (PHASE2.md §0.3)
                        }
                    }
                    continue;
                }
                if kq >= 0 {
                    libc::close(kq);
                }
                return crate::die_like(st);
            }
            // a stopped supervisor must wake to act on it (TERM+CONT to the relay, as `timeout`
            // sends, must end a stopped job)
            // the job is ending: continue the supervisor unconditionally
            // only a TERM the caller did not ignore
            if watch_term && crate::consume(libc::SIGTERM) {
                libc::kill(sup, libc::SIGTERM); // raw signal site: the relay's own child, the supervisor (PHASE2.md §0.3)
                libc::kill(sup, libc::SIGCONT); // raw signal site: the relay's own child, the supervisor (PHASE2.md §0.3)
            }
            if crate::consume(libc::SIGHUP) && libc::getsid(0) == relay {
                libc::kill(sup, libc::SIGHUP); // raw signal site: the relay's own child, the supervisor (PHASE2.md §0.3)
                if stopped(sup) {
                    libc::kill(sup, libc::SIGCONT); // raw signal site: the relay's own child, the supervisor (PHASE2.md §0.3)
                }
            }
            // never forwarded: the terminal sent it to the group. Not with --forward-int-to-root
            // either: a copy that reaches the supervisor after it took the terminal's INT would
            // be forwarded to the escapees a second time (S4 review round 2)
            crate::consume(libc::SIGINT);
            if kq >= 0 {
                // a registration that failed only costs this bounded wait
                let tick = libc::timespec { tv_sec: 0, tv_nsec: 250_000_000 };
                let mut ev: libc::kevent = zeroed();
                libc::kevent(kq, std::ptr::null(), 0, &mut ev, 1, &tick);
            } else {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    }
}

/// The supervisor under a relay: the relay has died, so raise TERM, as Linux's
/// PR_SET_PDEATHSIG does; the TERM paths then end the job. The death is seen as the relay's
/// NOTE_EXIT (or ESRCH when registering it). The parent pid is a fallback only when kqueue is
/// unusable: a debugger's attach also changes it.
fn relay_died() {
    unsafe { libc::raise(libc::SIGTERM) }; // raw signal site: this process (PHASE2.md §0.3)
}

/// The event loop's wait (PHASE1.md §1): returns the root's wait status, or None if the job
/// must be ended because of TERM. INT and HUP are forwarded (`Interrupts`), never an end.
///
/// - kqueue watches the root's exit (EVFILT_PROC NOTE_EXIT) and TERM (EVFILT_SIGNAL). A
///   report is only a wake-up; TERM is consumed with `consume` (the SIG_IGN/SIG_DFL toggle),
///   never `sigwait`.
/// - After NOTE_EXIT the root is reaped with a BLOCKING waitpid: NOTE_EXIT can arrive before
///   the root is reapable, and the one-shot event is then gone (round-7 P3-F1).
/// - If registering NOTE_EXIT fails (ESRCH: the root is already exiting), the root is reaped
///   with a blocking waitpid (brief).
/// - One fixed order when events coincide (round-7 P3-F4): TERM, then the root's exit, then
///   INT/HUP (each consumed with the toggle, so it is forwarded once, not at every wake).
/// - Any other kqueue failure: poll every 50 ms. A blocked TERM stays pending, so polling
///   loses nothing; it never falls into a blocking wait that ignores TERM.
fn wait(
    pid: pid_t,
    sig: &crate::Signals,
    relay: Option<pid_t>,
    members: &mut dyn FnMut() -> Vec<(pid_t, u64)>,
    group_is_ours: &mut dyn FnMut(i32) -> bool,
    ints: &mut crate::Interrupts,
) -> Option<c_int> {
    let mut jobs = crate::JobControl::new(relay);
    let watch_term = sig.watch_term;
    unsafe {
        let kq = libc::kqueue();
        let mut polling = kq < 0;
        // NOTE_EXIT refused with ESRCH: the root is already exiting (reap it with a blocking
        // wait). Any other registration failure: poll, never a wait that ignores TERM (S1
        // review, P3-b). Debug seam SHEEPDOG_TEST_KQ_EINVAL=1 forces an EINVAL failure of the
        // NOTE_EXIT registration only (TERM still registers: the hazardous combination).
        let mut proc_exiting = false;
        let mut relay_by_ppid = polling && relay.is_some();
        // the scan tick, 250 ms; debug seam SHEEPDOG_TEST_TICK_MS (under 1 s) widens it, so a
        // cell can tell "woke at once" from "woke at the next tick" by a wide margin
        let tick_ns = crate::tick_ms() as i64 * 1_000_000;
        let force_einval = crate::seam_flag("SHEEPDOG_TEST_KQ_EINVAL");
        // debug seam: fail only the TERM registration (S1 fix review, P3-2)
        let force_sig_einval = crate::seam_flag("SHEEPDOG_TEST_KQ_SIG_EINVAL");
        if !polling {
            let mut ch: [libc::kevent; 2] = zeroed();
            ch[0].ident = pid as usize;
            ch[0].filter = libc::EVFILT_PROC;
            ch[0].flags = libc::EV_ADD;
            ch[0].fflags = libc::NOTE_EXIT;
            ch[1].ident = libc::SIGTERM as usize;
            ch[1].filter = libc::EVFILT_SIGNAL;
            ch[1].flags = libc::EV_ADD;
            let reg = |c: &libc::kevent| -> Result<(), i32> {
                if (force_einval && c.filter == libc::EVFILT_PROC) || (force_sig_einval && c.filter == libc::EVFILT_SIGNAL) {
                    return Err(libc::EINVAL);
                }
                if libc::kevent(kq, c, 1, std::ptr::null_mut(), 0, std::ptr::null()) == 0 {
                    Ok(())
                } else {
                    Err(std::io::Error::last_os_error().raw_os_error().unwrap_or(0))
                }
            };
            match reg(&ch[0]) {
                Ok(()) => {}
                Err(e) if e == libc::ESRCH => proc_exiting = true,
                Err(e) => {
                    say!("sheepdog: cannot watch the command's exit (errno {e}); polling instead");
                    crate::trace("polling exit".into());
                    polling = true;
                }
            }
            if watch_term && !polling {
                if let Err(e) = reg(&ch[1]) {
                    say!("sheepdog: cannot watch TERM (errno {e}); polling instead");
                    crate::trace("polling term".into());
                    polling = true;
                }
            }
            let others = [
                (sig.watch_int, libc::SIGINT),
                (sig.watch_hup, libc::SIGHUP),
                (sig.watch_quit, libc::SIGQUIT),
                (sig.watch_stop[0], libc::SIGTSTP),
                (sig.watch_stop[1], libc::SIGTTIN),
                (sig.watch_stop[2], libc::SIGTTOU),
            ];
            for (on, s) in others {
                if on && !polling {
                    let mut ev: libc::kevent = zeroed();
                    ev.ident = s as usize;
                    ev.filter = libc::EVFILT_SIGNAL;
                    ev.flags = libc::EV_ADD;
                    if let Err(e) = reg(&ev) {
                        say!("sheepdog: cannot watch signal {s} (errno {e}); polling instead");
                        crate::trace(format!("polling signal {s}"));
                        polling = true;
                    }
                }
            }
            // the relay's exit: ESRCH means it is already gone; any other failure falls back to
            // the parent-pid check on each tick
            if let Some(r) = relay {
                crate::seam_sleep("SHEEPDOG_TEST_SLEEP_BEFORE_RELAY_REG_MS");
                let mut ev: libc::kevent = zeroed();
                ev.ident = r as usize;
                ev.filter = libc::EVFILT_PROC;
                ev.flags = libc::EV_ADD | libc::EV_ONESHOT;
                ev.fflags = libc::NOTE_EXIT;
                let reg = libc::kevent(kq, &ev, 1, std::ptr::null_mut(), 0, std::ptr::null());
                let reg_errno = std::io::Error::last_os_error().raw_os_error(); // before the seam can change it
                if cfg!(debug_assertions) {
                    // debug seam: mark that the relay's exit is now registered (or refused)
                    if let Ok(f) = std::env::var("SHEEPDOG_TEST_RELAY_REG_FILE") {
                        let _ = std::fs::File::create(f);
                    }
                }
                if reg != 0 {
                    if reg_errno == Some(libc::ESRCH) {
                        relay_died();
                    } else {
                        relay_by_ppid = true;
                    }
                }
            }
        }
        crate::seam_sleep("SHEEPDOG_TEST_SLEEP_AFTER_REGISTER_MS");
        let mut st = 0;
        let mut exited: Option<i32> = None;
        let result = loop {
            // polling never reads kqueue events, so it checks the parent pid instead
            if (relay_by_ppid || polling) && relay.is_some_and(|r| libc::getppid() != r) {
                relay_died();
                relay_by_ppid = false;
            }
            let term = watch_term && crate::consume(libc::SIGTERM);
            let int = sig.watch_int && crate::consume(libc::SIGINT);
            let hup = sig.watch_hup && crate::consume(libc::SIGHUP);
            let quit = sig.watch_quit && crate::consume(libc::SIGQUIT);
            // all stop signals of one wake are one stop; the first one is raised on sheepdog
            let mut stop = None;
            for (i, &s) in crate::STOPS.iter().enumerate() {
                if sig.watch_stop[i] && crate::consume(s) && stop.is_none() {
                    stop = Some(s);
                }
            }
            if sig.watch_cont && crate::consume(libc::SIGCONT) {
                jobs.continue_relay(); // continued on its own: the relay must not stay stopped
            }
            if exited.is_none() && libc::waitpid(pid, &mut st, libc::WNOHANG) == pid {
                exited = Some(st);
            }
            // the fixed order: TERM, then the root's exit, then INT/HUP
            if term {
                break None;
            }
            if let Some(status) = exited {
                for (got, s) in [(int, libc::SIGINT), (hup, libc::SIGHUP), (quit, libc::SIGQUIT)] {
                    if got {
                        ints.note(s);
                    }
                }
                break Some(status);
            }
            for (got, s) in [(int, libc::SIGINT), (hup, libc::SIGHUP), (quit, libc::SIGQUIT)] {
                if got {
                    ints.forward(s, pid, members);
                }
            }
            // job control after INT/HUP (the fixed order)
            if let Some(s) = stop {
                jobs.stop(s, sig, pid, members, stopped);
            }
            jobs.keep_relay_running(stopped);
            ints.tick(group_is_ours);
            if !polling && proc_exiting {
                // NOTE_EXIT was refused with ESRCH: the root is exiting; reap it, then decide again
                exited = Some(if libc::waitpid(pid, &mut st, 0) == pid { st } else { crate::exit_status(125) });
                continue;
            }
            crate::seam_sleep("SHEEPDOG_TEST_SLEEP_BEFORE_WAIT_MS");
            if polling {
                std::thread::sleep(std::time::Duration::from_millis(50));
                let _ = members();
                continue;
            }
            let mut ev: libc::kevent = zeroed();
            let tick = libc::timespec { tv_sec: 0, tv_nsec: tick_ns };
            let r = libc::kevent(kq, std::ptr::null(), 0, &mut ev, 1, &tick);
            // the error now: the membership scan below makes calls of its own that replace errno
            // (an EINTR from a STOP and CONT was then read as a failure; phase-1 review)
            let err = std::io::Error::last_os_error();
            let _ = members(); // membership while running (a tick or an event)
            if r > 0 && ev.filter == libc::EVFILT_PROC && relay.is_some_and(|x| ev.ident == x as usize) {
                relay_died(); // taken as TERM on the next pass
            } else if r > 0 && ev.filter == libc::EVFILT_PROC && ev.ident == pid as usize {
                // P3-F1: the root has exited; reap it now (a blocking wait, bounded by its exit).
                // Only the root's event: the relay's exit is also an EVFILT_PROC event, and a
                // blocking wait for a root that still runs would hang the loop.
                exited = Some(if libc::waitpid(pid, &mut st, 0) == pid { st } else { crate::exit_status(125) });
            } else if r < 0 && err.raw_os_error() != Some(libc::EINTR) {
                say!("sheepdog: kqueue failed ({err}); polling instead");
                crate::trace("polling".into());
                polling = true;
            }
        };
        if kq >= 0 {
            libc::close(kq);
        }
        result
    }
}

pub fn run(a: &Args, sig: &crate::Signals) -> i32 {
    match a.mode.as_deref() {
        None | Some("responsible") => {
            if let Some(code) = relay_if_needed(sig) {
                return code;
            }
            let ok = become_responsible(&a.argv, &sig.caller_mask);
            std::env::remove_var(REEXEC_MARK);
            let relay = my_relay();
            std::env::remove_var(RELAY_PID);
            // a relay that is already gone means the job must not start, whatever the caller did
            // with TERM (S2 review rounds 3-4)
            if relay.is_some_and(relay_exited) {
                return crate::die_by_term(143);
            }
            crate::status::cloexec(); // after the SETEXEC, which the fd had to survive
            crate::status::set_tracking(if ok { "responsibility" } else { "puniq" });
            if !ok {
                say!("sheepdog: the macOS responsibility API is not available; tracking is degraded");
                crate::trace("degraded".into());
                crate::status::set_degraded("the macOS responsibility API is not available");
            }
            let me = uniq(unsafe { libc::getpid() }).map(|u| u.0).unwrap_or(0);
            if let Some(code) = crate::term_before_spawn(sig) {
                return code;
            }
            // after the SETEXEC (the lock is CLOEXEC and would not survive it), before the root
            let journal = std::cell::RefCell::new(crate::journal::Journal::open(&a.owner, &a.argv));
            let tracker = std::cell::RefCell::new(crate::Tracker::default());
            tracker.borrow_mut().r.insert(me);
            tracker.borrow_mut().ever.insert(me);
            // the root's identity is known from its birth: a child it starts with the disclaim
            // has only `puniq` = the root as its fact, even if the root exits before any scan
            // Suspended, unless the caller blocks SIGCONT: then the resuming CONT would stay
            // pending in the root, a signal its caller never sent. That case keeps the race of
            // a root that exits before its identity is read (PLAN.md §3.2).
            let cont_blocked = unsafe { libc::sigismember(&sig.caller_mask, libc::SIGCONT) } == 1;
            // Every signal is held from the spawn until the root's identity is read, so nothing
            // can end or stop sheepdog in between (a stop and the job's CONT would resume the
            // root before sheepdog knows it). Then the stop signals are let through: a ctrl-Z stops
            // sheepdog before the CONT, and the job's CONT resumes both. Signals that end sheepdog
            // stay held until the CONT, so the root is never left stopped; they act after it.
            let mut held: libc::sigset_t = unsafe { zeroed() };
            let mut stops: libc::sigset_t = unsafe { zeroed() };
            unsafe {
                let mut all: libc::sigset_t = zeroed();
                libc::sigfillset(&mut all);
                libc::sigprocmask(libc::SIG_BLOCK, &all, &mut held);
                libc::sigemptyset(&mut stops);
                // the caller's mask decides (sheepdog itself blocks the stop signals it watches
                // for the event loop, so `held` has them all)
                for s in [libc::SIGTSTP, libc::SIGTTIN, libc::SIGTTOU] {
                    if libc::sigismember(&sig.caller_mask, s) != 1 {
                        libc::sigaddset(&mut stops, s);
                    }
                }
            }
            let root = match spawn(&a.cmd, false, !cont_blocked, &sig.caller_mask) {
                Ok(r) => r,
                Err(code) => {
                    unsafe { libc::sigprocmask(libc::SIG_SETMASK, &held, std::ptr::null_mut()) };
                    journal.into_inner().finish(true);
                    return code;
                }
            };
            crate::status::set_root_pid(root);
            crate::status::set_root("signaled"); // until the root's own end is known
            crate::seam_sleep("SHEEPDOG_TEST_SLEEP_AFTER_SPAWN_MS");
            if let Some((u, _)) = uniq(root) {
                tracker.borrow_mut().ever.insert(u);
                // before the resuming CONT below: the root is journaled before it runs
                journal.borrow_mut().record_root(root, u, &a.cmd);
            }
            unsafe { libc::sigprocmask(libc::SIG_UNBLOCK, &stops, std::ptr::null_mut()) };
            crate::seam_sleep("SHEEPDOG_TEST_SLEEP_BEFORE_CONT_CHECK_MS");
            let mut ended = false;
            if !cont_blocked {
                // The job may have ended while the root was suspended (its relay died, or TERM
                // came): the root is killed and never resumed by sheepdog. A stop and the job's
                // CONT may still have run it meanwhile, so the kill below is the whole tree's
                // (S2 review rounds 4-5).
                if relay.is_some_and(relay_exited) || (sig.watch_term && crate::term_pending()) {
                    unsafe {
                        libc::kill(root, libc::SIGKILL); // raw signal site: the root this supervisor spawned (PHASE2.md §0.3)
                        let mut st = 0;
                        libc::waitpid(root, &mut st, 0);
                    }
                    crate::status::set_root("not-started");
                    ended = true;
                } else {
                    if cfg!(debug_assertions) {
                        // debug seam: mark the moment the root is resumed, so a test can prove
                        // its action came before it
                        if let Ok(f) = std::env::var("SHEEPDOG_TEST_CONT_FILE") {
                            let _ = std::fs::File::create(f);
                        }
                    }
                    unsafe { libc::kill(root, libc::SIGCONT) }; // raw signal site: the root this supervisor spawned (PHASE2.md §0.3)
                }
            }
            // On an early end the signals that end sheepdog stay held through the kill and the
            // death by TERM (the stop signals were let through before the check, and a ctrl-Z can
            // still stop it): releasing them now would let another deadly one (INT, HUP, QUIT)
            // end sheepdog before the tree is killed (S2 review round 6).
            if !ended {
                unsafe { libc::sigprocmask(libc::SIG_SETMASK, &held, std::ptr::null_mut()) };
            }
            let mut current = || {
                let mut t = tracker.borrow_mut();
                let found = members(&mut t);
                journal.borrow_mut().record(&found);
                t.refresh(found);
                t.known.iter().map(|(&p, &id)| (p, id)).collect()
            };
            let mut ints = crate::Interrupts::new(a, relay);
            let mut ours = |pg: i32| crate::only_ours(&group_pids(pg), relay, &tracker.borrow().known);
            let status = if ended { None } else { wait(root, sig, relay, &mut current, &mut ours, &mut ints) };
            if a.leave_strays && status.is_some() {
                let mut j = journal.into_inner();
                let marked = j.mark_leave_strays();
                j.finish(!marked);
                crate::release_relay(relay, stopped);
                return crate::finish(status, Ok(()), &mut ints, sig);
            }
            let initial = tracker.borrow().known.clone();
            let result = kill_tree(
                &crate::KillOpts::from_env().with_grace(a.grace),
                || {
                    let mut t = tracker.borrow_mut();
                    let found = members(&mut t);
                    journal.borrow_mut().record(&found);
                    found
                },
                || {},
                || None,
                crate::signal,
                initial,
            );
            if status.is_none() {
                let _ = unsafe { libc::waitpid(root, std::ptr::null_mut(), libc::WNOHANG) };
            }
            journal.into_inner().finish(result.is_ok());
            crate::release_relay(relay, stopped);
            crate::finish(status, result, &mut ints, sig)
        }
        Some("root-disclaim") => {
            if let Some(code) = crate::term_before_spawn(sig) {
                return code;
            }
            let root = match spawn(&a.cmd, true, false, &sig.caller_mask) {
                Ok(r) => r,
                Err(code) => return code,
            };
            let r = uniq(root).map(|u| u.0).unwrap_or(0);
            let mut ints = crate::Interrupts::none();
            let status = wait(root, sig, None, &mut Vec::new, &mut |_| false, &mut ints);
            let result = kill_tree(&crate::KillOpts::from_env(), || responsible_to(r), || {}, || None, crate::signal, Default::default());
            crate::finish(status, result, &mut ints, sig)
        }
        Some(m) => {
            say!("sheepdog: unknown mode {m}");
            125
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_responsibility_spi_answers_for_this_process() {
        assert!(resp_uniq(unsafe { libc::getpid() }).is_some());
    }

    /// `parent` reads `kinfo_proc` at a measured offset: it must agree with getppid for this
    /// process, and with `ps` for another user's process (a root-owned one).
    #[test]
    fn parent_matches_getppid_and_ps() {
        assert_eq!(super::parent(unsafe { libc::getpid() }), Some(unsafe { libc::getppid() }));
        let out = std::process::Command::new("ps").args(["-Ao", "pid=,ppid=,uid="]).output().unwrap();
        let rows: Vec<(i32, i32, u32)> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| {
                let w: Vec<&str> = l.split_whitespace().collect();
                Some((w.first()?.parse().ok()?, w.get(1)?.parse().ok()?, w.get(2)?.parse().ok()?))
            })
            .collect();
        let me = unsafe { libc::getuid() };
        let (p, pp, _) = *rows.iter().find(|&&(p, _, u)| p > 1 && u != me && super::parent(p).is_some()).expect("no other user's process");
        assert_eq!(super::parent(p), Some(pp));
        assert_eq!(super::parent(999_999), None);
    }

    #[test]
    fn a_dead_pid_has_no_responsibility_fact() {
        assert_eq!(resp_uniq(999_999), None);
    }
}
