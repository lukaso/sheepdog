//! Process identity that survives pid reuse (PLAN.md §3.2, §3.3): the macOS uniqueid (a
//! 64-bit counter from boot, never reused within a boot), or the Linux start time (clock ticks
//! since boot, from /proc/<pid>/stat field 22). None = gone, or not readable.

#[cfg(target_os = "macos")]
pub fn identity(pid: i32) -> Option<u64> {
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
    let mut u: UniqInfo = unsafe { std::mem::zeroed() };
    let n = std::mem::size_of::<UniqInfo>() as libc::c_int;
    let r = unsafe { libc::proc_pidinfo(pid, 17, 0, &mut u as *mut _ as *mut libc::c_void, n) };
    (r == n && u.uniqueid != 0).then_some(u.uniqueid)
}

#[cfg(target_os = "linux")]
pub fn identity(pid: i32) -> Option<u64> {
    let s = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = &s[s.rfind(')')? + 2..];
    // fields after the command name start at field 3 (state); start time is field 22
    let f: Vec<&str> = rest.split_whitespace().collect();
    if f.first() == Some(&"Z") {
        return None; // a zombie is dead for our purposes
    }
    f.get(22 - 3)?.parse().ok()
}

/// Is `pid` stopped by a signal (state T)?
#[cfg(target_os = "macos")]
pub fn stopped(pid: i32) -> bool {
    let mut b: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let n = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    let r = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, &mut b as *mut _ as *mut libc::c_void, n) };
    r == n && b.pbi_status == 4 // SSTOP
}

/// Is `pid` stopped by a signal (state T)?
#[cfg(target_os = "linux")]
pub fn stopped(pid: i32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|s| s.rfind(')').and_then(|i| s[i + 1..].split_whitespace().next().map(|f| f == "T")))
        .unwrap_or(false)
}

/// True when `pid` is alive and is still the process that had identity `id`.
pub fn same(pid: i32, id: u64) -> bool {
    identity(pid) == Some(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_process_has_a_stable_identity() {
        let me = unsafe { libc::getpid() };
        let a = identity(me).expect("own identity");
        assert_eq!(identity(me), Some(a));
        assert!(same(me, a));
        assert!(!same(me, a.wrapping_add(1)));
    }

    #[test]
    fn a_dead_process_has_no_identity() {
        let mut c = std::process::Command::new("true").spawn().unwrap();
        let pid = c.id() as i32;
        c.wait().unwrap();
        assert_eq!(identity(pid), None);
    }
}
