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
//! Suspects (PLAN.md §3.0, PHASE2.md D7) are listed with the proved set and killed only with
//! `--include-suspects`. `sheepdog ps` lists the same rows and signals nothing (`kill --dry-run`).
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
#[derive(Clone)]
pub struct Proc {
    pub pid: i32,
    pub ppid: i32,
    pub uid: u32,
    pub id: u64,
    /// macOS: the original parent's uniqueid; Linux: None (no such fact)
    pub puniq: Option<u64>,
    pub sid: i32,
    pub pgid: i32,
    /// macOS: the responsible process's uniqueid; Linux: None
    pub resp: Option<u64>,
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
    include_suspects: bool,
    json: bool,
}

pub(crate) const USAGE_KILL: &str = "sheepdog kill [--dry-run] [--json] [--include-suspects] [--grace DURATION] PID | PID:ID | j-JOBID";
pub(crate) const USAGE_PS: &str = "sheepdog ps [--json] PID | PID:ID | j-JOBID";

fn usage() -> i32 {
    crate::fail!("usage: {USAGE_KILL}");
    say!("       {USAGE_PS}");
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
    let mut include_suspects = false;
    let mut json = false;
    let mut grace = Duration::from_secs(2);
    let mut i = 0;
    while i < args.len() {
        match args[i].as_bytes() {
            b"--dry-run" => dry_run = true,
            b"--include-suspects" => include_suspects = true,
            b"--json" => {
                json = true;
                crate::json_on();
            }
            b"--grace" => {
                i += 1;
                grace = parse_duration(&args.get(i)?.to_string_lossy())?;
            }
            a if tgt.is_none() && !a.starts_with(b"-") => tgt = Some(target(a)?),
            _ => return None,
        }
        i += 1;
    }
    Some(Args { target: tgt?, dry_run, grace, include_suspects, json })
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
pub(crate) fn job_of(t: i32) -> Option<i32> {
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
        // only what this scan saw stays known: this user's live, unprotected processes (a seed
        // that names another user's process is never carried into a signal)
        self.known.retain(|p, id| members.get(p) == Some(id));
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

/// `sheepdog ps ARGS`: the rows of `kill --dry-run` (it lists; it never signals).
pub fn ps(args: &[OsString]) -> i32 {
    let mut a: Vec<OsString> = vec!["--dry-run".into()];
    for x in args {
        if matches!(x.as_bytes(), b"--dry-run" | b"--include-suspects" | b"--grace") {
            return usage();
        }
        a.push(x.clone());
    }
    main(&a)
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
        crate::fail!("sheepdog: {why}, so sheepdog cannot tell which process is which. Mount a /proc for this pid namespace (for example unshare --mount-proc). Nothing was signalled.");
        return 1;
    }
    let by_job = matches!(a.target, Target::Job(_));
    let mut job_path: Option<std::path::PathBuf> = None;
    // the identity the target must still have (PID:ID, or a job's supervisor from its journal):
    // checked again below, so a pid reused in between is refused
    let (t, expected) = match &a.target {
        Target::Pid(p) => (*p, None),
        Target::PidId(p, id) => {
            // a PID:ID target is a fact from outside: a phase-2 source (the wall's token)
            let Some(_token) = crate::wall::gate() else { return 1 };
            if identity(*p) != Some(*id) {
                crate::fail!("sheepdog: refusing to kill pid {p}: it is not the process {p}:{id} any more. Nothing was signalled.");
                return 1;
            }
            (*p, Some(*id))
        }
        Target::Job(prefix) => match job_target(prefix, a.dry_run) {
            Ok(JobTarget::Live(sup, id, path)) => {
                job_path = Some(path);
                (sup, Some(id))
            }
            Ok(JobTarget::Done(code)) => return code,
            Ok(JobTarget::List(rows)) => return print_rows(&rows, a.json),
            Err(code) => return code,
        },
    };
    if t == 1 {
        crate::fail!("sheepdog: refusing to kill pid 1: it is the system's init process. Nothing was signalled.");
        return 1;
    }
    let Some(tid) = identity(t) else {
        say!("sheepdog: pid {t} is already gone; nothing to kill.");
        return 0;
    };
    if expected.is_some_and(|e| e != tid) {
        crate::fail!("sheepdog: refusing to kill pid {t}: it is another process now. Nothing was signalled.");
        return 1;
    }
    if os::procs().iter().find(|p| p.pid == t && p.id == tid).map(|p| p.uid) != Some(unsafe { libc::getuid() }) {
        crate::fail!("sheepdog: refusing to kill pid {t}: it belongs to another user. Nothing was signalled.");
        return 1;
    }
    let protected = match protected() {
        Ok(v) => v,
        Err(link) => {
            crate::fail!("sheepdog: refusing to kill pid {t}: sheepdog cannot follow its own chain of parent processes at pid {link} (on Linux, /proc mounted with hidepid hides them), so it cannot rule out that pid {t} is one of them. Nothing was signalled.");
            return 1;
        }
    };
    if protected.iter().any(|&(p, id)| p == t && id == tid) {
        let what = if t == unsafe { libc::getpid() } { "it is this sheepdog" } else { "it is an ancestor of this sheepdog (the shell or program that started it)" };
        crate::fail!("sheepdog: refusing to kill pid {t}: {what}. Nothing was signalled.");
        return 1;
    }
    let mut proved = Proved { known: HashMap::from([(t, tid)]), ever: HashSet::new(), protected };
    // the journal as a membership fact (PHASE2.md §1 decision 12): a target that a dead job's
    // journal names brings its own subtree by the journal's lineage (never the rest of the job)
    let _held = match journal_subtree(t, tid, &mut proved) {
        Ok(j) => j,
        Err(p) => {
            crate::fail!("sheepdog: refusing to kill pid {t}: its journaled subtree holds pid {p}, this sheepdog or one of its ancestors (the shell that runs it). Nothing was signalled.");
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
    // the suspects (PHASE2.md D7): a phase-2 source, none under the phase-1 opt-out
    let procs = os::procs();
    let sus = if crate::seam_flag("SHEEPDOG_TEST_PHASE1") { Vec::new() } else { suspects((t, tid), &set, &procs, &proved.protected) };
    // what a kill with the suspects would also take: their proved trees (ps lists it)
    let under: Vec<(i32, u64)> = if sus.is_empty() {
        Vec::new()
    } else {
        let known: HashMap<i32, u64> = sus.iter().map(|((p, id), _)| (*p, *id)).collect();
        let ever: HashSet<u64> = known.values().copied().collect();
        let mut s2 = Proved { known, ever, protected: proved.protected.clone() };
        s2.scan().into_iter().filter(|m| !set.contains(m) && !sus.iter().any(|(x, _)| x == m)).collect()
    };
    if a.dry_run {
        let ever: HashSet<u64> = set.iter().map(|&(_, id)| id).collect();
        let mut rows: Vec<Row> = set
            .iter()
            .map(|&(p, id)| {
                let mut ev = Vec::new();
                if p == t {
                    return Row { pid: p, id, class: "target", evidence: vec!["named".into()] };
                }
                if let Some(pr) = procs.iter().find(|x| x.pid == p && x.id == id) {
                    if set.iter().any(|&(q, _)| q == pr.ppid) {
                        ev.push("ppid".to_string());
                    }
                    if pr.puniq.is_some_and(|u| ever.contains(&u)) {
                        ev.push("puniq".to_string());
                    }
                }
                if ev.is_empty() {
                    ev.push("journal".to_string());
                }
                if sups.iter().any(|&(q, _)| q == p) {
                    ev.push("sheepdog: ended first".to_string());
                }
                Row { pid: p, id, class: "proved", evidence: ev }
            })
            .collect();
        // a live job: also its journal's live members that `kill` cannot prove (on macOS an
        // escapee is its job's member by responsibility only)
        if let Some(path) = &job_path {
            for (p, id) in crate::sweep::peek(path) {
                if same(p, id) && !rows.iter().any(|r| r.pid == p && r.id == id) {
                    rows.push(Row { pid: p, id, class: "proved", evidence: vec!["journal".into()] });
                }
            }
        }
        rows.extend(sus.iter().map(|((p, id), ev)| Row { pid: *p, id: *id, class: "suspect", evidence: ev.clone() }));
        rows.extend(under.iter().map(|&(p, id)| {
            let mut ev = vec!["under a suspect".to_string()];
            if is_sheepdog(p) {
                ev.push("sheepdog: ended first".into());
            }
            Row { pid: p, id, class: "suspect", evidence: ev }
        }));
        return print_rows(&rows, a.json);
    }
    let mut left = sus.len();
    let mut sups = sups;
    if a.include_suspects && !sus.is_empty() {
        // the first suspect enters the kill set only after the gate (PHASE2.md §0.1)
        if crate::wall::gate().is_some() {
            for ((p, id), _) in &sus {
                proved.known.insert(*p, *id);
                proved.ever.insert(*id);
            }
            left = 0;
            // a supervisor under a suspect is ended first too (its own kill reaches its job)
            for m in proved.scan() {
                if is_sheepdog(m.0) && !sups.contains(&m) {
                    sups.push(m);
                }
            }
        }
    }
    let mut opts = KillOpts::from_env().with_grace(a.grace);
    let unended = if sups.is_empty() { Vec::new() } else { end_supervisors(&sups, opts.deadline, &mut proved) };
    // `kill j-`: a job's supervisor that does not end on TERM is left alone (no escalation): its
    // own kill at the job's end still guards what this kill cannot prove
    if by_job && unended.contains(&t) {
        crate::fail!("sheepdog: the job's supervisor pid {t} did not end on TERM (its caller may ignore TERM); it was left running. The job is NOT ended.");
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
            crate::fail!("sheepdog: supervisor pid(s) {unended:?} did not end on TERM (its caller may ignore TERM), so the escapees of its job may still be alive. The tree is NOT clean.");
            125
        }
        Ok(()) => 0,
        Err(e) => kill_failed(e),
    };
    unsafe { libc::sigprocmask(libc::SIG_SETMASK, &mask, std::ptr::null_mut()) };
    if left > 0 {
        let pids: Vec<String> = sus.iter().filter(|((p, id), _)| same(*p, *id)).map(|((p, _), _)| p.to_string()).collect();
        if !pids.is_empty() {
            say!("sheepdog: {} suspect(s) left alive: pid {} (sheepdog ps {t} shows why; --include-suspects kills them too).", pids.len(), pids.join(", "));
        }
    }
    code
}

/// One row of `ps` / `kill --dry-run`.
pub(crate) struct Row {
    pid: i32,
    id: u64,
    class: &'static str,
    evidence: Vec<String>,
}

/// Print the rows (pid order within each class: target, proved, suspect); exit code 0. Tab-
/// separated: pid, program, class, evidence, age, memory, command; or one JSON object per line.
fn print_rows(rows: &[Row], json: bool) -> i32 {
    let rank = |c: &str| match c {
        "target" => 0,
        "proved" => 1,
        _ => 2,
    };
    let mut v: Vec<&Row> = rows.iter().collect();
    v.sort_by_key(|r| (rank(r.class), r.pid));
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    for r in v {
        if !same(r.pid, r.id) {
            continue; // gone since the scan
        }
        let age = os::start_secs(r.pid).map(|s| now.saturating_sub(s));
        let mem = crate::caps::mem_of(r.pid);
        let cmd = shown_cmd(&os::cmdline(r.pid).join(" "));
        let name = clean(&os::exe_name(r.pid).unwrap_or_else(|| "?".into()));
        let line = if json {
            let ev: Vec<String> = r.evidence.iter().map(|e| crate::journal::json_str(e)).collect();
            format!(
                "{{\"v\":1,\"pid\":{},\"id\":{},\"class\":\"{}\",\"evidence\":[{}],\"age_s\":{},\"mem\":{},\"cmd\":{}}}",
                r.pid,
                r.id,
                r.class,
                ev.join(","),
                age.map_or("null".into(), |a| a.to_string()),
                mem,
                crate::journal::json_str(&cmd)
            )
        } else {
            let age = age.map_or("?".into(), |a| format!("{a}s"));
            format!("{}\t{name}\t{}\t{}\t{age}\t{}\t{}", r.pid, r.class, clean(&r.evidence.join(",")), human(mem), clean(&cmd))
        };
        // a reader that has gone gives EPIPE (SIGPIPE is blocked): stop quietly, never panic
        if writeln!(std::io::stdout(), "{line}").is_err() {
            break;
        }
    }
    0
}

pub(crate) fn human(b: u64) -> String {
    match b {
        b if b >= 1 << 30 => format!("{:.1}G", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("{}M", b >> 20),
        b => format!("{}K", b >> 10),
    }
}

/// The suspects of target `t` (PLAN.md §3.0, PHASE2.md D7): live, same-uid processes that are
/// not in `set` (proved), not `protected`, not PID 1 and no `sheepdog`, that are orphaned (their
/// parent is PID 1 or a `sheepdog`), started after the target (a greater identity), and have a
/// link: the session or group of the target or a proved member, or (macOS) a `puniq` that is no
/// live process, greater than the target's uniqueid, with the target's responsible process.
/// Each with the links that matched.
fn suspects(t: (i32, u64), set: &[(i32, u64)], procs: &[Proc], protected: &[(i32, u64)]) -> Vec<((i32, u64), Vec<String>)> {
    let uid = unsafe { libc::getuid() };
    suspects_with(t, set, procs, protected, uid, |p| is_sheepdog(p) || job_of(p).is_some())
}

/// The rules of `suspects`, with the uid and "is a sheepdog or a live job's member" given (for
/// its unit cells). PHASE2.md D9 narrowed PLAN §3.0: orphaned means parent PID 1 (a child of a
/// sheepdog is its job's); a session or group led by PID 1 is no link (in a container it holds
/// almost everything); `puniq` is evidence beside a session or group link, never a link alone
/// (every process under one terminal app shares the responsible process).
fn suspects_with(t: (i32, u64), set: &[(i32, u64)], procs: &[Proc], protected: &[(i32, u64)], uid: u32, in_a_job: impl Fn(i32) -> bool) -> Vec<((i32, u64), Vec<String>)> {
    let Some(tp) = procs.iter().find(|p| p.pid == t.0 && p.id == t.1) else { return Vec::new() };
    let in_set = |p: &Proc| set.iter().any(|&(q, id)| q == p.pid && id == p.id) || (p.pid == t.0 && p.id == t.1);
    let known: Vec<&Proc> = procs.iter().filter(|p| in_set(p)).collect();
    let sessions: HashSet<i32> = known.iter().map(|p| p.sid).filter(|&s| s > 1).collect();
    let groups: HashSet<i32> = known.iter().map(|p| p.pgid).filter(|&g| g > 1).collect();
    let live: HashSet<u64> = procs.iter().map(|p| p.id).collect();
    let mut out = Vec::new();
    for p in procs {
        if p.uid != uid || p.pid <= 1 || in_set(p) || protected.iter().any(|&(q, id)| q == p.pid && id == p.id) {
            continue;
        }
        if p.ppid != 1 || p.id <= tp.id || in_a_job(p.pid) {
            continue;
        }
        let mut ev = Vec::new();
        if sessions.contains(&p.sid) {
            ev.push("session".to_string());
        }
        if groups.contains(&p.pgid) {
            ev.push("group".to_string());
        }
        if ev.is_empty() {
            continue;
        }
        if p.puniq.is_some_and(|u| u > tp.id && !live.contains(&u)) && p.resp.is_some() && p.resp == tp.resp {
            ev.push("puniq".to_string());
        }
        out.push(((p.pid, p.id), ev));
    }
    out
}

/// A command line as shown: the journal's text (capped at CMD_CAP bytes, backslashes doubled),
/// ending with `…` when it was cut.
pub(crate) fn shown_cmd(full: &str) -> String {
    let mut cmd = crate::journal::text(full.as_bytes());
    if full.len() > crate::journal::CMD_CAP {
        cmd.push('…');
    }
    cmd
}

/// Text for a terminal: every control character (C0, DEL, C1) as `\xHH`, so a process's
/// command line cannot forge rows or reach the terminal.
pub(crate) fn clean(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for ch in s.chars() {
        if (ch as u32) < 0x20 || ch == '\u{7f}' || ('\u{80}'..='\u{9f}').contains(&ch) {
            o.push_str(&format!("\\x{:02x}", ch as u32));
        } else {
            o.push(ch);
        }
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr(pid: i32, ppid: i32, id: u64, sid: i32, pgid: i32, puniq: Option<u64>, resp: Option<u64>) -> Proc {
        Proc { pid, ppid, uid: 501, id, puniq, sid, pgid, resp }
    }

    #[test]
    fn the_suspect_rules() {
        let t = pr(100, 50, 1000, 90, 90, Some(900), Some(5));
        let procs = vec![
            t.clone(),
            pr(101, 1, 1001, 90, 90, Some(9999), Some(5)), // a suspect: session and group
            pr(102, 60, 1002, 90, 90, None, Some(5)),      // parent alive: no orphan
            pr(103, 1, 999, 90, 90, None, Some(5)),        // started before the target
            pr(104, 1, 1004, 1, 1, None, Some(5)),         // only PID 1's session and group
            pr(105, 1, 1005, 77, 77, Some(9998), Some(5)), // only puniq: dead, after, same responsible
            pr(106, 1, 1006, 90, 90, None, Some(5)),       // a live job's member
            pr(107, 1, 1007, 90, 91, None, Some(5)),       // session only
        ];
        let got = suspects_with((100, 1000), &[], &procs, &[], 501, |p| p == 106);
        let pids: Vec<i32> = got.iter().map(|((p, _), _)| *p).collect();
        assert_eq!(pids, vec![101, 107]);
        assert_eq!(got[0].1, vec!["session", "group", "puniq"]);
        assert_eq!(got[1].1, vec!["session"]);
        // a target in PID 1's session links nothing
        let t1 = pr(200, 50, 2000, 1, 1, None, None);
        let procs1 = vec![t1, pr(201, 1, 2001, 1, 1, None, None)];
        assert!(suspects_with((200, 2000), &[], &procs1, &[], 501, |_| false).is_empty());
    }

    #[test]
    fn a_cut_command_line_says_so() {
        let cap = crate::journal::CMD_CAP;
        assert!(!shown_cmd(&"x".repeat(cap)).ends_with('…'));
        assert!(shown_cmd(&"x".repeat(cap + 1)).ends_with('…'));
    }

    #[test]
    fn clean_escapes_every_control_character() {
        assert_eq!(clean("a\nb\tc\u{1b}[2K\u{7f}\u{9b}é"), "a\\x0ab\\x09c\\x1b[2K\\x7f\\x9bé");
    }
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
    /// `--dry-run` / `ps` of a dead job: its journal's closure, as rows
    List(Vec<Row>),
    /// the job's supervisor lives (pid, identity): kill it (the normal path, with its checks)
    Live(i32, u64, std::path::PathBuf),
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
            crate::fail!("sheepdog: no job {prefix} in this boot's journals. Nothing was signalled.");
            return Err(1);
        }
        [one] => one.clone(),
        many => {
            let names: Vec<String> = many.iter().filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned())).collect();
            crate::fail!("sheepdog: {prefix} matches several jobs ({}); give more of the id. Nothing was signalled.", names.join(", "));
            return Err(2);
        }
    };
    let Some(sup) = journal_sup(&path) else {
        crate::fail!("sheepdog: job {prefix}'s journal is not this user's, not of this boot, or unreadable. Nothing was signalled.");
        return Err(1);
    };
    if sup.0 > 1 && same(sup.0, sup.1) {
        if !is_sheepdog(sup.0) {
            crate::fail!("sheepdog: job {prefix}'s journal names pid {} as its supervisor, but that process is not a sheepdog. Nothing was signalled.", sup.0);
            return Err(1);
        }
        return Ok(JobTarget::Live(sup.0, sup.1, path));
    }
    if dry_run {
        // what a sweep of this dead job would reach: its journaled members still alive and
        // their closure (the sweep's own scan)
        let mut rows = Vec::new();
        if let Ok(j) = crate::sweep::open_fenced(&path) {
            let known: HashMap<i32, u64> = j.members.iter().filter(|m| same(m.pid, m.id)).map(|m| (m.pid, m.id)).collect();
            let ever: HashSet<u64> = j.members.iter().map(|m| m.id).collect();
            let named: HashSet<(i32, u64)> = known.iter().map(|(&p, &id)| (p, id)).collect();
            let mut proved = Proved { known, ever, protected: protected().unwrap_or_default() };
            for (p, id) in proved.scan() {
                let ev = if named.contains(&(p, id)) { "journal" } else { "closure" };
                rows.push(Row { pid: p, id, class: "proved", evidence: vec![ev.to_string()] });
            }
        }
        return Ok(JobTarget::List(rows));
    }
    // a dead job: the sweep of this one journal
    let Ok(protected) = protected() else {
        crate::fail!("sheepdog: refusing to sweep {prefix}: sheepdog cannot follow its own chain of parent processes. Nothing was signalled.");
        return Err(1);
    };
    match crate::sweep::open_fenced(&path) {
        Ok(j) => match crate::sweep::sweep_job_as(j, &protected, crate::sweep::Mode::Explicit) {
            crate::sweep::Outcome::Swept(_) => Ok(JobTarget::Done(0)),
            crate::sweep::Outcome::Skipped(why) => {
                crate::fail!("sheepdog: refusing to sweep {prefix}: {why}. Nothing was signalled.");
                Err(1)
            }
            crate::sweep::Outcome::Deadline(c) => Ok(JobTarget::Done(c)),
        },
        Err(crate::sweep::Skip::Live) => {
            crate::fail!("sheepdog: job {prefix} is busy (another sweep, or its supervisor is just ending). Nothing was signalled.");
            Err(1)
        }
        Err(crate::sweep::Skip::Unsafe(why)) | Err(crate::sweep::Skip::Unreadable(why)) => {
            crate::fail!("sheepdog: refusing to sweep job {prefix}: {why}. Nothing was signalled.");
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
