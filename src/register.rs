//! Registration of nested runs (macOS; PLAN.md §3.2, PHASE2.md decision 13 and D5).
//!
//! The inner side (`register_all`) runs in the process that becomes an inner supervisor, before
//! its disclaim: it registers with every supervisor of the `SHEEPR_OUTER` chain, in series,
//! within one 2 s budget. The outer side (`Listener`) is served by the wait loop: non-blocking
//! accepts and reads, a 100 ms deadline per connection, at most 32 pending. The peer's identity is
//! the kernel's (`LOCAL_PEERTOKEN`, read after the record); the record's pid is only a claim.
//!
//! The listener's folder also holds an `owner` record (`v1 <pid> <identity> <pidns>`), so a later
//! `sweep` can remove the folder of a sheepr that is gone (issue #20: one killed with SIGKILL
//! cannot remove its own); `reap` does that.

use sheepr::regwire::{self, Entry};
use std::os::fd::RawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::say;
use std::io::Write as _;

/// The inner's budget for the whole chain.
pub const BUDGET: Duration = Duration::from_secs(2);
/// A connection's deadline for its whole record.
const READ_DEADLINE: Duration = Duration::from_millis(100);
/// Pending connections at most; past it, new ones wait in the listen backlog (64).
const MAX_PENDING: usize = 32;
/// Accepts in one `service` call at most, so a client that connects and closes without end
/// cannot keep the wait loop from its own checks (the rest waits in the backlog).
const MAX_ACCEPTS: usize = 2 * MAX_PENDING;
/// `sun_path` holds 104 bytes on macOS; a longer `$TMPDIR` path goes under /tmp.
const PATH_MAX: usize = 100;

fn sockaddr(path: &Path) -> Option<(libc::sockaddr_un, libc::socklen_t)> {
    use std::os::unix::ffi::OsStrExt;
    let b = path.as_os_str().as_bytes();
    let mut a: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if b.len() >= a.sun_path.len() {
        return None;
    }
    a.sun_family = libc::AF_UNIX as _;
    for (d, s) in a.sun_path.iter_mut().zip(b) {
        *d = *s as libc::c_char;
    }
    let len = (std::mem::size_of::<libc::sockaddr_un>() - a.sun_path.len() + b.len() + 1) as libc::socklen_t;
    Some((a, len))
}

fn set_flags(fd: RawFd) {
    unsafe {
        libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        let fl = libc::fcntl(fd, libc::F_GETFL);
        libc::fcntl(fd, libc::F_SETFL, fl | libc::O_NONBLOCK);
    }
}

/// Register with one outer supervisor: Ok(()) on its ack byte; Err(why) otherwise (refused: the
/// outer closed without a byte; or the socket, the write or the time failed).
fn register_one(e: &Entry, end: Instant) -> Result<(), String> {
    let (addr, len) = sockaddr(&e.path).ok_or("socket path too long")?;
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
    if fd < 0 {
        return Err(format!("socket: {}", std::io::Error::last_os_error()));
    }
    let close = |why: String| {
        unsafe { libc::close(fd) };
        Err(why)
    };
    set_flags(fd);
    // a unix-domain connect completes at once or fails at once
    if unsafe { libc::connect(fd, &addr as *const _ as *const libc::sockaddr, len) } != 0 {
        return close(format!("connect: {}", std::io::Error::last_os_error()));
    }
    let rec = regwire::record(&e.nonce, unsafe { libc::getpid() });
    let n = unsafe { libc::write(fd, rec.as_ptr() as *const libc::c_void, rec.len()) };
    if n != rec.len() as isize {
        return close(format!("write: {}", std::io::Error::last_os_error()));
    }
    loop {
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return close("no answer in time".into());
        }
        let mut p = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        let r = unsafe { libc::poll(&mut p, 1, left.as_millis().max(1) as libc::c_int) };
        if r < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
            continue;
        }
        if r <= 0 {
            continue; // the loop's own time check ends it
        }
        let mut b = [0u8; 1];
        let n = unsafe { libc::read(fd, b.as_mut_ptr() as *mut libc::c_void, 1) };
        return match n {
            1 if b[0] == b'1' => {
                unsafe { libc::close(fd) };
                Ok(())
            }
            0 => close("refused".into()),
            n if n < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EAGAIN) => continue,
            _ => close("an unexpected answer".into()),
        };
    }
}

