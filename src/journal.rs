//! The job journal (PHASE2.md §1 decisions 1-6, PLAN.md §3.5): the record `sweep` reads if this
//! supervisor dies without killing its job.
//!
//! `$STATE/jobs/<boot-id>-<pidns>/<job-id>.journal`, JSON lines: a header (the job id, boot id,
//! pid namespace, owner tag, owner uid, and this supervisor's pid, identity and argv), then one
//! line per member, `{"pid","id","ppid","pid_id","puniq","cmd"}`, the root's with `"root":true`.
//! The lines of one scan go out in one write, before any signal to that scan's new members.
//!
//! Published without ever being visible unlocked: a temporary file (0600, O_EXCL) is locked
//! (`flock LOCK_EX`, the fd CLOEXEC), gets its header, and only then is linked to its final name
//! (`link` never replaces: a taken id picks a new one); then the temporary name is removed. A
//! clean end unlinks the journal, then releases the lock by closing the fd.
//!
//! The journal never stops the job (decision 2): any failure disables it with one warning, a
//! `journal-failed` note, and the job runs and is killed as without a journal.
//!
//! `cmd` text: at most 256 bytes; a backslash is written `\\` and a byte that is not UTF-8
//! `\xHH`, so the text is never ambiguous; then JSON-escaped.

use sheepdog::ident::identity;
use std::collections::HashSet;
use std::ffi::OsString;
use std::fs::File;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::PathBuf;

/// The cap on a `cmd` (and the header's argv) in bytes.
pub const CMD_CAP: usize = 256;

pub struct Journal {
    file: Option<File>,
    path: Option<PathBuf>,
    seen: HashSet<(i32, u64)>,
    job: Option<String>,
    warned: bool,
    /// a write failed: nothing more is written, but the fd (and so the lock) is kept until the
    /// end, so the published file never looks like a dead job's to `sweep`
    dead: bool,
}

/// The text form of raw bytes (see the module header), before JSON escaping.
pub fn text(b: &[u8]) -> String {
    let b = &b[..b.len().min(CMD_CAP)];
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        match std::str::from_utf8(&b[i..]) {
            Ok(s) => {
                out.push_str(&s.replace('\\', "\\\\"));
                break;
            }
            Err(e) => {
                let ok = e.valid_up_to();
                out.push_str(&std::str::from_utf8(&b[i..i + ok]).unwrap_or("").replace('\\', "\\\\"));
                i += ok;
                let bad = e.error_len().unwrap_or(b.len() - i);
                for x in &b[i..i + bad] {
                    out.push_str(&format!("\\x{x:02x}"));
                }
                i += bad;
            }
        }
    }
    out
}

/// A JSON string literal for `s`.
pub fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn joined(args: &[OsString]) -> Vec<u8> {
    let mut v = Vec::new();
    for (i, a) in args.iter().enumerate() {
        if i > 0 {
            v.push(b' ');
        }
        v.extend_from_slice(a.as_bytes());
    }
    v
}

