//! `sheepr sweep [--owner X]` (PLAN.md §3.5; PHASE2.md §3): end what dead jobs left behind,
//! from their journals.
//!
//! Only this boot's and this pid namespace's folder is read, after its ownership checks (the state
//! directory, `jobs/` and the folder are this user's, and nobody else may write them); a journal
//! is opened without following a symlink and must be this user's file. A journal whose lock is
//! held (a live job), or that carries the leave-strays mark, or another owner tag, is skipped.
//!
//! The candidates are the journaled members whose identity still matches, plus the live closure
//! from them (the ppid chain; macOS also `puniq`, through journaled ids of members now dead and the
//! header's supervisor, whose child the root may have no line; a line's id is a link only while its
//! process is one the scan can hold, or once no live process has it: see `Fence`). A job that holds
//! this sweep's own process or one of its ancestors, or started one (its parent is a member, or on
//! macOS its `puniq` is the supervisor or a member), is skipped whole (never a signal to the
//! caller). Holding the lock, the sweep journals every closure candidate before its first signal (a
//! candidate that survives is still named for the next sweep). A candidate that is a
//! sheepr supervisor is ended first (TERM, CONT, then its grace plus the deadline); then the
//! freeze-and-kill loop, without a TERM grace. The journal is removed when every candidate is gone.
//!
//! Reading a journal is a phase-2 source: the sweep takes the wall's token first (PHASE2.md §0.1),
//! and does nothing under the phase-1 opt-out.
//!
//! Exit codes: 0 done, also when there is nothing to sweep: no state directory, no boot id, the
//! phase-1 opt-out, or a journal folder "not found" (any other error on the way to it refuses);
//! 1 refused: the state directory or its journal folder is not safe, cannot be reached or cannot
//! be listed, sheepr cannot follow its own chain of parent processes, or (Linux, checked first)
//! no /proc of its own pid namespace is mounted; 2 usage error; 125 a job's kill deadline passed
//! with members alive, or sheepr failed inside (the kill, or a panic).

use crate::kill::{end_supervisors, is_sheepr, parent, protected, Proved};
use crate::{kill_failed, kill_tree, say, signal, trace, KillOpts};
use sheepr::ident::same;
use sheepr::json::{self, Json};
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

/// One journaled member.
pub struct Line {
    pub pid: i32,
    pub id: u64,
    pub ppid: Option<i32>,
    pub pid_id: Option<u64>,
    pub puniq: Option<u64>,
}

/// A journal, open and locked by this process.
pub struct Journal {
    pub path: PathBuf,
    file: std::fs::File,
    pub job: String,
    /// the header's boot id and pid namespace: a journal from another boot or namespace is
    /// never swept, whichever folder it is in (the folder only narrows the read)
    pub boot: String,
    pub pidns: String,
    pub owner: String,
    /// the header's supervisor (pid, identity), if it names one
    pub sup: Option<(i32, u64)>,
    pub members: Vec<Line>,
    pub leave_strays: bool,
    /// the file does not end in a newline (a line cut short): an append starts a new line
    torn: bool,
}

/// Why a journal was not swept.
pub enum Skip {
    Live,
    Unsafe(String),
    Unreadable(String),
    /// its job chose to keep its strays (`--leave-strays`): not a problem, so `sweep` and the
    /// auto-sweep never say it (`kill <job>` still names it in its refusal)
    Kept(String),
}

/// A folder is safe to act on: a real directory (not a symlink), this user's, and nobody else
/// may write it.
pub fn safe_dir(p: &Path) -> Result<(), String> {
    let m = std::fs::symlink_metadata(p).map_err(|e| format!("{}: {e}", p.display()))?;
    if !m.file_type().is_dir() {
        return Err(format!("{} is not a directory", p.display()));
    }
    if m.uid() != unsafe { libc::geteuid() } {
        return Err(format!("{} belongs to another user", p.display()));
    }
    if m.mode() & 0o022 != 0 {
        return Err(format!("{} can be written by other users", p.display()));
    }
    Ok(())
}

fn num(j: &Json, k: &str) -> Option<f64> {
    j.get(k).and_then(Json::num)
}

