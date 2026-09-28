//! `sheepdog kill <pid>` (PLAN.md §3.0; PHASE1.md S6): kill the proved part of a tree that
//! sheepdog did not start. No setup, so no guarantee: the proved members are the target's live
//! descendants by the ppid chain, iterated to a fixed point, and on macOS also every live process
//! whose original parent's uniqueid (`puniq`) belongs to a member seen alive by an earlier scan.
//! The kill is the loop of §3.3 (TERM grace, then freeze, verify, kill, repeat, with a deadline).
//!
//! A `sheepdog` process in the proved set (an inner supervisor, or its relay) is ended first:
//! TERM, then CONT (a stopped supervisor would never act on the TERM), then a wait of up to its
//! grace plus the kill deadline, so that its own kill reaches escapees that this kill cannot
//! prove. A supervisor is recognised by its executable's file name, `sheepdog`.
//!
//! Exit codes: 0 every targeted process is gone (also: the target was already gone); 1 refused,
//! nothing signalled; 2 usage error; 125 the kill deadline passed with members alive.

#[cfg(target_os = "linux")]
use crate::linux as os;
#[cfg(target_os = "macos")]
use crate::macos as os;
use crate::{kill_failed, kill_tree, parse_duration, say, signal, trace, KillOpts};
use sheepdog::ident::{identity, same};
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::time::{Duration, Instant};

/// A live (not zombie) process, as the platform lists it.
pub struct Proc {
    pub pid: i32,
    pub ppid: i32,
    pub uid: u32,
    pub id: u64,
    /// macOS: the original parent's uniqueid; Linux: None (no such fact)
    pub puniq: Option<u64>,
}

enum Target {
    Pid(i32),
    /// `PID:ID`: only if the pid is still that process
    PidId(i32, u64),
    /// `j-XXXX`: a job id or a prefix of at least 4 hex digits (PHASE2.md §1 decision 4)
    Job(String),
}

struct Args {
    target: Target,
    dry_run: bool,
    grace: Duration,
}

fn usage() -> i32 {
    say!("usage: sheepdog kill [--dry-run] [--grace DURATION] PID | PID:ID | j-JOBID");
    2
}

fn target(a: &[u8]) -> Option<Target> {
    let s = std::str::from_utf8(a).ok()?;
    if let Some(hex) = s.strip_prefix("j-") {
        return (hex.len() >= 4 && hex.len() <= 8 && hex.bytes().all(|c| c.is_ascii_hexdigit())).then(|| Target::Job(s.to_ascii_lowercase()));
    }
    if let Some((p, id)) = s.split_once(':') {
        let p: i32 = p.parse().ok().filter(|&p| p > 0)?;
        return Some(Target::PidId(p, id.parse().ok()?));
    }
    (!s.is_empty() && s.bytes().all(|c| c.is_ascii_digit())).then(|| s.parse().ok().filter(|&p| p > 0).map(Target::Pid)).flatten()
}

fn parse(args: &[OsString]) -> Option<Args> {
    let mut tgt = None;
    let mut dry_run = false;
    let mut grace = Duration::from_secs(2);
    let mut i = 0;
    while i < args.len() {
        match args[i].as_bytes() {
            b"--dry-run" => dry_run = true,
            b"--grace" => {
                i += 1;
                grace = parse_duration(&args.get(i)?.to_string_lossy())?;
            }
            a if tgt.is_none() && !a.starts_with(b"-") => tgt = Some(target(a)?),
            _ => return None,
        }
        i += 1;
    }
    Some(Args { target: tgt?, dry_run, grace })
}

/// Is `pid` a sheepdog (its executable's file name)?
/// A sheepdog supervisor (a Linux root shim before its exec runs the same binary, but is the
/// job's root, not a supervisor).
pub(crate) fn is_sheepdog(pid: i32) -> bool {
    os::exe_name(pid).as_deref() == Some("sheepdog") && os::cmdline(pid).get(1).map(String::as_str) != Some("__root")
}

/// The parent of `pid`. Debug seam SHEEPDOG_TEST_PARENT_UNREADABLE=<pid>: that pid's parent
/// cannot be read (as under a `/proc` mounted with `hidepid`).
pub(crate) fn parent(pid: i32) -> Option<i32> {
    if crate::seam_ms("SHEEPDOG_TEST_PARENT_UNREADABLE") == Some(pid as u64) {
        return None;
    }
    os::parent(pid)
}