#[cfg(target_os = "linux")]
pub fn boot_id() -> Option<String> {
    std::fs::read_to_string("/proc/sys/kernel/random/boot_id").ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

#[cfg(target_os = "macos")]
pub fn boot_id() -> Option<String> {
    let name = std::ffi::CString::new("kern.bootsessionuuid").ok()?;
    let mut buf = [0u8; 64];
    let mut len = buf.len();
    let r = unsafe { libc::sysctlbyname(name.as_ptr(), buf.as_mut_ptr() as *mut libc::c_void, &mut len, std::ptr::null_mut(), 0) };
    if r != 0 {
        return None;
    }
    let s = String::from_utf8_lossy(&buf[..len]).trim_end_matches('\0').trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// The pid namespace: Linux `<dev>.<ino>` of /proc/self/ns/pid; macOS `0`.
pub fn pidns() -> String {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::MetadataExt;
        if let Ok(m) = std::fs::metadata("/proc/self/ns/pid") {
            return format!("{}.{}", m.dev(), m.ino());
        }
        "unknown".to_string()
    }
    #[cfg(target_os = "macos")]
    {
        "0".to_string()
    }
}

/// A job id: `j-` + 8 random hex digits from /dev/urandom (debug seam SHEEPDOG_TEST_JOB_ID forces the first one).
fn new_id(first: bool) -> Option<String> {
    if first && cfg!(debug_assertions) {
        if let Ok(forced) = std::env::var("SHEEPDOG_TEST_JOB_ID") {
            return Some(format!("j-{forced}"));
        }
    }
    let mut b = [0u8; 4];
    std::io::Read::read_exact(&mut File::open("/dev/urandom").ok()?, &mut b).ok()?;
    Some(format!("j-{:02x}{:02x}{:02x}{:02x}", b[0], b[1], b[2], b[3]))
}

/// The parent pid and the macOS `puniq` of `pid` (None when unreadable).
fn lineage(pid: i32) -> (Option<i32>, Option<u64>) {
    #[cfg(target_os = "macos")]
    {
        (crate::macos::parent(pid), crate::macos::puniq(pid))
    }
    #[cfg(target_os = "linux")]
    {
        (crate::linux::parent(pid), None)
    }
}

fn member_cmd(pid: i32) -> Vec<u8> {
    #[cfg(target_os = "macos")]
    let argv = crate::macos::cmdline(pid);
    #[cfg(target_os = "linux")]
    let argv = crate::linux::report_cmd(pid);
    argv.join(" ").into_bytes()
}

fn num(v: Option<impl std::fmt::Display>) -> String {
    v.map_or_else(|| "null".to_string(), |x| x.to_string())
}

impl Journal {
    /// No journal (and why, as a note).
    fn off(why: &str) -> Journal {
        crate::note(why.to_string());
        crate::status::add_note(why);
        Journal { file: None, path: None, seen: HashSet::new(), job: None, warned: false, dead: false }
    }

    fn failed(&mut self, what: &str) {
        let n = format!("journal-failed: {what}");
        if !self.warned {
            self.warned = true;
            crate::say!("sheepdog: cannot keep the job journal ({what}); the job runs, but `sweep` cannot clean up after it if sheepdog is killed");
        }
        crate::note(n.clone());
        crate::status::add_note(&n);
        self.dead = true; // the fd stays open: a published journal stays locked until the end
    }

    /// Publish this job's journal (after the macOS SETEXEC, before the root runs).
    pub fn open(owner: &str, argv: &[OsString]) -> Journal {
        // the phase-1 opt-out disables state outright (PHASE2.md §0.1)
        if crate::seam_flag("SHEEPDOG_TEST_PHASE1") {
            return Journal::off("journal-disabled");
        }
        let Some(state) = crate::state::resolve(cfg!(debug_assertions), |k| std::env::var_os(k), |p| p.exists()) else {
            return Journal::off("state-unset");
        };
        let mut j = Journal { file: None, path: None, seen: HashSet::new(), job: None, warned: false, dead: false };
        let Some(boot) = boot_id() else {
            j.failed("no boot id");
            return j;
        };
        let dir = state.join("jobs").join(format!("{boot}-{}", pidns()));
        if let Err(e) = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir) {
            j.failed(&format!("{}: {e}", dir.display()));
            return j;
        }
        let me = unsafe { libc::getpid() };
        let sup_id = identity(me).unwrap_or(0);
        for attempt in 0..8 {
            let Some(id) = new_id(attempt == 0) else {
                j.failed("no entropy for a job id");
                return j;
            };
            let tmp = dir.join(format!(".tmp-{id}-{me}"));
            let f = match std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp) {
                Ok(f) => f,
                Err(e) => {
                    j.failed(&format!("{}: {e}", tmp.display()));
                    return j;
                }
            };
            // std opens with O_CLOEXEC: the lock never reaches the job
            if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) } != 0 {
                let _ = std::fs::remove_file(&tmp);
                j.failed(&format!("lock: {}", std::io::Error::last_os_error()));
                return j;
            }
            let header = format!(
                "{{\"v\":1,\"kind\":\"header\",\"job\":{},\"boot\":{},\"pidns\":{},\"owner\":{},\"uid\":{},\"sup\":{{\"pid\":{me},\"id\":{sup_id}}},\"argv\":{}}}\n",
                json_str(&id),
                json_str(&boot),
                json_str(&pidns()),
                json_str(owner),
                unsafe { libc::getuid() },
                json_str(&text(&joined(argv)))
            );
            let mut f = f;
            if let Err(e) = f.write_all(header.as_bytes()) {
                let _ = std::fs::remove_file(&tmp);
                j.failed(&format!("header: {e}"));
                return j;
            }
            let fin = dir.join(format!("{id}.journal"));
            match std::fs::hard_link(&tmp, &fin) {
                Ok(()) => {
                    let _ = std::fs::remove_file(&tmp);
                    crate::seam_hold("SHEEPDOG_TEST_HOLD_AFTER_LINK");
                    j.file = Some(f);
                    j.path = Some(fin);
                    j.job = Some(id);
                    crate::status::set_job(j.job.clone());
                    return j;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let _ = std::fs::remove_file(&tmp); // the id is taken: try another
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&tmp);
                    j.failed(&format!("link: {e}"));
                    return j;
                }
            }
        }
        j.failed("no free job id");
        j
    }

    /// One write; false if it failed (then the journal is dead).
    fn write(&mut self, out: &str) -> bool {
        if self.dead || self.file.is_none() {
            return false;
        }
        let r = if crate::seam_flag("SHEEPDOG_TEST_JOURNAL_WRITE_FAIL") {
            Err(std::io::Error::other("test seam"))
        } else {
            // write_all completes a short write
            self.file.as_mut().map_or(Ok(()), |f| f.write_all(out.as_bytes()))
        };
        if let Err(e) = r {
            self.failed(&format!("write: {e}"));
            return false;
        }
        true
    }

    /// Journal the members of one scan that are new: one write, before the caller signals any.
    pub fn record(&mut self, found: &[(i32, u64)]) {
        if self.file.is_none() || self.dead {
            return;
        }
        let mut out = String::new();
        let mut new = Vec::new();
        for &(p, id) in found {
            if !self.seen.insert((p, id)) {
                continue;
            }
            let (ppid, puniq) = lineage(p);
            let pid_id = ppid.and_then(identity);
            out.push_str(&format!(
                "{{\"v\":1,\"pid\":{p},\"id\":{id},\"ppid\":{},\"pid_id\":{},\"puniq\":{},\"cmd\":{}}}\n",
                num(ppid),
                num(pid_id),
                num(puniq),
                json_str(&text(&member_cmd(p)))
            ));
            new.push(p);
        }
        if out.is_empty() {
            return;
        }
        if self.write(&out) {
            for p in new {
                crate::trace(format!("journal {p}"));
            }
        }
    }

    /// Journal the root, whose command is sheepdog's own arguments (never read back from the
    /// process). Called before the root runs.
    pub fn record_root(&mut self, pid: i32, id: u64, cmd: &[OsString]) {
        if self.file.is_some() {
            self.seen.insert((pid, id));
            let me = unsafe { libc::getpid() };
            let (_, puniq) = lineage(pid);
            let line = format!(
                "{{\"v\":1,\"pid\":{pid},\"id\":{id},\"ppid\":{me},\"pid_id\":{},\"puniq\":{},\"cmd\":{},\"root\":true}}\n",
                num(identity(me)),
                num(puniq),
                json_str(&text(&joined(cmd)))
            );
            if self.write(&line) {
                crate::trace(format!("journal {pid}"));
            }
        }
        if crate::seam_flag("SHEEPDOG_TEST_KILL_AFTER_ROOT_JOURNAL") {
            unsafe { libc::raise(libc::SIGKILL) }; // raw signal site: this process (PHASE2.md §0.3)
        }
    }

    /// The root ended by itself under `--leave-strays`: say so, and keep the journal. A journal
    /// that cannot carry the mark (a write failed) is removed instead: kept unmarked, a sweep
    /// would take it for a dead job and kill the strays. So finish with `!mark_leave_strays()`.
    pub fn mark_leave_strays(&mut self) -> bool {
        self.write("{\"v\":1,\"kind\":\"leave-strays\"}\n")
    }

    /// The end: after a clean kill the journal is unlinked, then the lock released by closing
    /// the fd; otherwise it is kept for `sweep` (debug seam SHEEPDOG_TEST_KEEP_JOURNAL keeps it).
    pub fn finish(mut self, clean: bool) {
        if clean && !crate::seam_flag("SHEEPDOG_TEST_KEEP_JOURNAL") {
            if let Some(p) = &self.path {
                let _ = std::fs::remove_file(p); // before the lock is released below
            }
        }
        self.file = None; // closing the fd releases the lock
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_escapes_backslashes_and_bad_bytes_unambiguously() {
        assert_eq!(text(b"a b"), "a b");
        assert_eq!(text(b"a\xff\xfeb"), r"a\xff\xfeb");
        assert_eq!(text(br"a\xffb"), r"a\\xffb", "a real backslash is doubled");
        assert_ne!(text(b"\xff"), text(br"\xff"));
        assert_eq!(text(&[b'x'; 300]).len(), CMD_CAP);
    }

    #[test]
    fn json_strings_escape_quotes_and_controls() {
        assert_eq!(json_str("a\"b\\c\n"), r#""a\"b\\c\u000a""#);
    }
}
