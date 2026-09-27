//! A process's environment block, read from outside (PHASE2.md §0.1: the test tag). Linux
//! `/proc/<pid>/environ`; macOS `KERN_PROCARGS2`. Both are the process's environment memory as
//! set at exec. macOS gives no environment for an Apple platform binary (`/bin/sh`,
//! `/bin/sleep`), measured 2026-09-28; such a process reads as a block without the tag.

/// What reading a process's environment found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvRead {
    /// the entries (`NAME=value`, raw bytes); may be empty
    Block(Vec<Vec<u8>>),
    /// alive, but its environment could not be read
    Unreadable,
    /// gone, a zombie, or exiting (its address space is being released)
    Gone,
}

/// The verdict for one process: does it carry the test tag `tag`?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagVerdict {
    Tagged,
    /// the block was read and holds no matching tag entry
    Withheld,
    Unreadable,
    Gone,
}

/// The whole entry `SHEEPDOG_TEST_TAG=<tag>` must be present; a substring never matches.
pub fn has_tag(entries: &[Vec<u8>], tag: &str) -> bool {
    let want = format!("SHEEPDOG_TEST_TAG={tag}");
    entries.iter().any(|e| e.as_slice() == want.as_bytes())
}

/// The verdict from a read and whether the process is exiting now. A block without the tag of a
/// process that is exiting is `Gone`: an exiting process's block reads empty (Linux, after its
/// address space is released) or fails (macOS), and it must not count as untagged.
pub fn verdict(read: &EnvRead, tag: &str, exiting: bool) -> TagVerdict {
    match read {
        EnvRead::Block(e) if has_tag(e, tag) => TagVerdict::Tagged,
        _ if exiting => TagVerdict::Gone,
        EnvRead::Block(_) => TagVerdict::Withheld,
        EnvRead::Unreadable => TagVerdict::Unreadable,
        EnvRead::Gone => TagVerdict::Gone,
    }
}

#[cfg(target_os = "linux")]
fn split(b: &[u8]) -> Vec<Vec<u8>> {
    b.split(|&c| c == 0).filter(|e| !e.is_empty()).map(<[u8]>::to_vec).collect()
}

#[cfg(target_os = "linux")]
pub fn read_env(pid: i32) -> EnvRead {
    match std::fs::read(format!("/proc/{pid}/environ")) {
        Ok(b) => EnvRead::Block(split(&b)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound || e.raw_os_error() == Some(libc::ESRCH) => EnvRead::Gone,
        Err(_) => EnvRead::Unreadable,
    }
}

/// Is `pid` exiting or gone now? Linux: no `stat`, state Z or X, or an address space already
/// released (`statm` all zero; a kernel thread also reads so, and is never a target).
#[cfg(target_os = "linux")]
pub fn exiting(pid: i32) -> bool {
    let Ok(s) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else { return true };
    let state = s.rfind(')').and_then(|i| s[i + 1..].split_whitespace().next()).unwrap_or("X");
    if state == "Z" || state == "X" {
        return true;
    }
    match std::fs::read_to_string(format!("/proc/{pid}/statm")) {
        Ok(m) => m.split_whitespace().all(|f| f == "0"),
        Err(_) => true,
    }
}

#[cfg(target_os = "macos")]
pub fn read_env(pid: i32) -> EnvRead {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let mut buf = vec![0u8; arg_max()];
    let mut len = buf.len();
    let r = unsafe { libc::sysctl(mib.as_mut_ptr(), 3, buf.as_mut_ptr() as *mut libc::c_void, &mut len, std::ptr::null_mut(), 0) };
    if r != 0 || len < 4 {
        return if kinfo_flags(pid).is_none() { EnvRead::Gone } else { EnvRead::Unreadable };
    }
    // argc, the exec path, NUL padding, argv (argc strings), then the environment strings
    let argc = i32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]).max(0) as usize;
    let rest = &buf[4..len];
    let Some(end) = rest.iter().position(|&c| c == 0) else { return EnvRead::Block(Vec::new()) };
    let rest = &rest[end..];
    let Some(start) = rest.iter().position(|&c| c != 0) else { return EnvRead::Block(Vec::new()) };
    let strings: Vec<&[u8]> = rest[start..].split(|&c| c == 0).collect();
    let env = strings.iter().skip(argc).take_while(|e| !e.is_empty()).map(|e| e.to_vec()).collect();
    EnvRead::Block(env)
}

/// KERN_ARGMAX: the largest argument and environment area (256 KiB if unreadable).
#[cfg(target_os = "macos")]
fn arg_max() -> usize {
    let mut mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
    let mut v: libc::c_int = 0;
    let mut len = std::mem::size_of::<libc::c_int>();
    let r = unsafe { libc::sysctl(mib.as_mut_ptr(), 2, &mut v as *mut _ as *mut libc::c_void, &mut len, std::ptr::null_mut(), 0) };
    if r == 0 && v > 0 {
        v as usize
    } else {
        256 * 1024
    }
}