/// Open, check and lock the journal at `path`. Only complete (newline-terminated) lines that
/// parse count; the first must be the header.
pub fn open(path: &Path) -> Result<Journal, Skip> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        // gone between the listing and this open: a job that ended cleanly (or another sweep's),
        // as a name that no longer matches the lock below
        .map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { Skip::Live } else { Skip::Unreadable(format!("{e}")) })?;
    let meta = file.metadata().map_err(|e| Skip::Unreadable(format!("{e}")))?;
    if meta.uid() != unsafe { libc::geteuid() } {
        return Err(Skip::Unsafe("the journal belongs to another user".into()));
    }
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(Skip::Live); // held: a live job (any other lock error counts as live too)
    }
    // the name must still be the file this lock is on (a clean end unlinks before it unlocks)
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.ino() == meta.ino() && m.dev() == meta.dev() => {}
        _ => return Err(Skip::Live),
    }
    let mut text = Vec::new();
    std::io::Read::read_to_end(&mut &file, &mut text).map_err(|e| Skip::Unreadable(format!("{e}")))?;
    let torn = !text.is_empty() && text.last() != Some(&b'\n');
    let body = &text[..text.iter().rposition(|&c| c == b'\n').map_or(0, |i| i + 1)];
    let mut lines = String::from_utf8_lossy(body).lines().filter_map(|l| json::parse(l).ok()).collect::<Vec<_>>().into_iter();
    let h = lines.next().filter(|h| h.get("kind").and_then(Json::str) == Some("header")).ok_or_else(|| Skip::Unreadable("no header".into()))?;
    let mut j = Journal {
        path: path.to_path_buf(),
        file,
        job: h.get("job").and_then(Json::str).unwrap_or("").to_string(),
        boot: h.get("boot").and_then(Json::str).unwrap_or("").to_string(),
        pidns: h.get("pidns").and_then(Json::str).unwrap_or("").to_string(),
        owner: h.get("owner").and_then(Json::str).unwrap_or("default").to_string(),
        sup: h.get("sup").and_then(|s| Some((num(s, "pid")? as i32, num(s, "id")? as u64))),
        members: Vec::new(),
        leave_strays: false,
        torn,
    };
    for l in lines {
        if l.get("kind").and_then(Json::str) == Some("leave-strays") {
            j.leave_strays = true;
        } else if let (Some(pid), Some(id)) = (num(&l, "pid"), num(&l, "id")) {
            j.members.push(Line {
                pid: pid as i32,
                id: id as u64,
                ppid: num(&l, "ppid").map(|n| n as i32),
                pid_id: num(&l, "pid_id").map(|n| n as u64),
                puniq: num(&l, "puniq").map(|n| n as u64),
            });
        }
    }
    Ok(j)
}

/// `open`, then the fences every reader keeps (PHASE2.md §3.1): the header's boot id and pid
/// namespace are this ones, and the journal has no leave-strays mark.
pub fn open_fenced(path: &Path) -> Result<Journal, Skip> {
    let j = open(path)?;
    if Some(j.boot.as_str()) != crate::journal::boot_id().as_deref() || j.pidns != crate::journal::pidns() {
        return Err(Skip::Unsafe("another boot or pid namespace".into()));
    }
    if j.leave_strays {
        return Err(Skip::Kept("its job left its strays on purpose (--leave-strays)".into()));
    }
    Ok(j)
}

/// A journal's header (job, owner, supervisor), read without its lock: never through a symlink,
/// only this user's file, and only with this boot's and pid namespace's header.
pub fn read_header(path: &Path) -> Option<(String, (i32, u64))> {
    let f = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW).open(path).ok()?;
    if f.metadata().ok()?.uid() != unsafe { libc::geteuid() } {
        return None;
    }
    let mut text = String::new();
    std::io::Read::read_to_string(&mut &f, &mut text).ok()?;
    let h = json::parse(text.lines().next()?).ok()?;
    if h.get("kind").and_then(Json::str) != Some("header")
        || h.get("boot").and_then(Json::str) != crate::journal::boot_id().as_deref()
        || h.get("pidns").and_then(Json::str) != Some(crate::journal::pidns().as_str())
    {
        return None;
    }
    let s = h.get("sup")?;
    Some((h.get("job").and_then(Json::str)?.to_string(), (num(s, "pid")? as i32, num(s, "id")? as u64)))
}