/// This process and all its ancestors up to pid 1, with their identities: never part of a
/// kill. Err(pid) if the chain cannot be read to its end (`pid` is the link that could not be
/// read): then any process may be an ancestor, and every target is refused (fail closed).
pub(crate) fn protected() -> Result<Vec<(i32, u64)>, i32> {
    let mut v = Vec::new();
    let mut p = unsafe { libc::getpid() };
    while p > 1 {
        if v.len() >= 4096 {
            return Err(p);
        }
        v.push((p, identity(p).ok_or(p)?));
        p = parent(p).ok_or(p)?;
    }
    Ok(v)
}

/// The supervisor of the running job that `t` belongs to, if any: the nearest `sheepdog`
/// ancestor, or (macOS) a `sheepdog` responsible for it.
fn job_of(t: i32) -> Option<i32> {
    let mut p = parent(t);
    let mut n = 0;
    while let Some(q) = p.filter(|&q| q > 1 && n < 4096) {
        if is_sheepdog(q) {
            return Some(q);
        }
        p = parent(q);
        n += 1;
    }
    os::responsible_pid(t).filter(|&r| r > 1 && r != t && is_sheepdog(r))
}

/// The proved set, sticky across scans: a member stays known until it is gone, and (macOS) the
/// uniqueid of every member ever seen stays a `puniq` link after the member has exited.
pub(crate) struct Proved {
    pub(crate) known: HashMap<i32, u64>,
    pub(crate) ever: HashSet<u64>,
    pub(crate) protected: Vec<(i32, u64)>,
}

