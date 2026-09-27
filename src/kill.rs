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

/// This process and its ancestors, with their identities: never part of a kill.
fn protected() -> Vec<(i32, u64)> {
    let mut v = Vec::new();
    let mut p = unsafe { libc::getpid() };
    while p > 1 && v.len() < 4096 {
        if let Some(id) = identity(p) {
            v.push((p, id));
        }
        match os::parent(p) {
            Some(q) => p = q,
            None => break,
        }
    }
    v
}

/// The supervisor of the running job that `t` belongs to, if any: the nearest `sheepdog`
/// ancestor, or (macOS) a `sheepdog` responsible for it.
fn job_of(t: i32) -> Option<i32> {
    let mut p = os::parent(t);
    let mut n = 0;
    while let Some(q) = p.filter(|&q| q > 1 && n < 4096) {
        if is_sheepdog(q) {
            return Some(q);
        }
        p = os::parent(q);
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
/// their grace (read from their argv; 2 s if unreadable) plus the kill deadline.
fn end_supervisors(sups: &[(i32, u64)], deadline: Duration) {
    let mut bound = Duration::ZERO;
    for &(p, id) in sups {
        trace(format!("supervisor {p}"));
        let _ = signal(p, id, libc::SIGTERM);
        let _ = signal(p, id, libc::SIGCONT);
        bound = bound.max(grace_of(p) + deadline);
    }
    let end = Instant::now() + bound;
    while sups.iter().any(|&(p, id)| same(p, id)) && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(10));
    }
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
    let protected = protected();
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
            say!("sheepdog: pid {t} belongs to a running job (supervisor pid {s}); this kills only {t} and the processes it started. To end the whole job: sheepdog kill {s}");
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
    let opts = KillOpts::from_env().with_grace(a.grace);
    if !sups.is_empty() {
        end_supervisors(&sups, opts.deadline);
    }
    let initial: HashMap<i32, u64> = set.into_iter().collect();
    // a signal that ends sheepdog between a freeze and its SIGKILL would leave the tree stopped
    // for good: hold them during the kill, then act on any that arrived (the caller sees sheepdog
    // die of it, as without the hold)
    let held = hold(&[libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGQUIT]);
    let code = match kill_tree(&opts, || proved.scan(), || {}, || None, signal, initial) {
        Ok(()) => 0,
        Err(e) => kill_failed(e),
    };
    unsafe { libc::sigprocmask(libc::SIG_SETMASK, &held, std::ptr::null_mut()) };
    code
}

/// Block `sigs`; returns the previous mask.
fn hold(sigs: &[libc::c_int]) -> libc::sigset_t {
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        for &s in sigs {
            libc::sigaddset(&mut set, s);
        }
        let mut old: libc::sigset_t = std::mem::zeroed();
        libc::sigprocmask(libc::SIG_BLOCK, &set, &mut old);
        old
    }
}