/// Which ids a journal gives as `puniq` links (macOS; Linux has no `puniq`, so none there), read
/// once, before the reader's first scan. A line's id is a link when its process is one the scan
/// can hold (alive at the line's pid, this user's, above pid 1), or when no live process has the
/// id, by a credible list. So a member alive at the read and gone before the scan still links its
/// orphans, and a forged or corrupt line that names launchd (pid 1, id 1, the original parent of
/// hundreds of this user's processes), another user's process, or a process at another pid than
/// its own brings in none of its children. (A line that names this user's live process at its own
/// pid makes it a member, as the journal is the record.) Every reader of a dead job's lineage uses
/// it: `Journal::proved` and `kill`'s journal subtree.
pub(crate) struct Fence {
    /// every live process's id, of any user; None when the list is not credible
    live: Option<HashSet<u64>>,
    /// what the scan can hold: this user's live processes above pid 1
    held: HashSet<(i32, u64)>,
}

/// The (pid, id) pairs of a process list that the scan can hold (`kill::scan_can_hold`).
#[cfg(target_os = "macos")]
fn held_of(procs: Vec<crate::kill::Proc>, uid: u32) -> HashSet<(i32, u64)> {
    procs.into_iter().filter(|p| crate::kill::scan_can_hold(p, uid)).map(|p| (p.pid, p.id)).collect()
}

impl Fence {
    pub(crate) fn now() -> Fence {
        #[cfg(target_os = "macos")]
        let f = {
            // The order is load-bearing: the held set first, then the live ids. A member that ends
            // between the two reads is then held, and gone at the second read: a link either way.
            // Read the other way round, it would be live at the first read and missing from the
            // held set, so no link, and its orphans would be lost.
            let held = held_of(crate::macos::procs(), unsafe { libc::getuid() });
            // debug seam: a pause between the two reads
            crate::seam_sleep("SHEEPR_TEST_SLEEP_BETWEEN_FENCE_READS_MS");
            let live = crate::macos::live_ids();
            Fence { live, held }
        };
        #[cfg(not(target_os = "macos"))]
        let f = Fence { live: None, held: HashSet::new() };
        // debug seam: a pause after the read, before the caller's first scan
        crate::seam_sleep("SHEEPR_TEST_SLEEP_AFTER_FENCE_MS");
        f
    }

    /// the id of a line that names (`pid`, `id`) may be a link
    pub(crate) fn link(&self, pid: i32, id: u64) -> bool {
        self.held.contains(&(pid, id)) || self.gone(id)
    }

    /// no live process has `id`, by a credible list
    pub(crate) fn gone(&self, id: u64) -> bool {
        self.live.as_ref().is_some_and(|l| id != 0 && !l.contains(&id))
    }
}

impl Journal {
    /// The proved set a sweep of this dead job starts from (also `ps` and `kill --dry-run` of
    /// it): the journaled members whose identity still matches, and as `puniq` links (through
    /// the `Fence`) the ids of the members and of the header's supervisor. The supervisor starts
    /// one process, the root, so its id finds a root that a supervisor SIGKILLed between the spawn
    /// and the root's line never journaled.
    pub(crate) fn proved(&self, protected: &[(i32, u64)]) -> Proved {
        let fence = Fence::now();
        let known: HashMap<i32, u64> = self.members.iter().filter(|m| same(m.pid, m.id)).map(|m| (m.pid, m.id)).collect();
        let mut ever: HashSet<u64> = self.members.iter().filter(|m| fence.link(m.pid, m.id)).map(|m| m.id).collect();
        // the supervisor of a dead job is gone (a live one holds the lock)
        ever.extend(self.sup.map(|(_, id)| id).filter(|&id| fence.gone(id)));
        Proved { known, ever, protected: protected.to_vec() }
    }

    /// Append lines for `new` (closure candidates no line names yet), before any signal to them.
    fn journal_closure(&mut self, new: &[(i32, u64)]) {
        if new.is_empty() {
            return;
        }
        let mut out = String::new();
        if self.torn {
            out.push('\n');
            self.torn = false;
        }
        for &(p, id) in new {
            out.push_str(&format!("{{\"v\":1,\"pid\":{p},\"id\":{id},\"ppid\":null,\"pid_id\":null,\"puniq\":null,\"cmd\":\"\",\"closure\":true}}\n"));
        }
        // the append goes to the file this lock is on (checked by inode)
        let ok = std::fs::OpenOptions::new().append(true).custom_flags(libc::O_NOFOLLOW).open(&self.path).and_then(|mut f| {
            let (a, b) = (f.metadata()?, self.file.metadata()?);
            if a.ino() != b.ino() || a.dev() != b.dev() {
                return Err(std::io::Error::other("the journal was replaced"));
            }
            f.write_all(out.as_bytes())
        });
        match ok {
            Ok(()) => {
                for &(p, _) in new {
                    trace(format!("journal {p}"));
                }
            }
            Err(e) => crate::note(format!("sweep: could not journal the closure of {}: {e}", self.job)),
        }
    }
}

