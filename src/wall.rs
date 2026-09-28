//! The test walls inside the signal door (PHASE2.md §0.1). Debug builds only: a release build
//! admits every signal here and refuses to run at all in a test environment (`main`).
//!
//! The latch is one-way: it only ever becomes true. It is set by the gate call, which every
//! phase-2 source must make before its first fact can reach the door (the token it returns is
//! the only way to get one), or by the debug seam SHEEPDOG_TEST_LATCH=1. While it is on, the door
//! signals a target only if the target's environment block carries the whole entry
//! `SHEEPDOG_TEST_TAG=<this process's own tag>`. A process without a tag of its own withholds
//! every target. The verdict is taken once per (pid, identity read at the moment of the check),
//! so the CONT after a KILL does not re-read a dying process, and a pid reused by another process
//! gets a verdict of its own.

use sheepdog::envtag::{self, EnvRead, TagVerdict};
use sheepdog::ident::identity;
use std::collections::{HashMap, HashSet};
use std::os::raw::c_int;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

/// The one-way latch.
pub struct Latch(AtomicBool);

impl Latch {
    pub const fn new() -> Self {
        Latch(AtomicBool::new(false))
    }
    pub fn on(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
    fn set(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// This process's latch.
pub static LATCH: Latch = Latch::new();

/// A phase-2 source's permission to produce facts that reach the door. Only `gate` makes one.
#[allow(dead_code)] // its sources arrive from P1 on (PHASE2.md §2)
pub struct Token(());

/// The gate call on `latch`: None under the phase-1 opt-out (the source is disabled outright);
/// otherwise the latch is set first, then the token returned.
#[allow(dead_code)]
pub fn gate_with(latch: &Latch, opt_out: bool) -> Option<Token> {
    if opt_out {
        return None;
    }
    latch.set();
    Some(Token(()))
}

/// The gate call every phase-2 source makes (the phase-1 opt-out is SHEEPDOG_TEST_PHASE1=1).
#[allow(dead_code)]
pub fn gate() -> Option<Token> {
    let t = gate_with(&LATCH, crate::seam_flag("SHEEPDOG_TEST_PHASE1"));
    if t.is_none() {
        crate::note("source disabled".to_string());
    }
    t
}

fn own_tag() -> Option<&'static str> {
    static TAG: OnceLock<Option<String>> = OnceLock::new();
    TAG.get_or_init(|| std::env::var("SHEEPDOG_TEST_TAG").ok().filter(|t| !t.is_empty())).as_deref()
}

/// Verdicts taken, by (pid, identity at the check); and the targets already signalled once
/// (for the debug seam SHEEPDOG_TEST_ENV_EMPTY_AFTER_FIRST).
fn cache() -> &'static Mutex<(HashMap<(i32, u64), TagVerdict>, HashSet<(i32, u64)>)> {
    static C: OnceLock<Mutex<(HashMap<(i32, u64), TagVerdict>, HashSet<(i32, u64)>)>> = OnceLock::new();
    C.get_or_init(|| Mutex::new((HashMap::new(), HashSet::new())))
}

fn verdict_for(pid: i32, id: u64, tag: &str) -> TagVerdict {
    let Some(live) = identity(pid) else { return TagVerdict::Gone };
    // the pid is no longer the process the caller means: nothing may be sent to it, and it is
    // no untagged target either (the identity check would refuse it too)
    if live != id {
        return TagVerdict::Gone;
    }
    let seam_empty = {
        let c = cache().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(&v) = c.0.get(&(pid, live)) {
            return v;
        }
        // debug seam: a read after this target's first signal finds an empty environment
        crate::seam_flag("SHEEPDOG_TEST_ENV_EMPTY_AFTER_FIRST") && c.1.contains(&(pid, live))
    }; // the lock is not held while the reads below wait
    // debug seam SHEEPDOG_TEST_ENV_EMPTY_READS=N: the first N reads of a target are empty
    let mut empty_reads = crate::seam_ms("SHEEPDOG_TEST_ENV_EMPTY_READS").unwrap_or(0);
    let mut read_once = || {
        if seam_empty {
            return EnvRead::Block(Vec::new());
        }
        if empty_reads > 0 {
            empty_reads -= 1;
            return EnvRead::Block(Vec::new());
        }
        envtag::read_env(pid)
    };
    let mut read = read_once();
    // Linux shows an empty environment for a moment inside an exec (before the new image's
    // stack is set up): read again, 2 ms apart, for up to 20 ms, while the process is still the
    // same one and not exiting, before calling an empty block untagged
    for _ in 0..10 {
        if !matches!(read, EnvRead::Block(ref e) if e.is_empty()) || envtag::exiting(pid) || identity(pid) != Some(live) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
        read = read_once();
    }
    let mut c = cache().lock().unwrap_or_else(|e| e.into_inner());
    // the read must describe the process whose identity was taken
    if identity(pid) != Some(live) {
        return TagVerdict::Gone;
    }
    let tagged = matches!(read, EnvRead::Block(ref e) if envtag::has_tag(e, tag));
    // exiting, or reaped and reused meanwhile: either way not the process to signal
    let exiting = !tagged && (envtag::exiting(pid) || identity(pid) != Some(live));
    let v = envtag::verdict(&read, tag, exiting);
    // a verdict from an empty block is never kept: it may be a moment of an exec
    let empty = matches!(read, EnvRead::Block(ref e) if e.is_empty());
    if v == TagVerdict::Tagged || (v == TagVerdict::Withheld && !empty) {
        c.0.insert((pid, live), v);
    }
    if v == TagVerdict::Tagged {
        c.1.insert((pid, live));
    }
    v
}

/// The rollback's tripwire (PHASE2.md D4): the rollback CONT is not held back by the wall (it
/// undoes this supervisor's own STOP), but a rollback that reaches a process without the tag
/// means that STOP breached the wall, so it is reported to the sink and the test run goes red.
pub fn rollback_tripwire(pid: i32, id: u64) {
    if !cfg!(debug_assertions) || !LATCH.on() {
        return;
    }
    let v = match own_tag() {
        Some(tag) => verdict_for(pid, id, tag),
        None => TagVerdict::Withheld,
    };
    if matches!(v, TagVerdict::Withheld | TagVerdict::Unreadable) {
        let line = format!("rollback-untagged {pid} {}", libc::SIGCONT);
        crate::trace(line.clone());
        if let Ok(p) = std::env::var("SHEEPDOG_TEST_SINK") {
            crate::trace_to(std::path::Path::new(&p), &line);
        }
    }
}

/// The door's wall check for `sig` to `pid` (called after the inert seam and, on Linux, after
/// the pidfd is open, before the identity check). True = the door may go on.
pub fn admit(pid: i32, id: u64, sig: c_int) -> bool {
    if !cfg!(debug_assertions) {
        return true;
    }
    if crate::seam_flag("SHEEPDOG_TEST_LATCH") {
        LATCH.set();
    }
    if !LATCH.on() {
        return true;
    }
    let v = match own_tag() {
        Some(tag) => verdict_for(pid, id, tag),
        None => TagVerdict::Withheld,
    };
    let word = match v {
        TagVerdict::Tagged => return true,
        TagVerdict::Gone => {
            crate::trace(format!("gone {pid} {sig}"));
            return false;
        }
        TagVerdict::Withheld => "withheld",
        TagVerdict::Unreadable => "unreadable",
    };
    let line = format!("{word} {pid} {sig}");
    crate::trace(line.clone());
    if let Ok(p) = std::env::var("SHEEPDOG_TEST_SINK") {
        crate::trace_to(std::path::Path::new(&p), &line);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The gate sets the latch before it hands out a token; under the opt-out it hands out
    /// none and leaves the latch as it was.
    #[test]
    fn the_gate_sets_the_latch_or_disables_the_source() {
        let l = Latch::new();
        assert!(!l.on());
        assert!(gate_with(&l, true).is_none(), "the opt-out disables the source");
        assert!(!l.on(), "and does not set the latch");
        assert!(gate_with(&l, false).is_some());
        assert!(l.on(), "a token only with the latch on");
    }
}
