//! The caps (PLAN.md §3.1, §3.4; PHASE2.md §1 decisions 9-10): `--timeout` (running time: the
//! time sheepdog spends stopped by its own job control is not counted; a discarded stop costs
//! nothing, so then all time counts), `--max-mem` (the sum over live members: macOS
//! `ri_phys_footprint`; Linux `smaps_rollup` Pss, `statm` RSS where that is missing), and
//! `--max-procs` (live members). Evaluated on every tick while the job runs and on every scan
//! while it is being ended, in the order timeout, memory, processes. The first to fire is the
//! trigger (exit 124); a later one is a note; one that first fires after the root ended by
//! itself is a note only (the exit code stays the command's).

use std::sync::Mutex;
use std::time::{Duration, Instant};

struct Caps {
    timeout: Option<Duration>,
    max_mem: Option<u64>,
    max_procs: Option<usize>,
    start: Instant,
    stopped_for: Duration,
    root_ended: bool,
    fired: Vec<&'static str>,
    triggered: bool,
}

static C: Mutex<Option<Caps>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Caps) -> R) -> Option<R> {
    C.lock().unwrap_or_else(|e| e.into_inner()).as_mut().map(f)
}

/// Set the caps for this run (none at all: nothing is ever evaluated).
pub fn set(timeout: Option<Duration>, max_mem: Option<u64>, max_procs: Option<usize>) {
    if timeout.is_some() || max_mem.is_some() || max_procs.is_some() {
        *C.lock().unwrap_or_else(|e| e.into_inner()) = Some(Caps { timeout, max_mem, max_procs, start: Instant::now(), stopped_for: Duration::ZERO, root_ended: false, fired: Vec::new(), triggered: false });
    }
}

/// Sheepdog was stopped by its own job control for `d`: that time does not run the job.
pub fn exclude(d: Duration) {
    with(|c| c.stopped_for += d);
}

/// The root ended by itself: a cap that first fires from now on is a note only.
pub fn root_ended() {
    with(|c| c.root_ended = true);
}

/// Whether a cap or the timeout ended the job (so the exit code is 124).
pub fn triggered() -> bool {
    with(|c| c.triggered).unwrap_or(false)
}

/// Evaluate the caps against `members`; true if one fired now and ends the job.
pub fn check(members: &[(i32, u64)]) -> bool {
    with(|c| {
        let running = c.start.elapsed().saturating_sub(c.stopped_for);
        let mut now: Vec<&'static str> = Vec::new();
        if c.timeout.is_some_and(|t| running >= t) {
            now.push("timeout");
        }
        if let Some(max) = c.max_mem {
            let sum: u64 = members.iter().map(|&(p, _)| mem_of(p)).sum();
            if sum > max {
                now.push("cap");
            }
        }
        if c.max_procs.is_some_and(|m| members.len() > m) && !now.contains(&"cap") {
            now.push("cap");
        }
        let mut ends = false;
        for n in now {
            if c.fired.contains(&n) {
                continue;
            }
            c.fired.push(n);
            if c.root_ended {
                crate::status::add_note(&format!("{n} after the command ended"));
            } else if !c.triggered && crate::status::has_trigger() {
                // a TERM from outside came first: it stays the trigger, this is a note
                crate::status::set_trigger(n);
            } else if !c.triggered {
                c.triggered = true;
                crate::status::set_trigger(n);
                ends = true;
            } else {
                crate::status::set_trigger(n); // a note: the first trigger stays
            }
        }
        ends
    })
    .unwrap_or(false)
}

/// A process's memory in bytes (0 when unreadable).
#[cfg(target_os = "macos")]
fn mem_of(pid: i32) -> u64 {
    let mut ri: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
    let r = unsafe { libc::proc_pid_rusage(pid, libc::RUSAGE_INFO_V4, &mut ri as *mut _ as *mut libc::rusage_info_t) };
    if r == 0 {
        ri.ri_phys_footprint
    } else {
        0
    }
}

#[cfg(target_os = "linux")]
fn mem_of(pid: i32) -> u64 {
    if let Ok(s) = std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup")) {
        if let Some(kb) = s.lines().find(|l| l.starts_with("Pss:")).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse::<u64>().ok()) {
            return kb * 1024;
        }
    }
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(4096) as u64;
    std::fs::read_to_string(format!("/proc/{pid}/statm")).ok().and_then(|s| s.split_whitespace().nth(1)?.parse::<u64>().ok()).map_or(0, |p| p * page)
}

/// `500M`, `1G`, `512K` or plain bytes (binary units); None for anything else.
pub fn parse_size(s: &str) -> Option<u64> {
    let (num, mul) = match s.chars().last()? {
        'K' | 'k' => (&s[..s.len() - 1], 1024),
        'M' | 'm' => (&s[..s.len() - 1], 1024 * 1024),
        'G' | 'g' => (&s[..s.len() - 1], 1024 * 1024 * 1024),
        _ => (s, 1),
    };
    num.parse::<u64>().ok().filter(|&n| n > 0)?.checked_mul(mul)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_parse_in_binary_units() {
        assert_eq!(parse_size("500M"), Some(500 * 1024 * 1024));
        assert_eq!(parse_size("1G"), Some(1024 * 1024 * 1024));
        assert_eq!(parse_size("4096"), Some(4096));
        assert_eq!(parse_size("0M"), None);
        assert_eq!(parse_size("M"), None);
        assert_eq!(parse_size("1.5G"), None);
    }
}