/// How a sweep of one journal ended.
pub enum Outcome {
    /// every candidate is gone; the journal is removed
    Swept(usize),
    /// skipped whole, with why; the journal is kept
    Skipped(String),
    /// members are still alive at the deadline; the journal is kept
    Deadline(i32),
}

/// The members named by a journal, read without its lock (a live job's: the auto-sweep leaves
/// them alone). Only this user's file, never through a symlink.
pub(crate) fn peek(path: &Path) -> Vec<(i32, u64)> {
    let Ok(f) = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW).open(path) else { return Vec::new() };
    if f.metadata().map_or(true, |m| m.uid() != unsafe { libc::geteuid() }) {
        return Vec::new();
    }
    let mut text = String::new();
    let _ = std::io::Read::read_to_string(&mut &f, &mut text);
    text.lines().filter_map(|l| json::parse(l).ok()).filter_map(|l| Some((num(&l, "pid")? as i32, num(&l, "id")? as u64))).collect()
}

/// How a sweep treats live supervisors and live jobs.
pub enum Mode<'a> {
    /// `sheepr sweep`: a candidate supervisor is ended first
    Explicit,
    /// the auto-sweep: a live supervisor and its set, and anything a live job's journal names,
    /// are left for an explicit sweep; no grace; a 500 ms deadline, and what a pass that misses
    /// it stopped is continued
    Auto { live_named: &'a HashSet<(i32, u64)> },
}

/// Is `c` deferred by the auto-sweep: in `held` (a live supervisor, anything a live job's journal
/// names, and everything this journal's lineage puts under one of those), or with a live parent
/// chain that reaches one, or (macOS) responsible to a live supervisor?
fn deferred(c: (i32, u64), sups: &[i32], held: &HashSet<(i32, u64)>) -> bool {
    if held.contains(&c) || sups.contains(&c.0) {
        return true;
    }
    let held_pids: HashSet<i32> = held.iter().map(|&(p, _)| p).collect();
    let mut p = parent(c.0);
    let mut n = 0;
    while let Some(q) = p.filter(|&q| q > 1 && n < 4096) {
        if sups.contains(&q) || held_pids.contains(&q) {
            return true;
        }
        p = parent(q);
        n += 1;
    }
    #[cfg(target_os = "macos")]
    if crate::macos::responsible_pid(c.0).is_some_and(|r| sups.contains(&r)) {
        return true;
    }
    false
}

/// The auto-sweep's held set: the live supervisors among `set`, what live jobs' journals name,
/// and every line of this journal whose lineage (parent pid and identity, or macOS `puniq`)
/// leads to one of those.
fn held_set(j: &Journal, set: &[(i32, u64)], live_named: &HashSet<(i32, u64)>) -> HashSet<(i32, u64)> {
    let mut held: HashSet<(i32, u64)> = set.iter().copied().filter(|&(p, _)| is_sheepr(p)).collect();
    held.extend(live_named.iter().copied());
    loop {
        let ids: HashSet<u64> = held.iter().map(|&(_, id)| id).collect();
        let more: Vec<(i32, u64)> = j
            .members
            .iter()
            .filter(|m| !held.contains(&(m.pid, m.id)))
            .filter(|m| m.ppid.zip(m.pid_id).is_some_and(|pp| held.contains(&pp)) || m.puniq.is_some_and(|u| ids.contains(&u)))
            .map(|m| (m.pid, m.id))
            .collect();
        if more.is_empty() {
            return held;
        }
        held.extend(more);
    }
}

/// (macOS) The original parent's uniqueid of `p`, while it is still the process `id`; a root
/// that only the header's supervisor ties to the job is such a child (Linux: none).
fn puniq_of(p: i32, id: u64) -> Option<u64> {
    #[cfg(target_os = "macos")]
    return crate::macos::uniq(p).filter(|&(u, _)| u == id).map(|(_, pu)| pu);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (p, id);
        None
    }
}