/// The inner side: register with every entry of the chain, within one budget. Returns how many
/// acked; warns once per entry that did not.
pub fn register_all(chain: &[Entry]) -> usize {
    let end = Instant::now() + BUDGET;
    let mut ok = 0;
    for e in chain {
        match register_one(e, end) {
            Ok(()) => {
                ok += 1;
                crate::note(format!("registered {}", e.path.display()));
            }
            Err(why) => {
                crate::note(format!("unregistered {}: {why}", e.path.display()));
                say!("sheepr: could not register with the sheepr that runs this one ({why}); the outer adds this job only if its next scan still finds this process in its tree");
            }
        }
    }
    ok
}

/// The chain this process was given, parsed; with `warn`, says so for lines that do not parse.
pub fn inherited(warn: bool) -> Vec<Entry> {
    let Ok(s) = std::env::var(regwire::VAR) else { return Vec::new() };
    let (v, bad) = regwire::parse(&s);
    if warn && bad > 0 {
        say!("sheepr: {bad} malformed entr{} in {} ignored", if bad == 1 { "y" } else { "ies" }, regwire::VAR);
    }
    v
}

struct Conn {
    fd: RawFd,
    buf: Vec<u8>,
    deadline: Instant,
}

/// The outer side: a listening socket in a fresh 0700 directory, and the pending connections.
pub struct Listener {
    fd: RawFd,
    dir: PathBuf,
    path: PathBuf,
    nonce: [u8; 16],
    pending: Vec<Conn>,
    /// Accepts in one `service` call at most (MAX_ACCEPTS; a cell sets a smaller one).
    max_accepts: usize,
    /// Accepts in the last `service` call (for its cell).
    last_accepts: usize,
}

/// The wait loop's admit function, called twice for a peer's uniqueid: with `false` it only
/// answers "is it a member now?"; with `true` (after every other check passed) it adds it to R.
pub type Admit<'a> = dyn FnMut(u64, bool) -> bool + 'a;

impl Listener {
    /// A listener in a `mkdtemp` directory under `$TMPDIR`, or under /tmp when the socket path
    /// would pass 100 bytes. None (and one warning) if it cannot be made.
    pub fn open() -> Option<Listener> {
        let mut nonce = [0u8; 16];
        if std::fs::File::open("/dev/urandom").and_then(|mut f| std::io::Read::read_exact(&mut f, &mut nonce)).is_err() {
            say!("sheepr: no entropy for a registration nonce; nested runs are found by the scan only");
            return None;
        }
        let base = base_for(std::env::var_os("TMPDIR"));
        let tmpl = std::ffi::CString::new(format!("{}/sr-XXXXXXXX", base.display().to_string().trim_end_matches('/'))).ok()?;
        let mut raw = tmpl.into_bytes_with_nul();
        let d = unsafe { libc::mkdtemp(raw.as_mut_ptr() as *mut libc::c_char) };
        if d.is_null() {
            say!("sheepr: cannot make a registration directory: {}", std::io::Error::last_os_error());
            return None;
        }
        raw.pop();
        let dir = PathBuf::from(String::from_utf8_lossy(&raw).into_owned());
        let path = dir.join("s");
        // who owns the folder, before the socket exists (a `sweep` removes it once this sheepr is
        // gone); best effort: without it, a sweep removes the folder only when it is old and
        // nothing listens on it
        let me = unsafe { libc::getpid() };
        if let Some(id) = sheepr::ident::identity(me) {
            let _ = std::fs::write(dir.join(OWNER), format!("v1 {me} {id} {}\n", crate::journal::pidns()));
        }
        let fail = |why: String, fd: RawFd| {
            if fd >= 0 {
                unsafe { libc::close(fd) };
            }
            let _ = std::fs::remove_file(&path);
            let _ = std::fs::remove_file(dir.join(OWNER));
            let _ = std::fs::remove_dir(&dir);
            say!("sheepr: cannot listen for nested runs ({why}); they are found by the scan only");
            None
        };
        if !regwire::path_fits(&path) {
            return fail("the socket path cannot be passed on".into(), -1);
        }
        let Some((addr, len)) = sockaddr(&path) else { return fail("the socket path is too long".into(), -1) };
        let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
        if fd < 0 {
            return fail(format!("socket: {}", std::io::Error::last_os_error()), fd);
        }
        set_flags(fd);
        if unsafe { libc::bind(fd, &addr as *const _ as *const libc::sockaddr, len) } != 0 {
            return fail(format!("bind: {}", std::io::Error::last_os_error()), fd);
        }
        if unsafe { libc::listen(fd, 64) } != 0 {
            return fail(format!("listen: {}", std::io::Error::last_os_error()), fd);
        }
        crate::note(format!("listening {}", path.display()));
        Some(Listener { fd, dir, path, nonce, pending: Vec::new(), max_accepts: MAX_ACCEPTS, last_accepts: 0 })
    }

