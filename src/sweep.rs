//! `sheepdog sweep [--owner X]` (PLAN.md §3.5; PHASE2.md §3): end what dead jobs left behind,
//! from their journals.
//!
//! Only this boot's and this pid namespace's folder is read, after its ownership checks (the state
//! directory, `jobs/` and the folder are this user's, and nobody else may write them); a journal
//! is opened without following a symlink and must be this user's file. A journal whose lock is
//! held (a live job), or that carries the leave-strays mark, or another owner tag, is skipped.
//!
//! The candidates are the journaled members whose identity still matches, plus the live closure
//! from them (the ppid chain; macOS also `puniq`, through journaled ids of members now dead). A job
//! that holds this sweep's own process or one of its ancestors is skipped whole (never a signal
//! to the caller). Holding the lock, the sweep journals every closure candidate before its first
//! signal (a candidate that survives is still named for the next sweep). A candidate that is a
//! sheepdog supervisor is ended first (TERM, CONT, then its grace plus the deadline); then the
//! freeze-and-kill loop, without a TERM grace. The journal is removed when every candidate is gone.
//!
//! Reading a journal is a phase-2 source: the sweep takes the wall's token first (PHASE2.md §0.1),
//! and does nothing under the phase-1 opt-out.
//!
//! Exit codes: 0 done (also: nothing to sweep); 1 refused (the state directory is not safe);
//! 2 usage error; 125 a job's kill deadline passed with members alive.