/// Why a sweep of this dead job skips it whole (never a signal to the caller, and the journal
/// stays), or None: the job holds this sweep's own process or one of its ancestors (`protected`),
/// or started one: its parent is in `set` (the first scan's), or (macOS) its original parent is a
/// link of `proved` (the header's supervisor, or a member live or dead: a root only the header
/// ties to the job, a terminal a member started). The sweep, `kill j-JOBID` and the dry runs of
/// it (`kill --dry-run`, `ps`) all ask this, so they agree.
pub(crate) fn skip_reason(j: &Journal, protected: &[(i32, u64)], proved: &Proved, set: &[(i32, u64)]) -> Option<String> {
    if let Some(&(p, _)) = protected.iter().find(|&&(p, id)| j.members.iter().any(|m| m.pid == p && m.id == id)) {
        return Some(format!("it holds pid {p}, this sheepr or one of its ancestors"));
    }
    let members: HashSet<i32> = set.iter().map(|&(p, _)| p).collect();
    if let Some(&(p, _)) = protected.iter().find(|&&(p, _)| parent(p).is_some_and(|q| members.contains(&q))) {
        return Some(format!("pid {p}, this sheepr or one of its ancestors, is a child of one of its members"));
    }
    if let Some(&(p, _)) = protected.iter().find(|&&(p, id)| puniq_of(p, id).is_some_and(|u| proved.ever.contains(&u))) {
        return Some(format!("pid {p}, this sheepr or one of its ancestors, was started by this job (by its sheepr or one of its processes)"));
    }
    None
}

/// Sweep one open journal in `mode`. `protected`: this process and its ancestors.
pub fn sweep_job_as(mut j: Journal, protected: &[(i32, u64)], mode: Mode) -> Outcome {
    let explicit = matches!(mode, Mode::Explicit);
    // the scan sends nothing: a job that holds or started this sweep's process or an ancestor is
    // then skipped whole
    let mut proved = j.proved(protected);
    let set = proved.scan();
    if let Some(why) = skip_reason(&j, protected, &proved, &set) {
        return Outcome::Skipped(why);
    }
    let mut named: HashSet<(i32, u64)> = j.members.iter().map(|m| (m.pid, m.id)).collect();
    // every candidate is journaled before its first signal, also one a later scan finds
    let mut journal_new = |j: &mut Journal, found: &[(i32, u64)]| {
        let new: Vec<(i32, u64)> = found.iter().copied().filter(|m| !named.contains(m)).collect();
        j.journal_closure(&new);
        named.extend(new);
    };
    journal_new(&mut j, &set);
    let mut opts = KillOpts::from_env();
    if let Mode::Auto { live_named } = mode {
        // a pass is bounded (500 ms, unless the debug deadline seam is set)
        if crate::seam_ms("SHEEPR_TEST_DEADLINE_MS").is_none() {
            opts.deadline = std::time::Duration::from_millis(500);
        }
        let sups: Vec<i32> = set.iter().filter(|&&(p, _)| is_sheepr(p)).map(|&(p, _)| p).collect();
        let held_by = held_set(&j, &set, live_named);
        let held: Vec<(i32, u64)> = set.iter().copied().filter(|&c| deferred(c, &sups, &held_by)).collect();
        let n = set.len() - held.len();
        let mut scan = || {
            let found = proved.scan();
            journal_new(&mut j, &found);
            found.into_iter().filter(|&c| !deferred(c, &sups, &held_by)).collect::<Vec<_>>()
        };
        let initial: HashMap<i32, u64> = scan().into_iter().collect();
        let r = kill_tree(&opts, &mut scan, || {}, || None, signal, initial);
        return match r {
            Ok(()) if held.is_empty() => {
                let _ = std::fs::remove_file(&j.path);
                Outcome::Swept(n)
            }
            Ok(()) => {
                crate::status::add_note(&format!("deferred: job {} holds a live supervisor or a live job's processes; run sheepr sweep", j.job));
                crate::note(format!("deferred {}", j.job));
                Outcome::Swept(n)
            }
            Err(crate::KillError::Deadline(alive)) => {
                // continue what this pass stopped: a partial pass must leave nothing stopped
                for p in &alive {
                    if let Some(&id) = proved.known.get(p) {
                        let _ = signal(*p, id, libc::SIGCONT);
                    }
                }
                crate::status::add_note(&format!("partial: job {} still has {} process(es) alive after its 500 ms pass", j.job, alive.len()));
                Outcome::Deadline(0)
            }
            Err(crate::KillError::Internal) => Outcome::Deadline(125),
        };
    }
    if explicit {
        let sups: Vec<(i32, u64)> = set.iter().copied().filter(|&(p, _)| is_sheepr(p)).collect();
        if !sups.is_empty() {
            end_supervisors(&sups, opts.deadline, &mut proved);
        }
    }
    // what the supervisor wait's scans found goes into the kill's initial set: journaled now
    // (the kill's own scan may no longer return it, e.g. after an exec that changed its euid)
    let seen: Vec<(i32, u64)> = proved.known.iter().map(|(&p, &id)| (p, id)).collect();
    journal_new(&mut j, &seen);
    opts.hold = vec![libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGQUIT];
    let initial = proved.known.clone();
    let n = set.len();
    let scan = || {
        let found = proved.scan();
        journal_new(&mut j, &found);
        found
    };
    match kill_tree(&opts, scan, || {}, || None, signal, initial) {
        Ok(()) => {
            let _ = std::fs::remove_file(&j.path); // before the lock is released (j dropped)
            Outcome::Swept(n)
        }
        Err(e) => Outcome::Deadline(kill_failed(e)),
    }
}

