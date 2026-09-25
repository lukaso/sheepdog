//! macOS: responsibility-based membership (PLAN.md §2, §3.1, §3.2).

use crate::{code_of, cstrings, kill_tree, say, Args};
use std::io::Write;
use std::ffi::OsString;
use sheepdog::ident::identity;
use libc::{c_char, c_int, c_void, pid_t, posix_spawnattr_t};
use std::ffi::CString;
use std::mem::{size_of, zeroed};

const PROC_PIDUNIQIDENTIFIERINFO: c_int = 17;
const SZOMB: u32 = 5;
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
    if cfg!(debug_assertions) && std::env::var("SHEEPDOG_TEST_SPI").as_deref() == Ok("broken") {
        return None;
    }
    let f: RespUniq = sym("responsibility_get_uniqueid_responsible_for_pid")?;
    let v = unsafe { f(pid) };
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

fn all_pids() -> Vec<pid_t> {
    let n = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    let mut buf = vec![0 as pid_t; (n.max(0) as usize) * 2 + 64];
    let bytes = (buf.len() * size_of::<pid_t>()) as c_int;
    let got = unsafe { libc::proc_listallpids(buf.as_mut_ptr() as *mut c_void, bytes) };
    buf.truncate(got.max(0) as usize);
    buf
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
fn spawn(cmd: &[OsString], disclaim_root: bool, caller_mask: &libc::sigset_t) -> pid_t {
    let argv = cstrings(cmd).unwrap_or_else(|e| {
        say!("sheepdog: {e}");
        std::process::exit(125)
    });
    let mut ptrs: Vec<*mut c_char> = argv.iter().map(|c| c.as_ptr() as *mut c_char).collect();
    ptrs.push(std::ptr::null_mut());
    let mut pid: pid_t = 0;
    let rc = unsafe {
        let mut attr: posix_spawnattr_t = zeroed();
        libc::posix_spawnattr_init(&mut attr);
        // the root gets the caller's mask (sheepdog blocks TERM and SIGCHLD for itself)
        libc::posix_spawnattr_setsigmask(&mut attr, caller_mask);
        libc::posix_spawnattr_setflags(&mut attr, libc::POSIX_SPAWN_SETSIGMASK as i16);
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
        std::process::exit(if rc == libc::ENOENT { 127 } else { 126 });
    }
    pid
}

/// The event loop's wait (PHASE1.md §1): returns the root's exit code, or None if the job
/// must be ended because of TERM.
///
/// - kqueue watches the root's exit (EVFILT_PROC NOTE_EXIT) and TERM (EVFILT_SIGNAL). A
///   report is only a wake-up; TERM is consumed with `consume` (the SIG_IGN/SIG_DFL toggle),
///   never `sigwait`.
/// - After NOTE_EXIT the root is reaped with a BLOCKING waitpid: NOTE_EXIT can arrive before
///   the root is reapable, and the one-shot event is then gone (round-7 P3-F1).
/// - If registering NOTE_EXIT fails (ESRCH: the root is already exiting), the root is reaped
///   with a blocking waitpid (brief).
/// - One fixed order when events coincide (round-7 P3-F4): TERM, then the root's exit.
/// - Any other kqueue failure: poll every 50 ms. A blocked TERM stays pending, so polling
///   loses nothing; it never falls into a blocking wait that ignores TERM.
fn wait(pid: pid_t, watch_term: bool) -> Option<i32> {
    unsafe {
        let kq = libc::kqueue();
        let mut polling = kq < 0;
        // NOTE_EXIT refused with ESRCH: the root is already exiting (reap it with a blocking
        // wait). Any other registration failure: poll, never a wait that ignores TERM (S1
        // review, P3-b). Debug seam SHEEPDOG_TEST_KQ_EINVAL=1 forces an EINVAL failure of the
        // NOTE_EXIT registration only (TERM still registers: the hazardous combination).
        let mut proc_exiting = false;
        let force_einval = crate::seam_flag("SHEEPDOG_TEST_KQ_EINVAL");
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
                if force_einval && c.filter == libc::EVFILT_PROC {
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
                    polling = true;
                }
            }
            if watch_term && !polling {
                if let Err(e) = reg(&ch[1]) {
                    say!("sheepdog: cannot watch TERM (errno {e}); polling instead");
                    polling = true;
                }
            }
        }
        crate::seam_sleep("SHEEPDOG_TEST_SLEEP_AFTER_REGISTER_MS");
        let mut st = 0;
        let mut exited: Option<i32> = None;
        let result = loop {
            let term = watch_term && crate::consume(libc::SIGTERM);
            if exited.is_none() && libc::waitpid(pid, &mut st, libc::WNOHANG) == pid {
                exited = Some(code_of(st));
            }
            // the fixed order: TERM, then the root's exit
            if term {
                break None;
            }
            if let Some(code) = exited {
                break Some(code);
            }
            if !polling && proc_exiting {
                // NOTE_EXIT was refused with ESRCH: the root is exiting; reap it, then decide again
                exited = Some(if libc::waitpid(pid, &mut st, 0) == pid { code_of(st) } else { 125 });
                continue;
            }
            crate::seam_sleep("SHEEPDOG_TEST_SLEEP_BEFORE_WAIT_MS");
            if polling {
                std::thread::sleep(std::time::Duration::from_millis(50));
                continue;
            }
            let mut ev: libc::kevent = zeroed();
            let tick = libc::timespec { tv_sec: 0, tv_nsec: 250_000_000 };
            let r = libc::kevent(kq, std::ptr::null(), 0, &mut ev, 1, &tick);
            if r > 0 && ev.filter == libc::EVFILT_PROC {
                // P3-F1: the root has exited; reap it now (a blocking wait, bounded by its exit)
                exited = Some(if libc::waitpid(pid, &mut st, 0) == pid { code_of(st) } else { 125 });
            } else if r < 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
                say!("sheepdog: kqueue failed ({}); polling instead", std::io::Error::last_os_error());
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
            let ok = become_responsible(&a.argv, &sig.caller_mask);
            std::env::remove_var(REEXEC_MARK);
            if !ok {
                say!("sheepdog: the macOS responsibility API is not available; tracking is degraded");
            }
            let me = uniq(unsafe { libc::getpid() }).map(|u| u.0).unwrap_or(0);
            if let Some(code) = crate::term_before_spawn(sig) {
                return code;
            }
            let root = spawn(&a.cmd, false, &sig.caller_mask);
            let code = wait(root, sig.watch_term);
            let result = kill_tree(&crate::KillOpts::from_env(), || responsible_to(me), || {}, || None, crate::signal);
            if code.is_none() {
                let _ = unsafe { libc::waitpid(root, std::ptr::null_mut(), libc::WNOHANG) };
                if result.is_ok() {
                    return crate::die_by_term(143);
                }
            }
            match result {
                Ok(()) => code.unwrap_or(143),
                Err(e) => crate::kill_failed(e),
            }
        }
        Some("root-disclaim") => {
            if let Some(code) = crate::term_before_spawn(sig) {
                return code;
            }
            let root = spawn(&a.cmd, true, &sig.caller_mask);
            let r = uniq(root).map(|u| u.0).unwrap_or(0);
            let code = wait(root, sig.watch_term);
            let result = kill_tree(&crate::KillOpts::from_env(), || responsible_to(r), || {}, || None, crate::signal);
            if code.is_none() && result.is_ok() {
                return crate::die_by_term(143);
            }
            match result {
                Ok(()) => code.unwrap_or(143),
                Err(e) => crate::kill_failed(e),
            }
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

    #[test]
    fn a_dead_pid_has_no_responsibility_fact() {
        assert_eq!(resp_uniq(999_999), None);
    }
}