impl Proved {
    pub(crate) fn scan(&mut self) -> Vec<(i32, u64)> {
        let uid = unsafe { libc::getuid() };
        let procs: Vec<Proc> = os::procs()
            .into_iter()
            .filter(|p| p.pid > 1 && p.uid == uid && !self.protected.iter().any(|&(q, id)| q == p.pid && id == p.id))
            .collect();
        let mut members: HashMap<i32, u64> =
            procs.iter().filter(|p| self.known.get(&p.pid) == Some(&p.id)).map(|p| (p.pid, p.id)).collect();
        loop {
            let mut changed = false;
            for p in &procs {
                if !members.contains_key(&p.pid)
                    && (members.contains_key(&p.ppid) || p.puniq.is_some_and(|u| self.ever.contains(&u)))
                {
                    members.insert(p.pid, p.id);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        for (&p, &id) in &members {
            self.known.insert(p, id);
            self.ever.insert(id);
        }
        self.known.retain(|&p, &mut id| same(p, id));
        members.into_iter().collect()
    }
}

/// TERM, then CONT, to every supervisor in the set, then wait until they are gone, at most
/// their grace (read from their argv; 2 s if unreadable) plus the kill deadline. The rest of the
/// tree runs meanwhile, so it is scanned all the while (a child seen while its parent lives stays
/// proved after the parent exits). Returns the supervisors still alive at the end.
pub(crate) fn end_supervisors(sups: &[(i32, u64)], deadline: Duration, proved: &mut Proved) -> Vec<i32> {
    let mut bound = Duration::ZERO;
    for &(p, id) in sups {
        trace(format!("supervisor {p}"));
        let _ = signal(p, id, libc::SIGTERM);
        let _ = signal(p, id, libc::SIGCONT);
        bound = bound.max(grace_of(p) + deadline);
    }
    let end = Instant::now() + bound;
    while sups.iter().any(|&(p, id)| same(p, id)) && Instant::now() < end {
        proved.scan();
        std::thread::sleep(Duration::from_millis(10));
    }
    sups.iter().filter(|&&(p, id)| same(p, id)).map(|&(p, _)| p).collect()
}

/// The `--grace` of a `sheepdog run` from its argv (default 2 s).
fn grace_of(pid: i32) -> Duration {
    let argv = os::cmdline(pid);
    let opts = argv.iter().skip(1).take_while(|a| a.as_str() != "--");
    let mut it = opts.skip_while(|a| a.as_str() != "--grace");
    it.next();
    it.next().and_then(|g| parse_duration(g)).unwrap_or(Duration::from_secs(2))
}

pub fn main(args: &[OsString]) -> i32 {
    // signals that would end sheepdog at their default action and that it never uses: blocked
    // for the whole run (a closed stderr is EPIPE, never SIGPIPE; phase-1 review)
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        crate::block_all_but_faults(&mut set);
        // INT, TERM, HUP and QUIT stay deliverable until the first freeze (a ctrl-C during the
        // grace ends `kill` at once); the hold covers them from there
        for s in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGQUIT] {
            libc::sigdelset(&mut set, s);
        }
        libc::sigprocmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
    }
    let Some(a) = parse(args) else { return usage() };
    #[cfg(target_os = "linux")]
    if let Some(why) = os::proc_problem() {
        say!("sheepdog: {why}, so sheepdog cannot tell which process is which. Mount a /proc for this pid namespace (for example unshare --mount-proc). Nothing was signalled.");
        return 1;
    }
    let by_job = matches!(a.target, Target::Job(_));
    // the identity the target must still have (PID:ID, or a job's supervisor from its journal):
    // checked again below, so a pid reused in between is refused
    let (t, expected) = match &a.target {
        Target::Pid(p) => (*p, None),
        Target::PidId(p, id) => {
            // a PID:ID target is a fact from outside: a phase-2 source (the wall's token)
            let Some(_token) = crate::wall::gate() else { return 1 };
            if identity(*p) != Some(*id) {
                say!("sheepdog: refusing to kill pid {p}: it is not the process {p}:{id} any more. Nothing was signalled.");
                return 1;
            }
            (*p, Some(*id))
        }
        Target::Job(prefix) => match job_target(prefix, a.dry_run) {
            Ok(JobTarget::Live(sup, id)) => (sup, Some(id)),
            Ok(JobTarget::Done(code)) => return code,
            Err(code) => return code,
        },
    };
    if t == 1 {
        say!("sheepdog: refusing to kill pid 1: it is the system's init process. Nothing was signalled.");
        return 1;
    }
    let Some(tid) = identity(t) else {
        say!("sheepdog: pid {t} is already gone; nothing to kill.");
        return 0;
    };
    if expected.is_some_and(|e| e != tid) {
        say!("sheepdog: refusing to kill pid {t}: it is another process now. Nothing was signalled.");
        return 1;
    }
    if os::procs().iter().find(|p| p.pid == t && p.id == tid).map(|p| p.uid) != Some(unsafe { libc::getuid() }) {
        say!("sheepdog: refusing to kill pid {t}: it belongs to another user. Nothing was signalled.");
        return 1;
    }
    let protected = match protected() {
        Ok(v) => v,
        Err(link) => {
            say!("sheepdog: refusing to kill pid {t}: sheepdog cannot follow its own chain of parent processes at pid {link} (on Linux, /proc mounted with hidepid hides them), so it cannot rule out that pid {t} is one of them. Nothing was signalled.");
            return 1;
        }
    };
    if protected.iter().any(|&(p, id)| p == t && id == tid) {
        let what = if t == unsafe { libc::getpid() } { "it is this sheepdog" } else { "it is an ancestor of this sheepdog (the shell or program that started it)" };
        say!("sheepdog: refusing to kill pid {t}: {what}. Nothing was signalled.");
        return 1;
    }
    let mut proved = Proved { known: HashMap::from([(t, tid)]), ever: HashSet::new(), protected };
    // the journal as a membership fact (PHASE2.md §1 decision 12): a target that a dead job's
    // journal names brings its own subtree by the journal's lineage (never the rest of the job)
    let _held = match journal_subtree(t, tid, &mut proved) {
        Ok(j) => j,
        Err(p) => {
            say!("sheepdog: refusing to kill pid {t}: its journaled subtree holds pid {p}, this sheepdog or one of its ancestors (the shell that runs it). Nothing was signalled.");
            return 1;
        }
    };
    let set = proved.scan();
    let sups: Vec<(i32, u64)> = set.iter().copied().filter(|&(p, _)| is_sheepdog(p)).collect();
    if !is_sheepdog(t) {
        if let Some(s) = job_of(t) {
            // one line that claims only this: `s` is the nearest sheepdog above `t` (by parent,
            // or on macOS by responsibility), and `sheepdog kill s` is the way to end its job:
            // its TERM ends the whole job, strays too, even with --leave-strays. An `s` whose
            // caller ignores TERM does not end on it; what that kill can still prove is said at
            // `ADOPTS_ESCAPEES`
            match live_job_of(s) {
                Some(job) => say!("sheepdog: pid {t} runs under sheepdog pid {s} (job {job}). To end that job: sheepdog kill {job}"),
                None => say!("sheepdog: pid {t} runs under sheepdog pid {s}. To end that sheepdog's job: sheepdog kill {s}"),
            }
        }
    }
    if a.dry_run {
        let mut v = set.clone();
        v.sort();
        for (p, _) in v {
            let name = os::exe_name(p).unwrap_or_else(|| "?".into());
            let note = if sups.iter().any(|&(q, _)| q == p) { "\t(a sheepdog: ended first)" } else { "" };
            // a reader that has gone gives EPIPE (SIGPIPE is blocked): stop quietly, never panic
            if writeln!(std::io::stdout(), "{p}\t{name}{note}").is_err() {
                break;
            }
        }
        return 0;
    }
    let mut opts = KillOpts::from_env().with_grace(a.grace);
    let unended = if sups.is_empty() { Vec::new() } else { end_supervisors(&sups, opts.deadline, &mut proved) };
    // `kill j-`: a job's supervisor that does not end on TERM is left alone (no escalation): its
    // own kill at the job's end still guards what this kill cannot prove
    if by_job && unended.contains(&t) {
        say!("sheepdog: the job's supervisor pid {t} did not end on TERM (its caller may ignore TERM); it was left running. The job is NOT ended.");
        return 125;
    }
    let initial: HashMap<i32, u64> = proved.known.clone();
    // a signal that ends sheepdog between a freeze and its SIGKILL would leave the tree stopped
    // for good: the kill loop holds them from its first freeze on, and sheepdog then acts on any
    // that arrived (the caller sees it die of the signal, as without the hold)
    opts.hold = vec![libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGQUIT];
    let mut mask: libc::sigset_t = unsafe { std::mem::zeroed() };
    unsafe { libc::sigprocmask(libc::SIG_SETMASK, std::ptr::null(), &mut mask) };
    let code = match kill_tree(&opts, || proved.scan(), || {}, || None, signal, initial) {
        Ok(()) if !unended.is_empty() && !os::ADOPTS_ESCAPEES => {
            // its escapees are members of its job only, which `kill` cannot prove on macOS
            say!("sheepdog: supervisor pid(s) {unended:?} did not end on TERM (its caller may ignore TERM), so the escapees of its job may still be alive. The tree is NOT clean.");
            125
        }
        Ok(()) => 0,
        Err(e) => kill_failed(e),
    };
    unsafe { libc::sigprocmask(libc::SIG_SETMASK, &mask, std::ptr::null_mut()) };
    code
}

/// The journals of this boot and pid namespace (none without a state).
fn journal_files() -> Vec<std::path::PathBuf> {
    let Some(state) = crate::state::resolve(cfg!(debug_assertions), |k| std::env::var_os(k), |p| p.exists()) else { return Vec::new() };
    let Some(dir) = crate::sweep::folder(&state) else { return Vec::new() };
    if [state.clone(), state.join("jobs"), dir.clone()].iter().any(|p| crate::sweep::safe_dir(p).is_err()) {
        return Vec::new();
    }
    let mut v: Vec<_> = std::fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "journal")).collect();
    v.sort();
    v
}

