//! How the integration tests signal a process (the 2026-09-26 host incident was a test that
//! sent `kill(-1)`). Every signal a test sends goes through one of these doors:
//! - `send`: a process the test identified, as a (pid, identity) pair recorded when the pid was
//!   found; sent only if the pid still has that identity, never to pid 1 or less;
//! - `send_group`: a process group the test created, only while its leader still has the
//!   recorded identity;
//! - `send_child`: a `Child` the test spawned, only while it is not reaped (its zombie holds the
//!   pid). Once `wait` or `try_wait` has seen it end, its pid may belong to anyone.
//!
//! Helpers that find pids by a `ps` scan return (pid, identity) pairs, read right at discovery.

#![allow(dead_code)] // each test file uses a different part

pub use sheepr::json;

use sheepr::ident::{identity, same};
use std::process::{Child, Command};

/// A pid the test found, with its identity read now. None if it is already gone, pid <= 1, or
/// its identity is 0 (on Linux the start tick of a boot-time kernel thread).
pub fn found(pid: i32) -> Option<(i32, u64)> {
    if pid <= 1 {
        return None;
    }
    identity(pid).filter(|&id| id != 0).map(|id| (pid, id))
}

/// The recorded process is still alive (and is still the one that was recorded).
pub fn alive(p: (i32, u64)) -> bool {
    same(p.0, p.1)
}

/// Signal one recorded process, re-checked by identity. Never a group, never pid <= 1.
pub fn send(pid: i32, id: u64, sig: i32) -> bool {
    assert!(pid > 1, "refusing to signal pid {pid}");
    // identity 0 is "unknown" in the fixtures' records, never a process to signal
    id != 0 && same(pid, id) && unsafe { libc::kill(pid, sig) } == 0
}

/// Signal a process group this test created: its leader must still be the recorded process.
pub fn send_group(pgid: i32, leader_id: u64, sig: i32) -> bool {
    assert!(pgid > 1, "refusing to signal group {pgid}");
    leader_id != 0 && same(pgid, leader_id) && unsafe { libc::getpgid(pgid) } == pgid && unsafe { libc::kill(-pgid, sig) } == 0
}

/// Signal a child this test spawned, only while it is not reaped. `try_wait` reaps a child that
/// has exited; std then keeps its status, so a later `wait` still returns it.
pub fn send_child(c: &mut Child, sig: i32) -> bool {
    let pid = c.id() as i32;
    assert!(pid > 1, "refusing to signal pid {pid}");
    match c.try_wait() {
        Ok(None) => (unsafe { libc::kill(pid, sig) }) == 0,
        _ => false,
    }
}

/// Live processes whose `ps -Ao pid=,args=` line carries `marker` as a whole word and passes
/// `keep` (given the line's words: the pid, then argv), each with its identity read right after
/// the scan. A process that ended in between is left out. Err if `ps` cannot run or fails.
pub fn scan(marker: &str, keep: impl Fn(&[&str]) -> bool) -> Result<Vec<(i32, u64)>, String> {
    let out = Command::new("ps").args(["-Ao", "pid=,args="]).output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("ps exited {:?}", out.status.code()));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let w: Vec<&str> = l.split_whitespace().collect();
            if !w.iter().any(|x| *x == marker) || !keep(&w) {
                return None;
            }
            found(w.first()?.parse().ok()?)
        })
        .collect())
}

/// SIGKILL every live process whose argv carries one of `markers`, by identity. Cleanup only:
/// a failed `ps` means nothing is found.
pub fn kill_marked(markers: &[&str]) {
    for m in markers {
        for (p, id) in scan(m, |_| true).unwrap_or_default() {
            send(p, id, libc::SIGKILL);
        }
    }
}

/// PHASE2.md §0.4: the ONE place a cell points sheepr's withheld lines at a sink of its own
/// (a wall-control cell that expects a line); every other run writes to the run's sink, which
/// the runner requires empty. Grep for `cell_sink` to find every override.
pub fn cell_sink(cmd: &mut Command, sink: &std::path::Path) {
    cmd.env("SHEEPR_TEST_SINK", sink);
}

/// Whether a recorded process is stopped now (state T in `ps`).
pub fn stopped(p: (i32, u64)) -> bool {
    if !alive(p) {
        return false;
    }
    let out = Command::new("ps").args(["-o", "stat=", "-p", &p.0.to_string()]).output();
    out.map(|o| String::from_utf8_lossy(&o.stdout).trim_start().starts_with('T')).unwrap_or(false)
}

/// The test environment (PHASE2.md §0.4) is in place: the cargo runner (`scripts/test-env`)
/// started this binary with a test tag, a canary state directory (no sentinel), a withheld sink,
/// and the debug `sheepr` first on PATH, so a nested `sheepr` found by name is this build.
/// Checked once per test binary; every helper that names a binary calls it.
pub fn test_env() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        assert!(cfg!(debug_assertions), "the tests need a debug build: its test walls do not exist in a release build");
        let tag = std::env::var("SHEEPR_TEST_TAG").unwrap_or_default();
        assert!(tag.len() >= 16 && tag.bytes().all(|b| b.is_ascii_hexdigit()), "no test tag: run the tests through cargo (the runner in .cargo/config.toml)");
        let canary = std::path::PathBuf::from(std::env::var_os("SHEEPR_TEST_STATE").unwrap_or_default());
        assert!(canary.is_dir() && !canary.join(".sheepr-test").exists(), "no canary state directory");
        assert!(std::env::var_os("SHEEPR_TEST_SINK").is_some(), "no withheld sink");
        let path = std::env::var("PATH").unwrap_or_default();
        let first = std::path::Path::new(path.split(':').next().unwrap_or("")).join("sheepr");
        let ours = std::fs::canonicalize(env!("CARGO_BIN_EXE_sheepr")).ok();
        assert!(ours.is_some() && std::fs::canonicalize(&first).ok() == ours, "the first sheepr on PATH is not this debug build");
    });
}
