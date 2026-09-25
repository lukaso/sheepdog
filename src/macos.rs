//! macOS: responsibility-based membership (PLAN.md §2, §3.1, §3.2).

use crate::{code_of, kill_tree, Args};
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

/// Live, same-uid processes (not zombies) other than this one, whose responsible uniqueid is `r`.
fn responsible_to(r: u64) -> Vec<pid_t> {
    let me = unsafe { libc::getpid() };
    let uid = unsafe { libc::getuid() };
    all_pids()
        .into_iter()
        .filter(|&p| p != me && p > 0)
        .filter(|&p| bsd(p).map_or(false, |b| b.pbi_uid == uid && b.pbi_status != SZOMB))
        .filter(|&p| resp_uniq(p) == Some(r))
        .collect()
}

fn cstrings(v: &[String]) -> Vec<CString> {
    v.iter().map(|s| CString::new(s.as_str()).unwrap()).collect()
}

/// PLAN.md §3.1: re-exec this process with SETEXEC + disclaim, so it becomes responsible for
/// itself and for everything it spawns. Returns false when the SPI is unavailable (fallback).
fn become_responsible() -> bool {
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
    let argv = cstrings(&std::env::args().collect::<Vec<_>>());
    let mut ptrs: Vec<*mut c_char> = argv.iter().map(|c| c.as_ptr() as *mut c_char).collect();
    ptrs.push(std::ptr::null_mut());
    unsafe {
        let mut attr: posix_spawnattr_t = zeroed();
        libc::posix_spawnattr_init(&mut attr);
        disclaim(&mut attr, 1);
        libc::posix_spawnattr_setflags(&mut attr, libc::POSIX_SPAWN_SETEXEC as i16);
        let env = *_NSGetEnviron() as *const *mut c_char;
        libc::posix_spawn(std::ptr::null_mut(), path.as_ptr(), std::ptr::null(), &attr, ptrs.as_ptr(), env);
    }
    false // only reached if the re-exec failed
}

/// Spawn the root with the default signal dispositions and an empty mask (PLAN.md §3.1).
fn spawn(cmd: &[String], disclaim_root: bool) -> pid_t {
    let argv = cstrings(cmd);
    let mut ptrs: Vec<*mut c_char> = argv.iter().map(|c| c.as_ptr() as *mut c_char).collect();
    ptrs.push(std::ptr::null_mut());
    let mut pid: pid_t = 0;
    let rc = unsafe {
        let mut attr: posix_spawnattr_t = zeroed();
        libc::posix_spawnattr_init(&mut attr);
        let mut all: libc::sigset_t = zeroed();
        libc::sigfillset(&mut all);
        let mut none: libc::sigset_t = zeroed();
        libc::sigemptyset(&mut none);
        libc::posix_spawnattr_setsigdefault(&mut attr, &all);
        libc::posix_spawnattr_setsigmask(&mut attr, &none);
        libc::posix_spawnattr_setflags(
            &mut attr,
            (libc::POSIX_SPAWN_SETSIGDEF | libc::POSIX_SPAWN_SETSIGMASK) as i16,
        );
        if disclaim_root {
            if let Some(d) = sym::<Disclaim>("responsibility_spawnattrs_setdisclaim") {
                d(&mut attr, 1);
            }
        }
        let env = *_NSGetEnviron() as *const *mut c_char;
        libc::posix_spawnp(&mut pid, ptrs[0], std::ptr::null(), &attr, ptrs.as_ptr(), env)
    };
    if rc != 0 {
        eprintln!("sheepdog: cannot run {}: {}", cmd[0], std::io::Error::from_raw_os_error(rc));
        std::process::exit(if rc == libc::ENOENT { 127 } else { 126 });
    }
    pid
}

fn wait(pid: pid_t) -> i32 {
    let mut st = 0;
    loop {
        let r = unsafe { libc::waitpid(pid, &mut st, 0) };
        if r == pid {
            return code_of(st);
        }
        if r < 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            return 125;
        }
    }
}

pub fn run(a: &Args) -> i32 {
    match a.mode.as_deref() {
        None | Some("responsible") => {
            let ok = become_responsible();
            std::env::remove_var(REEXEC_MARK);
            if !ok {
                eprintln!("sheepdog: the macOS responsibility API is not available; tracking is degraded");
            }
            let me = uniq(unsafe { libc::getpid() }).map(|u| u.0).unwrap_or(0);
            let root = spawn(&a.cmd, false);
            let code = wait(root);
            if !kill_tree(|| responsible_to(me), || {}) {
                return 125;
            }
            code
        }
        Some("root-disclaim") => {
            let root = spawn(&a.cmd, true);
            let r = uniq(root).map(|u| u.0).unwrap_or(0);
            let code = wait(root);
            if !kill_tree(|| responsible_to(r), || {}) {
                return 125;
            }
            code
        }
        Some(m) => {
            eprintln!("sheepdog: unknown mode {m}");
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