/// The header's supervisor (pid, identity) of a journal, read without its lock (fenced: never
/// through a symlink, only this user's file, this boot's and pid namespace's header).
fn journal_sup(path: &std::path::Path) -> Option<(i32, u64)> {
    crate::sweep::read_header(path).map(|(_, sup)| sup)
}

/// The job id of the live job whose supervisor is `sup` (by its journal's header).
fn live_job_of(sup: i32) -> Option<String> {
    let id = identity(sup)?;
    journal_files().into_iter().find(|p| journal_sup(p) == Some((sup, id))).and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
}

enum JobTarget {
    /// the job's supervisor lives (pid, identity): kill it (the normal path, with its checks)
    Live(i32, u64),
    /// the job was handled here (a dead job swept): this exit code
    Done(i32),
}

/// `kill j-XXXX` (PHASE2.md §1 decision 11): exactly one journal must match the prefix (none: 1,
/// several: 2, with the candidates listed). A live supervisor (pid and identity as in the header)
/// is the target; a dead one (gone, or its pid reused) means the job is swept.
fn job_target(prefix: &str, dry_run: bool) -> Result<JobTarget, i32> {
    // a job id resolves through a journal: a phase-2 source (the wall's token) before any fact
    let Some(_token) = crate::wall::gate() else { return Err(1) };
    let found: Vec<_> = journal_files().into_iter().filter(|p| p.file_stem().is_some_and(|s| s.to_string_lossy().starts_with(prefix))).collect();
    let path = match found.as_slice() {
        [] => {
            say!("sheepdog: no job {prefix} in this boot's journals. Nothing was signalled.");
            return Err(1);
        }
        [one] => one.clone(),
        many => {
            let names: Vec<String> = many.iter().filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned())).collect();
            say!("sheepdog: {prefix} matches several jobs ({}); give more of the id. Nothing was signalled.", names.join(", "));
            return Err(2);
        }
    };
    let Some(sup) = journal_sup(&path) else {
        say!("sheepdog: job {prefix}'s journal is not this user's, not of this boot, or unreadable. Nothing was signalled.");
        return Err(1);
    };
    if sup.0 > 1 && same(sup.0, sup.1) {
        return Ok(JobTarget::Live(sup.0, sup.1));
    }
    if dry_run {
        // what a sweep of this dead job would start from: its journaled members still alive
        if let Ok(j) = crate::sweep::open_fenced(&path) {
            for m in j.members.iter().filter(|m| same(m.pid, m.id)) {
                if writeln!(std::io::stdout(), "{}\t{}", m.pid, os::exe_name(m.pid).unwrap_or_else(|| "?".into())).is_err() {
                    break;
                }
            }
        }
        return Ok(JobTarget::Done(0));
    }
    // a dead job: the sweep of this one journal
    let Ok(protected) = protected() else {
        say!("sheepdog: refusing to sweep {prefix}: sheepdog cannot follow its own chain of parent processes. Nothing was signalled.");
        return Err(1);
    };
    match crate::sweep::open_fenced(&path) {
        Ok(j) => match crate::sweep::sweep_job_as(j, &protected, crate::sweep::Mode::Explicit) {
            crate::sweep::Outcome::Swept(_) => Ok(JobTarget::Done(0)),
            crate::sweep::Outcome::Skipped(why) => {
                say!("sheepdog: refusing to sweep {prefix}: {why}. Nothing was signalled.");
                Err(1)
            }
            crate::sweep::Outcome::Deadline(c) => Ok(JobTarget::Done(c)),
        },
        Err(crate::sweep::Skip::Live) => {
            say!("sheepdog: job {prefix} is busy (another sweep, or its supervisor is just ending). Nothing was signalled.");
            Err(1)
        }
        Err(crate::sweep::Skip::Unsafe(why)) | Err(crate::sweep::Skip::Unreadable(why)) => {
            say!("sheepdog: refusing to sweep job {prefix}: {why}. Nothing was signalled.");
            Err(1)
        }
    }
}