use crate::kill::{end_supervisors, is_sheepdog, parent, protected, Proved};
use crate::{kill_failed, kill_tree, say, signal, trace, KillOpts};
use sheepdog::ident::same;
use sheepdog::json::{self, Json};
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
    pub sup: (i32, u64),
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
        .map_err(|e| Skip::Unreadable(format!("{e}")))?;
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
    let text = std::fs::read(path).map_err(|e| Skip::Unreadable(format!("{e}")))?;
    let torn = !text.is_empty() && text.last() != Some(&b'\n');
    let body = &text[..text.iter().rposition(|&c| c == b'\n').map_or(0, |i| i + 1)];
    let mut lines = String::from_utf8_lossy(body).lines().filter_map(|l| json::parse(l).ok()).collect::<Vec<_>>().into_iter();
    let h = lines.next().filter(|h| h.get("kind").and_then(Json::str) == Some("header")).ok_or_else(|| Skip::Unreadable("no header".into()))?;
    let sup = h.get("sup").and_then(|s| Some((num(s, "pid")? as i32, num(s, "id")? as u64))).unwrap_or((0, 0));
    let mut j = Journal {
        path: path.to_path_buf(),
        file,
        job: h.get("job").and_then(Json::str).unwrap_or("").to_string(),
        boot: h.get("boot").and_then(Json::str).unwrap_or("").to_string(),
        pidns: h.get("pidns").and_then(Json::str).unwrap_or("").to_string(),
        owner: h.get("owner").and_then(Json::str).unwrap_or("default").to_string(),
        sup,
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

impl Journal {
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

/// Sweep one open journal. `protected`: this process and its ancestors.
pub fn sweep_job(mut j: Journal, protected: &[(i32, u64)], explicit: bool) -> Outcome {
    // a job that holds this sweep's own process or an ancestor is never touched
    if let Some(&(p, _)) = protected.iter().find(|&&(p, id)| j.members.iter().any(|m| m.pid == p && m.id == id)) {
        return Outcome::Skipped(format!("it holds pid {p}, this sweep or one of its ancestors"));
    }
    let known: HashMap<i32, u64> = j.members.iter().filter(|m| same(m.pid, m.id)).map(|m| (m.pid, m.id)).collect();
    let ever: HashSet<u64> = j.members.iter().map(|m| m.id).collect();
    let mut proved = Proved { known, ever, protected: protected.to_vec() };
    let set = proved.scan();
    let members: HashSet<i32> = set.iter().map(|&(p, _)| p).collect();
    if let Some(&(p, _)) = protected.iter().find(|&&(p, _)| parent(p).is_some_and(|q| members.contains(&q))) {
        return Outcome::Skipped(format!("pid {p}, this sweep or one of its ancestors, is a child of one of its members"));
    }
    let named: HashSet<(i32, u64)> = j.members.iter().map(|m| (m.pid, m.id)).collect();
    let new: Vec<(i32, u64)> = set.iter().copied().filter(|m| !named.contains(m)).collect();
    j.journal_closure(&new);
    let mut opts = KillOpts::from_env();
    if explicit {
        let sups: Vec<(i32, u64)> = set.iter().copied().filter(|&(p, _)| is_sheepdog(p)).collect();
        if !sups.is_empty() {
            end_supervisors(&sups, opts.deadline, &mut proved);
        }
    }
    opts.hold = vec![libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGQUIT];
    let initial = proved.known.clone();
    let n = set.len();
    match kill_tree(&opts, || proved.scan(), || {}, || None, signal, initial) {
        Ok(()) => {
            let _ = std::fs::remove_file(&j.path); // before the lock is released (j dropped)
            Outcome::Swept(n)
        }
        Err(e) => Outcome::Deadline(kill_failed(e)),
    }
}

/// This boot's and pid namespace's journal folder in `state`.
pub fn folder(state: &Path) -> Option<PathBuf> {
    Some(state.join("jobs").join(format!("{}-{}", crate::journal::boot_id()?, crate::journal::pidns())))
}

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
            _ => {
                say!("usage: sheepdog sweep [--owner NAME]");
                return 2;
            }
        }
    }
    #[cfg(target_os = "linux")]
    if let Some(why) = crate::linux::proc_problem() {
        say!("sheepdog: {why}, so sweep cannot tell which processes are which. Nothing was signalled.");
        return 1;
    }
    let Some(state) = crate::state::resolve(cfg!(debug_assertions), |k| std::env::var_os(k), |p| p.exists()) else {
        crate::note("state-unset".into());
        return 0;
    };
    let Some(dir) = folder(&state) else { return 0 };
    if !dir.exists() {
        return 0;
    }
    for p in [state.clone(), state.join("jobs"), dir.clone()] {
        if let Err(why) = safe_dir(&p) {
            say!("sheepdog: refusing to sweep: {why}. Nothing was signalled.");
            return 1;
        }
    }
    let Some(_token) = crate::wall::gate() else { return 0 };
    let protected = match protected() {
        Ok(v) => v,
        Err(link) => {
            say!("sheepdog: refusing to sweep: sheepdog cannot follow its own chain of parent processes at pid {link}, so it cannot rule out a job that holds one of them. Nothing was signalled.");
            return 1;
        }
    };
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "journal")).collect();
    files.sort();
    let (mut swept, mut killed, mut code) = (0usize, 0usize, 0);
    for f in files {
        let j = match open(&f) {
            Ok(j) => j,
            Err(Skip::Live) => continue,
            Err(Skip::Unsafe(why)) | Err(Skip::Unreadable(why)) => {
                crate::note(format!("sweep skipped {}: {why}", f.display()));
                continue;
            }
        };
        if Some(j.boot.as_str()) != crate::journal::boot_id().as_deref() || j.pidns != crate::journal::pidns() {
            crate::note(format!("sweep skipped {}: another boot or pid namespace", f.display()));
            continue;
        }
        if j.owner != owner || j.leave_strays {
            continue;
        }
        let job = j.job.clone();
        match sweep_job(j, &protected, true) {
            Outcome::Swept(n) => {
                swept += 1;
                killed += n;
            }
            Outcome::Skipped(why) => {
                say!("sheepdog: skipped job {job}: {why}.");
                crate::note(format!("sweep skipped {job}: {why}"));
            }
            Outcome::Deadline(c) => code = code.max(c),
        }
    }
    if swept > 0 {
        say!("sheepdog: swept {swept} dead job{} ({killed} process{} ended).", if swept == 1 { "" } else { "s" }, if killed == 1 { "" } else { "es" });
    }
    code
}