    pub fn entry(&self) -> Entry {
        Entry { nonce: self.nonce, path: self.path.clone() }
    }

    /// The fds the wait loop watches for reading: the listener and each pending connection.
    pub fn fds(&self) -> Vec<RawFd> {
        std::iter::once(self.fd).chain(self.pending.iter().map(|c| c.fd)).collect()
    }

    /// When the loop must wake next for a deadline, if any connection is pending.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.pending.iter().map(|c| c.deadline).min()
    }

    /// The listening socket's fd, for the wait loop's watch.
    pub fn listen_fd(&self) -> RawFd {
        self.fd
    }

    /// At the cap: the wait loop stops watching the listener until a connection is done.
    pub fn full(&self) -> bool {
        self.pending.len() >= MAX_PENDING
    }

    /// Accept what is waiting (up to the cap: the rest stays in the listen backlog), read what has
    /// come, answer every complete record and drop every connection past its deadline; again while
    /// that made room, up to the listener's accept bound (MAX_ACCEPTS). Never blocks. Returns the
    /// new fds to watch: only connections still pending.
    pub fn service(&mut self, admit: &mut Admit) -> Vec<RawFd> {
        let mut new = Vec::new();
        let mut accepts = 0;
        loop {
            let mut accepted = false;
            while self.pending.len() < MAX_PENDING && accepts < self.max_accepts {
                let c = unsafe { libc::accept(self.fd, std::ptr::null_mut(), std::ptr::null_mut()) };
                if c < 0 {
                    break; // EAGAIN: nothing more waiting (any other error: try again next wake)
                }
                set_flags(c);
                self.pending.push(Conn { fd: c, buf: Vec::with_capacity(regwire::RECORD), deadline: Instant::now() + READ_DEADLINE });
                new.push(c);
                accepted = true;
                accepts += 1;
            }
            self.serve_pending(admit);
            if !accepted || self.full() || accepts >= self.max_accepts {
                break;
            }
        }
        self.last_accepts = accepts;
        // an fd closed in this call (and perhaps reused since) is never handed back
        new.retain(|fd| self.pending.iter().any(|c| c.fd == *fd));
        new
    }

    fn serve_pending(&mut self, admit: &mut Admit) {
        let now = Instant::now();
        let mut keep = Vec::with_capacity(self.pending.len());
        for mut c in std::mem::take(&mut self.pending) {
            let mut b = [0u8; regwire::RECORD];
            let want = regwire::RECORD - c.buf.len();
            let n = unsafe { libc::read(c.fd, b.as_mut_ptr() as *mut libc::c_void, want) };
            if n > 0 {
                c.buf.extend_from_slice(&b[..n as usize]);
            }
            let eof = n == 0;
            if c.buf.len() == regwire::RECORD {
                let ok = self.answer(&c, admit);
                if ok {
                    unsafe { libc::write(c.fd, b"1".as_ptr() as *const libc::c_void, 1) };
                }
                unsafe { libc::close(c.fd) };
            } else if eof || now >= c.deadline {
                unsafe { libc::close(c.fd) };
                crate::note(format!("registration dropped: {}", if eof { "closed early" } else { "too slow" }));
            } else {
                keep.push(c);
            }
        }
        self.pending = keep;
    }

    /// Check one complete record. The peer is the kernel's, taken now (after the record); the
    /// claim must name it; it must be this uid and a member now (the scan's own function), with
    /// the same process version before and after that check.
    fn answer(&self, c: &Conn, admit: &mut Admit) -> bool {
        let Some((nonce, claim)) = regwire::read_record(&c.buf) else {
            crate::note("registration refused: malformed".into());
            say!("sheepr: a malformed registration was refused");
            return false;
        };
        if nonce != self.nonce {
            crate::note("registration refused: wrong nonce".into());
            return false;
        }
        let Some(tok) = peer_token(c.fd) else {
            crate::note("registration refused: no peer token".into());
            return false;
        };
        let (euid, ruid, pid, pidversion) = (tok[1], tok[3], tok[5] as i32, tok[7] as i32);
        let me = unsafe { libc::getuid() };
        if euid != me || ruid != me {
            crate::note(format!("registration refused: uid {ruid}"));
            return false;
        }
        if claim != pid {
            crate::note(format!("registration refused: claims {claim}, is {pid}"));
            say!("sheepr: a registration named pid {claim}, but its sender is pid {pid}; refused");
            return false;
        }
        let Some((uniq, v1)) = versioned(pid) else { return false };
        if v1 != pidversion {
            crate::note(format!("registration refused: {pid} changed before the check"));
            return false;
        }
        let member = admit(uniq, false);
        let v2 = if crate::seam_flag("SHEEPR_TEST_REG_PIDVERSION_CHANGE") { versioned(pid).map(|v| v.1 + 1) } else { versioned(pid).map(|v| v.1) };
        if v2 != Some(pidversion) {
            crate::note(format!("registration refused: {pid} changed during the check"));
            return false;
        }
        if !member {
            crate::note(format!("registration refused: {pid} is not a member"));
            return false;
        }
        if !admit(uniq, true) {
            crate::note(format!("registration refused: {pid} could not be added"));
            return false;
        }
        crate::note(format!("registration accepted: {pid}"));
        true
    }
}