/// If a dead job's journal names the target (pid and identity), add the target's own subtree by
/// the journal's lineage: lines whose parent (pid and identity) is in the subtree, and on macOS
/// lines whose `puniq` is the identity of one in it. Returns the journal, open and locked, so no
/// sweep takes it meanwhile.
/// Err(pid): the subtree holds `pid`, this process or one of its ancestors: the kill is refused
/// whole, as the sweep skips such a job whole (never a signal to the caller).
fn journal_subtree(t: i32, tid: u64, proved: &mut Proved) -> Result<Option<crate::sweep::Journal>, i32> {
    let files = journal_files();
    for f in files {
        let Ok(j) = crate::sweep::open_fenced(&f) else { continue }; // a live job's is locked
        if !j.members.iter().any(|m| m.pid == t && m.id == tid) {
            continue;
        }
        let Some(_token) = crate::wall::gate() else { return Ok(None) }; // a phase-2 fact
        let mut sub: HashSet<(i32, u64)> = HashSet::from([(t, tid)]);
        loop {
            let ids: HashSet<u64> = sub.iter().map(|&(_, id)| id).collect();
            let more: Vec<(i32, u64)> = j
                .members
                .iter()
                .filter(|m| !sub.contains(&(m.pid, m.id)))
                .filter(|m| m.ppid.zip(m.pid_id).is_some_and(|pp| sub.contains(&pp)) || m.puniq.is_some_and(|u| ids.contains(&u)))
                .map(|m| (m.pid, m.id))
                .collect();
            if more.is_empty() {
                break;
            }
            sub.extend(more);
        }
        if let Some(&(p, _)) = proved.protected.iter().find(|pr| sub.contains(pr)) {
            return Err(p);
        }
        for (p, id) in sub {
            proved.ever.insert(id);
            if same(p, id) {
                proved.known.insert(p, id);
            }
        }
        return Ok(Some(j));
    }
    Ok(None)
}