/// Whether `dir` is absent, so there is nothing to sweep. Only "not found" is: a folder that
/// exists but cannot be reached or read is never taken as empty (issue #15); `safe_dir` and
/// `journals` then name it.
fn absent(dir: &Path) -> bool {
    matches!(std::fs::symlink_metadata(dir), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
}

/// The journals in `dir`, sorted. A folder that cannot be listed (or a listing that fails part
/// way) is an error, never an empty list (issue #15). A journal file that cannot be read is not:
/// `open` makes it `Skip::Unreadable` later, which is said.
fn journals(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let cannot = |e: std::io::Error| format!("cannot list {}: {e}", dir.display());
    let mut v = Vec::new();
    for e in std::fs::read_dir(dir).map_err(cannot)? {
        let p = e.map_err(cannot)?.path();
        if p.extension().is_some_and(|e| e == "journal") {
            v.push(p);
        }
    }
    v.sort();
    Ok(v)
}

/// Where `sweep` and the auto-sweep look for registration folders (issue #20): only where this
/// sheepr's own listener would put its folder (`register::base_for`, from TMPDIR). A folder
/// elsewhere is a sheepr's of another environment, which reaps it there. A debug build (the
/// tests) looks only with the seam SHEEPR_TEST_REAP_LISTENERS=1, and only in a folder its cell
/// marked as its own (the regular file `.sheepr-test-reap`: the operator's temp folders never hold
/// it, so no cell aims a sweep at them), and never in /tmp itself (`register::refused_in_debug`;
/// the seam SHEEPR_TEST_REFUSE_AS_TMP adds a cell's folder to what it refuses, so a cell can show
/// the refusal applies without aiming a sweep at /tmp).
#[cfg(target_os = "macos")]
fn reap_bases() -> Vec<PathBuf> {
    let base = crate::register::base_for(std::env::var_os("TMPDIR"));
    if cfg!(debug_assertions) {
        let marked = std::fs::symlink_metadata(base.join(".sheepr-test-reap")).is_ok_and(|m| m.is_file());
        let also = std::env::var_os("SHEEPR_TEST_REFUSE_AS_TMP").map(PathBuf::from);
        let ok = crate::seam_flag("SHEEPR_TEST_REAP_LISTENERS") && marked && !crate::register::refused_in_debug(&base, also.as_deref());
        return if ok { vec![base] } else { Vec::new() };
    }
    vec![base]
}

/// The auto-sweep before a `sheepr run` (PHASE2.md §3.5-§3.7): the same owner's dead jobs,
/// within 200 ms between journals. A state folder it cannot use, and a journal it cannot read
/// or will not act on for safety (another user's, another boot's header), is a note that names
/// it (issue #15); another owner's journal and a kept one are skipped silently, as they should
/// be; a deadline missed is `partial`. It takes the wall's
/// token only when there is a journal to open: an empty state produces no fact.
pub fn auto(owner: &str, quiet: bool) {
    // the registration folders of sheeprs that are gone (issue #20; macOS: Linux has no
    // registration socket), whatever the state
    #[cfg(target_os = "macos")]
    {
        let ms = crate::seam_ms("SHEEPR_TEST_REAP_MS").unwrap_or(50);
        let n = crate::register::reap(&reap_bases(), std::time::Instant::now() + std::time::Duration::from_millis(ms));
        if n > 0 {
            crate::note(format!("auto-sweep removed {n} registration folder(s)"));
        }
    }
    let Some(state) = crate::state::resolve(cfg!(debug_assertions), |k| std::env::var_os(k), |p| p.exists()) else { return };
    let Some(dir) = folder(&state) else { return };
    if absent(&dir) {
        return;
    }
    for p in [state.clone(), state.join("jobs"), dir.clone()] {
        if let Err(why) = safe_dir(&p) {
            crate::status::add_note(&format!("auto-sweep skipped: {why}"));
            return;
        }
    }
    let files = match journals(&dir) {
        Ok(f) => f,
        Err(why) => {
            crate::status::add_note(&format!("auto-sweep skipped: {why}"));
            return;
        }
    };
    if files.is_empty() {
        return;
    }
    let Some(_token) = crate::wall::gate() else { return };
    let Ok(protected) = protected() else {
        crate::status::add_note("auto-sweep skipped: sheepr cannot follow its own chain of parent processes");
        return;
    };
    let start = std::time::Instant::now();
    let mut opened = Vec::new();
    let mut live_named: HashSet<(i32, u64)> = HashSet::new();
    for f in files {
        match open_fenced(&f) {
            Ok(j) => opened.push(j),
            Err(Skip::Live) => live_named.extend(peek(&f)),
            Err(Skip::Unsafe(why)) | Err(Skip::Unreadable(why)) => crate::status::add_note(&format!("auto-sweep skipped {}: {why}", f.display())),
            Err(Skip::Kept(_)) => {}
        }
    }
    let (mut swept, mut killed) = (0usize, 0usize);
    for j in opened {
        if start.elapsed() > std::time::Duration::from_millis(200) {
            crate::status::add_note("partial: the auto-sweep's 200 ms budget ran out; the rest waits for the next run");
            break;
        }
        if j.owner != owner {
            continue;
        }
        let job = j.job.clone();
        match sweep_job_as(j, &protected, Mode::Auto { live_named: &live_named }) {
            Outcome::Swept(n) => {
                swept += 1;
                killed += n;
            }
            Outcome::Skipped(why) => crate::status::add_note(&format!("auto-sweep skipped job {job}: {why}")),
            Outcome::Deadline(_) => {}
        }
    }
    if swept > 0 && killed > 0 && !quiet {
        say!("sheepr: swept {swept} dead job{} before the command ({killed} process{} ended).", if swept == 1 { "" } else { "s" }, if killed == 1 { "" } else { "es" });
    }
}

/// This boot's and pid namespace's journal folder in `state`.
pub fn folder(state: &Path) -> Option<PathBuf> {
    Some(state.join("jobs").join(format!("{}-{}", crate::journal::boot_id()?, crate::journal::pidns())))
}

pub(crate) const USAGE: &str = "sheepr sweep [--owner NAME]";

pub fn main(args: &[OsString]) -> i32 {
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        crate::block_all_but_faults(&mut set);
        for s in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGQUIT] {
            libc::sigdelset(&mut set, s);
        }
        libc::sigprocmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
    }
    let mut owner = "default".to_string();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_bytes() {
            b"--owner" if i + 1 < args.len() => {
                owner = args[i + 1].to_string_lossy().into_owned();
                i += 2;
            }
            a => {
                let w = crate::shown(&args[i]);
                crate::fail!("sheepr: sweep: {}", match a {
                    b"--owner" => "--owner needs a value: a name.".to_string(),
                    a if a.starts_with(b"-") => format!("unknown option {w}."),
                    _ => format!("unexpected argument {w}."),
                });
                crate::say!("usage: {USAGE}");
                return 2;
            }
        }
    }
    #[cfg(target_os = "linux")]
    if let Some(why) = crate::linux::proc_problem() {
        crate::fail!("sheepr: {why}, so sweep cannot tell which processes are which. Nothing was signalled.");
        return 1;
    }
    // the registration folders of sheeprs that are gone (issue #20; macOS: Linux has no
    // registration socket), whatever the state
    #[cfg(target_os = "macos")]
    {
        let n = crate::register::reap(&reap_bases(), std::time::Instant::now() + std::time::Duration::from_secs(5));
        if n > 0 {
            say!("sheepr: removed {n} registration folder{} of {} that {} gone.", if n == 1 { "" } else { "s" }, if n == 1 { "a sheepr" } else { "sheeprs" }, if n == 1 { "is" } else { "are" });
        }
    }
    let Some(state) = crate::state::resolve(cfg!(debug_assertions), |k| std::env::var_os(k), |p| p.exists()) else {
        crate::note("state-unset".into());
        return 0;
    };
    let Some(dir) = folder(&state) else { return 0 };
    if absent(&dir) {
        return 0;
    }
    for p in [state.clone(), state.join("jobs"), dir.clone()] {
        if let Err(why) = safe_dir(&p) {
            crate::fail!("sheepr: refusing to sweep: {why}. Nothing was signalled.");
            return 1;
        }
    }
    let Some(_token) = crate::wall::gate() else { return 0 };
    let protected = match protected() {
        Ok(v) => v,
        Err(link) => {
            crate::fail!("sheepr: refusing to sweep: sheepr cannot follow its own chain of parent processes at pid {link}, so it cannot rule out a job that holds one of them. Nothing was signalled.");
            return 1;
        }
    };
    let files = match journals(&dir) {
        Ok(f) => f,
        Err(why) => {
            crate::fail!("sheepr: refusing to sweep: {}. Nothing was signalled.", crate::kill::clean(&why));
            return 1;
        }
    };
    let (mut swept, mut killed, mut code) = (0usize, 0usize, 0);
    for f in files {
        let j = match open_fenced(&f) {
            Ok(j) => j,
            Err(Skip::Live) => continue,
            Err(Skip::Unsafe(why)) | Err(Skip::Unreadable(why)) => {
                // said, not only noted: the trace exists only in a debug build (issue #15)
                say!("sheepr: could not sweep {}: {why}. Its processes, if any, were not ended.", crate::kill::clean(&f.display().to_string()));
                crate::note(format!("sweep skipped {}: {why}", f.display()));
                continue;
            }
            Err(Skip::Kept(why)) => {
                crate::note(format!("sweep skipped {}: {why}", f.display()));
                continue;
            }
        };
        if j.owner != owner {
            continue;
        }
        let job = j.job.clone();
        match sweep_job_as(j, &protected, Mode::Explicit) {
            Outcome::Swept(n) => {
                swept += 1;
                killed += n;
            }
            Outcome::Skipped(why) => {
                say!("sheepr: skipped job {job}: {why}.");
                crate::note(format!("sweep skipped {job}: {why}"));
            }
            Outcome::Deadline(c) => code = code.max(c),
        }
    }
    if swept > 0 {
        say!("sheepr: swept {swept} dead job{} ({killed} process{} ended).", if swept == 1 { "" } else { "s" }, if killed == 1 { "" } else { "es" });
    }
    code
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use sheepr::ident::identity;

    /// The fence links what the scan can hold and a gone id, and nothing else: this process (this
    /// user's, alive at its pid) and an id no process has are links; launchd (pid 1), a root-owned
    /// process above pid 1 (another user's, where `ps` gives the uid), and this process's id at
    /// another pid are not.
    #[test]
    fn the_fence_links_only_what_the_scan_can_hold_or_a_gone_id() {
        let me = unsafe { libc::getpid() };
        let my_id = identity(me).unwrap();
        let out = std::process::Command::new("ps").args(["-Ao", "pid=,uid="]).output().unwrap();
        let root_proc = String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| {
                let mut w = l.split_whitespace();
                Some((w.next()?.parse::<i32>().ok()?, w.next()?.parse::<u32>().ok()?))
            })
            .filter(|&(p, uid)| p > 1 && uid == 0)
            .find_map(|(p, _)| identity(p).map(|id| (p, id)))
            .expect("a root-owned process above pid 1");
        let f = Fence::now();
        assert!(f.link(me, my_id), "this process");
        assert!(f.link(999_998, 1 << 40), "an id no process has");
        assert!(!f.link(1, identity(1).unwrap()), "launchd");
        assert!(!f.link(root_proc.0, root_proc.1), "root's process {root_proc:?}");
        assert!(!f.link(999_998, my_id), "this process's id at another pid");
    }

    /// The fence's held set is what the scan can hold, also for root: never pid 1, never another
    /// user's process (as a user, `procs()` reads only this user's processes, so only a list built
    /// here shows the filter at work).
    #[test]
    fn the_fences_held_set_is_what_the_scan_can_hold() {
        let p = |pid: i32, uid: u32| crate::kill::Proc { pid, ppid: 1, uid, id: pid as u64 + 1000, puniq: None, sid: pid, pgid: pid, resp: None };
        let list = || vec![p(1, 0), p(500, 501), p(600, 0)];
        let root = held_of(list(), 0);
        assert!(!root.contains(&(1, 1001)), "launchd, for root");
        assert!(!root.contains(&(500, 1500)), "another user's process, for root");
        assert!(root.contains(&(600, 1600)), "root's own process, for root");
        let user = held_of(list(), 501);
        assert!(user.contains(&(500, 1500)), "this user's process");
        assert!(!user.contains(&(600, 1600)) && !user.contains(&(1, 1001)), "root's processes, for a user");
    }
}