#[cfg(test)]
mod base_tests {
    use super::*;

    /// Where a listener puts its folder, and so where `reap` looks: $TMPDIR when the socket path
    /// under it fits, else /tmp (also with no TMPDIR, or a relative one).
    #[test]
    fn the_folder_base_is_tmpdir_when_the_path_fits_else_tmp() {
        assert_eq!(base_for(Some("/var/folders/ab/T".into())), PathBuf::from("/var/folders/ab/T"));
        assert_eq!(base_for(Some(format!("/x/{}", "a".repeat(120)).into())), PathBuf::from("/tmp"));
        assert_eq!(base_for(None), PathBuf::from("/tmp"));
        assert_eq!(base_for(Some("relative".into())), PathBuf::from("/tmp"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A record complete at its first read is answered and closed in the same call: `service`
    /// never hands back an fd it has closed (the wait loop would watch whatever reuses it).
    #[test]
    fn service_returns_only_pending_connections() {
        let mut l = Listener::open().expect("a listener");
        let mut done = std::os::unix::net::UnixStream::connect(&l.path).unwrap();
        done.write_all(&regwire::record(&[0; 16], 1)).unwrap(); // wrong nonce: answered at once
        let _quiet = std::os::unix::net::UnixStream::connect(&l.path).unwrap(); // stays pending
        let mut no = |_: u64, _: bool| false;
        let back = l.service(&mut no);
        let watched = l.fds();
        assert_eq!(back.len(), 1, "{back:?}");
        assert!(back.iter().all(|fd| watched.contains(fd)), "returned {back:?}, pending {watched:?}");
    }

    /// A same-uid process that connects and closes without end cannot keep the wait loop inside
    /// one `service` call: a call stops at its accept bound, and what it left in the backlog is
    /// taken by the next call. Deterministic: 20 closed connections wait in the backlog and the
    /// bound is 8 (an unbounded call takes all 20; nothing refills the backlog meanwhile).
    #[test]
    fn service_stops_at_its_accept_bound() {
        let mut l = Listener::open().expect("a listener");
        // a listener is made with the production bound, which is finite and larger than the
        // smaller bound this cell uses to check the bound's effect
        assert!(l.max_accepts == MAX_ACCEPTS && MAX_ACCEPTS > 8 && MAX_ACCEPTS < usize::MAX, "bound {}", l.max_accepts);
        l.max_accepts = 8;
        for _ in 0..20 {
            drop(std::os::unix::net::UnixStream::connect(&l.path).unwrap());
        }
        let mut no = |_: u64, _: bool| false;
        let mut counts = Vec::new();
        for _ in 0..4 {
            let _ = l.service(&mut no);
            counts.push(l.last_accepts);
        }
        assert_eq!(counts, vec![8, 8, 4, 0]);
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        for c in &self.pending {
            unsafe { libc::close(c.fd) };
        }
        unsafe { libc::close(self.fd) };
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_file(self.dir.join(OWNER));
        let _ = std::fs::remove_dir(&self.dir);
    }
}

/// The owner record's name in a registration folder.
const OWNER: &str = "owner";

/// Where a listener puts its folder, and so where `reap` looks (issue #20): `tmpdir` when the
/// socket path under it fits, else /tmp (also with no TMPDIR, or a relative one).
pub fn base_for(tmpdir: Option<std::ffi::OsString>) -> PathBuf {
    let tmp = tmpdir.map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| "/tmp".into());
    // mkdtemp's name: "sr-" + 8 characters; the socket's name: "s"
    if tmp.join("sr-XXXXXXXX").join("s").as_os_str().len() <= PATH_MAX { tmp } else { PathBuf::from("/tmp") }
}

/// Remove the registration folders of sheeprs that are gone (issue #20), under each of `bases`,
/// until `deadline`; returns how many. A folder is removed only when it is a registration folder
/// exactly (named `sr-` and 8 letters or digits, this user's, mode 0700, not a symlink, holding
/// nothing but the socket `s` and `owner`) and either its owner record (this pid namespace) names
/// a process that is gone, or it has no owner record that parses (an older sheepr's, one made and
/// never bound, or a write that failed) and is more than a day old with nothing listening on its
/// socket.
pub fn reap(bases: &[PathBuf], deadline: Instant) -> usize {
    let mut n = 0;
    for base in bases {
        let Ok(rd) = std::fs::read_dir(base) else { continue };
        for e in rd.flatten() {
            if Instant::now() > deadline {
                return n;
            }
            let name = e.file_name();
            let b = std::os::unix::ffi::OsStrExt::as_bytes(name.as_os_str());
            if b.len() != 11 || !b.starts_with(b"sr-") || !b[3..].iter().all(|c| c.is_ascii_alphanumeric()) {
                continue;
            }
            let f = base.join(&name);
            if gone(&f) {
                let _ = std::fs::remove_file(f.join(OWNER));
                let _ = std::fs::remove_file(f.join("s"));
                if std::fs::remove_dir(&f).is_ok() {
                    n += 1;
                }
            }
        }
    }
    n
}

/// Is `f` the registration folder of a sheepr that is gone (see `reap`)?
fn gone(f: &Path) -> bool {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    let Ok(m) = std::fs::symlink_metadata(f) else { return false };
    if !m.file_type().is_dir() || m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o777 != 0o700 {
        return false;
    }
    let Ok(rd) = std::fs::read_dir(f) else { return false };
    let (mut sock, mut owner) = (false, None);
    for e in rd.flatten() {
        let Ok(t) = e.file_type() else { return false };
        match e.file_name().to_str() {
            Some("s") if t.is_socket() => sock = true,
            Some(OWNER) if t.is_file() => owner = Some(std::fs::read_to_string(e.path()).unwrap_or_default()),
            _ => return false, // anything else: not (only) a registration folder
        }
    }
    if let Some(o) = owner {
        let w: Vec<&str> = o.trim_end_matches('\n').split(' ').collect();
        if let ["v1", pid, id, ns] = w.as_slice() {
            if let (Ok(p), Ok(i)) = (pid.parse::<i32>(), id.parse::<u64>()) {
                if p > 1 {
                    return *ns == crate::journal::pidns() && owner_gone(p, i);
                }
            }
        }
        // a record that does not parse (an empty or cut write): as no record
    }
    let old = m.modified().ok().and_then(|t| t.elapsed().ok()).is_some_and(|a| a > Duration::from_secs(86400));
    // an older sheepr's folder: nothing listens on its socket (or it never got one)
    old && (!sock || std::os::unix::net::UnixStream::connect(f.join("s")).is_err_and(|e| e.kind() == std::io::ErrorKind::ConnectionRefused))
}

/// Is the owner `p` (identity `i`) gone? Only on positive evidence: its identity read and another,
/// or no process `p` at all (ESRCH). An identity that cannot be read for a live process (a zombie
/// not yet reaped, a policy that refuses the read) keeps the folder. The debug seam
/// SHEEPR_TEST_IDENTITY_UNREADABLE=<pid> makes that pid's identity unreadable.
fn owner_gone(p: i32, i: u64) -> bool {
    let unreadable = cfg!(debug_assertions) && std::env::var("SHEEPR_TEST_IDENTITY_UNREADABLE").ok().and_then(|v| v.parse::<i32>().ok()) == Some(p);
    match if unreadable { None } else { sheepr::ident::identity(p) } {
        Some(x) => x != i,
        None => (unsafe { libc::kill(p, 0) }) == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH),
    }
}

/// The peer's audit token (`LOCAL_PEERTOKEN`): eight u32; [1] euid, [3] ruid, [5] pid, [7] pidversion.
fn peer_token(fd: RawFd) -> Option<[u32; 8]> {
    const SOL_LOCAL: libc::c_int = 0;
    const LOCAL_PEERTOKEN: libc::c_int = 6;
    let mut t = [0u32; 8];
    let mut len = std::mem::size_of_val(&t) as libc::socklen_t;
    let r = unsafe { libc::getsockopt(fd, SOL_LOCAL, LOCAL_PEERTOKEN, t.as_mut_ptr() as *mut libc::c_void, &mut len) };
    (r == 0 && len as usize == std::mem::size_of_val(&t)).then_some(t)
}

/// (uniqueid, p_idversion) of a live process.
fn versioned(pid: i32) -> Option<(u64, i32)> {
    crate::macos::uniq_version(pid)
}
