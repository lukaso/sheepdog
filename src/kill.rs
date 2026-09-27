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

struct Args {
    pid: i32,
    dry_run: bool,
    grace: Duration,
}

fn usage() -> i32 {
    say!("usage: sheepdog kill [--dry-run] [--grace DURATION] PID");
    2
}

fn parse(args: &[OsString]) -> Option<Args> {
    let mut pid = None;
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
            a if !a.is_empty() && a.iter().all(u8::is_ascii_digit) && pid.is_none() => {
                pid = Some(std::str::from_utf8(a).ok()?.parse::<i32>().ok().filter(|&p| p > 0)?);
            }
            _ => return None,
        }
        i += 1;
    }
    Some(Args { pid: pid?, dry_run, grace })
}

/// Is `pid` a sheepdog (its executable's file name)?
fn is_sheepdog(pid: i32) -> bool {
    os::exe_name(pid).as_deref() == Some("sheepdog")
}

/// The parent of `pid`. Debug seam SHEEPDOG_TEST_PARENT_UNREADABLE=<pid>: that pid's parent
/// cannot be read (as under a `/proc` mounted with `hidepid`).
fn parent(pid: i32) -> Option<i32> {
    if crate::seam_ms("SHEEPDOG_TEST_PARENT_UNREADABLE") == Some(pid as u64) {
        return None;
    }
    os::parent(pid)
}

/// This process and all its ancestors up to pid 1, with their identities: never part of a
/// kill. Err(pid) if the chain cannot be read to its end (`pid` is the link that could not be
/// read): then any process may be an ancestor, and every target is refused (fail closed).
fn protected() -> Result<Vec<(i32, u64)>, i32> {
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
struct Proved {
    known: HashMap<i32, u64>,
    ever: HashSet<u64>,
    protected: Vec<(i32, u64)>,
}

impl Proved {
    fn scan(&mut self) -> Vec<(i32, u64)> {
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
fn end_supervisors(sups: &[(i32, u64)], deadline: Duration, proved: &mut Proved) -> Vec<i32> {
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
    let Some(a) = parse(args) else { return usage() };
    let t = a.pid;
    if t == 1 {
        say!("sheepdog: refusing to kill pid 1: it is the system's init process. Nothing was signalled.");
        return 1;
    }
    let Some(tid) = identity(t) else {
        say!("sheepdog: pid {t} is already gone; nothing to kill.");
        return 0;
    };
    if os::procs().iter().find(|p| p.pid == t && p.id == tid).map(|p| p.uid) != Some(unsafe { libc::getuid() }) {
        say!("sheepdog: refusing to kill pid {t}: it belongs to another user. Nothing was signalled.");
        return 1;
    }
    let protected = match protected() {
        Ok(v) => v,
        Err(link) => {
            say!("sheepdog: refusing to kill pid {t}: sheepdog cannot read pid {link} in its own chain of parent processes (on Linux, /proc mounted with hidepid hides them), so it cannot rule out that pid {t} is one of them. Nothing was signalled.");
            return 1;
        }
    };
    if protected.iter().any(|&(p, id)| p == t && id == tid) {
        let what = if t == unsafe { libc::getpid() } { "it is this sheepdog" } else { "it is an ancestor of this sheepdog (the shell or program that started it)" };
        say!("sheepdog: refusing to kill pid {t}: {what}. Nothing was signalled.");
        return 1;
    }
    let mut proved = Proved { known: HashMap::from([(t, tid)]), ever: HashSet::new(), protected };
    let set = proved.scan();
    let sups: Vec<(i32, u64)> = set.iter().copied().filter(|&(p, _)| is_sheepdog(p)).collect();
    if !is_sheepdog(t) {
        if let Some(s) = job_of(t) {
            // one line, true for the root too: which member is the root is not needed here
            say!("sheepdog: pid {t} belongs to a running job (supervisor pid {s}); this kills {t} and the processes it started, and if {t} is the job's root, the supervisor then ends the whole job. To end the whole job: sheepdog kill {s}");
        }
    }
    if a.dry_run {
        let mut v = set.clone();
        v.sort();
        for (p, _) in v {
            let name = os::exe_name(p).unwrap_or_else(|| "?".into());
            let note = if sups.iter().any(|&(q, _)| q == p) { "\t(a sheepdog: ended first)" } else { "" };
            println!("{p}\t{name}{note}");
        }
        return 0;
    }
    let mut opts = KillOpts::from_env().with_grace(a.grace);
    let unended = if sups.is_empty() { Vec::new() } else { end_supervisors(&sups, opts.deadline, &mut proved) };
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