/// `kinfo_proc`'s `p_flag` and `p_stat` (offsets 32 and 36 of the 648-byte struct, which the
/// libc crate lacks; a unit cell checks them on a stopped and on a zombie child). None = gone.
#[cfg(target_os = "macos")]
pub fn kinfo_flags(pid: i32) -> Option<(i32, u8)> {
    const SIZE: usize = 648;
    let mut mib = [libc::CTL_KERN, libc::KERN_PROC, libc::KERN_PROC_PID, pid];
    let mut buf = [0u8; SIZE];
    let mut len = SIZE;
    let r = unsafe { libc::sysctl(mib.as_mut_ptr(), 4, buf.as_mut_ptr() as *mut libc::c_void, &mut len, std::ptr::null_mut(), 0) };
    if r != 0 || len != SIZE {
        return None;
    }
    Some((i32::from_ne_bytes(buf[32..36].try_into().ok()?), buf[36]))
}

/// Is `pid` exiting or gone now? macOS: gone, a zombie (`p_stat` SZOMB), or `P_WEXIT` set.
#[cfg(target_os = "macos")]
pub fn exiting(pid: i32) -> bool {
    const P_WEXIT: i32 = 0x2000;
    const SZOMB: u8 = 5;
    match kinfo_flags(pid) {
        Some((flag, stat)) => stat == SZOMB || flag & P_WEXIT != 0,
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(s: &str) -> Vec<u8> {
        s.as_bytes().to_vec()
    }

    #[test]
    fn the_tag_matches_the_whole_entry_only() {
        let t = "abcd1234abcd1234";
        assert!(has_tag(&[e("A=1"), e("SHEEPDOG_TEST_TAG=abcd1234abcd1234")], t));
        assert!(!has_tag(&[e("SHEEPDOG_TEST_TAG=abcd1234abcd12345")], t), "a longer value");
        assert!(!has_tag(&[e("XSHEEPDOG_TEST_TAG=abcd1234abcd1234")], t), "a longer name");
        assert!(!has_tag(&[e("SHEEPDOG_TEST_TAG=abcd1234")], t), "a prefix of the tag");
        assert!(!has_tag(&[], t));
    }

    #[test]
    fn an_exiting_process_without_the_tag_is_gone_not_withheld() {
        let t = "abcd1234abcd1234";
        let tagged = EnvRead::Block(vec![e("SHEEPDOG_TEST_TAG=abcd1234abcd1234")]);
        let empty = EnvRead::Block(Vec::new());
        assert_eq!(verdict(&tagged, t, false), TagVerdict::Tagged);
        assert_eq!(verdict(&tagged, t, true), TagVerdict::Tagged);
        assert_eq!(verdict(&empty, t, false), TagVerdict::Withheld, "alive with no tag");
        assert_eq!(verdict(&empty, t, true), TagVerdict::Gone, "an exiting process's block reads empty");
        assert_eq!(verdict(&EnvRead::Unreadable, t, false), TagVerdict::Unreadable);
        assert_eq!(verdict(&EnvRead::Unreadable, t, true), TagVerdict::Gone);
        assert_eq!(verdict(&EnvRead::Gone, t, false), TagVerdict::Gone);
    }

    /// The live-process reads: this process's own environment holds a variable we set before
    /// spawning a child that inherits it; and an exited child is exiting/gone.
    #[test]
    fn a_spawned_child_carries_a_variable_and_an_exited_child_is_exiting() {
        let mut c = std::process::Command::new("/bin/sh")
            .args(["-c", "exec cat"])
            .env("SD_ENVTAG_PROBE", "on")
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let pid = c.id() as i32;
        // `cat` is not a platform binary on Linux; on macOS /bin/cat is, so only Linux reads it
        std::thread::sleep(std::time::Duration::from_millis(100));
        if cfg!(target_os = "linux") {
            match read_env(pid) {
                EnvRead::Block(b) => assert!(b.iter().any(|x| x.as_slice() == b"SD_ENVTAG_PROBE=on")),
                other => panic!("{other:?}"),
            }
        }
        assert!(!exiting(pid), "alive and running");
        drop(c.stdin.take());
        // wait until it has exited but is not reaped: a zombie
        let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !exiting(pid) && std::time::Instant::now() < end {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(exiting(pid), "an exited, unreaped child is exiting");
        c.wait().unwrap();
    }

    /// macOS: the measured `kinfo_proc` offsets: a stopped child reads `p_stat` SSTOP (4), a
    /// zombie SZOMB (5) with `P_WEXIT`, a running one neither.
    #[cfg(target_os = "macos")]
    #[test]
    fn kinfo_offsets_read_stop_and_zombie() {
        let mut c = std::process::Command::new("/bin/sleep").arg("30").spawn().unwrap();
        let pid = c.id() as i32;
        let (flag, stat) = kinfo_flags(pid).unwrap();
        assert_ne!(stat, 5);
        assert_eq!(flag & 0x2000, 0);
        unsafe { libc::kill(pid, libc::SIGSTOP) };
        let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while kinfo_flags(pid).map(|f| f.1) != Some(4) && std::time::Instant::now() < end {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(kinfo_flags(pid).map(|f| f.1), Some(4), "SSTOP");
        unsafe { libc::kill(pid, libc::SIGKILL) };
        let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while kinfo_flags(pid).map(|f| f.1) != Some(5) && std::time::Instant::now() < end {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let (flag, stat) = kinfo_flags(pid).unwrap();
        assert_eq!(stat, 5, "SZOMB");
        assert_ne!(flag & 0x2000, 0, "P_WEXIT");
        c.wait().unwrap();
    }
}
