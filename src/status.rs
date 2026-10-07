//! `--status-fd N` (PLAN.md §3.1; PHASE2.md §1 decision 8): one JSON line, written once at the
//! end, by the supervisor only (a relay drops the fd), within IDLE and CAP (issue #14). Fields: `v`, `job`, `root` (`exited`,
//! `signaled`, `not-started`), `code`, `trigger` (null, `term`; P3 adds `timeout` and `cap`),
//! `trigger_at` (ms since start), `deadline_missed`, `killed` (`[{pid, cmd, escaped}]`, escaped:
//! null | `setsid` | `reparented`), `survivors`, `tracking`, `degraded`, `error`, `notes`. The fd
//! is set CLOEXEC once the supervisor runs (after the macOS SETEXEC), so the job never holds it.
//! The same record feeds the kill report on stderr (PLAN.md §10.4).

use std::io::Write;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub struct Killed {
    pub pid: i32,
    pub cmd: String,
    pub escaped: Option<&'static str>,
}

struct Status {
    fd: Option<i32>,
    start: Option<Instant>,
    job: Option<String>,
    root: &'static str,
    /// set once the root's end is known for certain (an exec that failed): later guesses keep off
    root_final: bool,
    root_pid: i32,
    trigger: Option<(&'static str, u128)>,
    deadline_missed: bool,
    killed: Vec<Killed>,
    survivors: Vec<i32>,
    tracking: &'static str,
    degraded: Option<String>,
    error: Option<String>,
    quiet: bool,
    notes: Vec<String>,
}

static S: Mutex<Status> = Mutex::new(Status {
    fd: None,
    start: None,
    job: None,
    root: "not-started",
    root_final: false,
    root_pid: 0,
    trigger: None,
    deadline_missed: false,
    killed: Vec::new(),
    survivors: Vec::new(),
    tracking: "none",
    degraded: None,
    error: None,
    quiet: false,
    notes: Vec::new(),
});

fn with<R>(f: impl FnOnce(&mut Status) -> R) -> R {
    f(&mut S.lock().unwrap_or_else(|e| e.into_inner()))
}

pub fn start() {
    with(|s| s.start = Some(Instant::now()));
}
pub fn set_fd(fd: i32) {
    with(|s| s.fd = Some(fd));
}
/// The supervisor runs now (after any SETEXEC): the job must not inherit the status fd.
pub fn cloexec() {
    with(|s| {
        if let Some(fd) = s.fd {
            unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
        }
    });
}
pub fn set_job(job: Option<String>) {
    with(|s| s.job = job);
}
pub fn set_root(root: &'static str) {
    with(|s| {
        if !s.root_final {
            s.root = root;
        }
    });
}
/// The root's end, known for certain; no later `set_root` changes it (Linux: an exec that failed).
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub fn set_root_final(root: &'static str) {
    with(|s| {
        s.root = root;
        s.root_final = true;
    });
}
pub fn set_root_pid(pid: i32) {
    with(|s| s.root_pid = pid);
}
pub fn root_pid() -> i32 {
    with(|s| s.root_pid)
}
/// The first trigger wins (PLAN.md §3.1 exit-code order); a later one is a note.
pub fn set_trigger(t: &'static str) {
    with(|s| {
        let at = s.start.map_or(0, |st| st.elapsed().as_millis());
        match s.trigger {
            None => s.trigger = Some((t, at)),
            Some((first, _)) if first != t => s.notes.push(format!("{t} after the trigger")),
            _ => {}
        }
    });
}
/// Whether a trigger is already recorded (a TERM from outside, taken before a cap fired).
pub fn has_trigger() -> bool {
    with(|s| s.trigger.is_some())
}
pub fn set_deadline(alive: &[i32]) {
    with(|s| {
        s.deadline_missed = true;
        s.survivors = alive.to_vec();
    });
}
pub fn add_killed(k: Killed) {
    with(|s| s.killed.push(k));
}
pub fn set_tracking(t: &'static str) {
    with(|s| s.tracking = t);
}
pub fn set_degraded(why: &str) {
    with(|s| s.degraded = Some(why.to_string()));
}
pub fn set_error(e: &str) {
    with(|s| s.error = Some(e.to_string()));
}
pub fn set_quiet(q: bool) {
    with(|s| s.quiet = q);
}
pub fn add_note(n: &str) {
    with(|s| s.notes.push(n.to_string()));
}

/// This process is a relay (sheepr started with children): only the supervisor writes the
/// status line, so the relay drops the fd from its state.
pub fn disable() {
    with(|s| s.fd = None);
}

/// The kill report (PLAN.md §10.4) for a job whose root ended with `root_status` (None: ended
/// by a TERM from outside) and whose kill ended `clean` (no deadline, no internal error).
/// Nothing is printed when nothing was killed. `--quiet` keeps only the degraded line.
pub fn report(root_status: Option<libc::c_int>, clean: bool) {
    // debug seam: the phase-1 hint cells count every stderr line, and the report is not theirs
    if crate::seam_flag("SHEEPR_TEST_NO_REPORT") {
        return;
    }
    let (killed, degraded, quiet) = with(|s| (s.killed.iter().map(|k| (k.pid, k.cmd.clone(), k.escaped)).collect::<Vec<_>>(), s.degraded.clone(), s.quiet));
    if killed.is_empty() {
        return;
    }
    if !quiet {
        // (the cause, for the trace; the words, for the reader)
        let (cause, why) = match root_status {
            Some(st) if libc::WIFSIGNALED(st) => ("signaled".to_string(), format!("the command died of signal {}", libc::WTERMSIG(st))),
            Some(st) => ("exited".to_string(), format!("the command exited {}", libc::WEXITSTATUS(st))),
            None => match crate::caps::by() {
                Some(flag) => (flag.to_string(), format!("the {flag} limit fired")),
                None => ("term".to_string(), "TERM from outside".to_string()),
            },
        };
        crate::note(format!("report-cause {cause}"));
        let escaped: Vec<_> = killed.iter().filter(|k| k.2.is_some()).collect();
        crate::say!(
            "sheepr: {why}: ended the job, {} process{} killed, {} of them had escaped{}",
            killed.len(),
            if killed.len() == 1 { "" } else { "es" },
            escaped.len(),
            if escaped.is_empty() { "." } else { ":" }
        );
        for (pid, cmd, how) in escaped {
            let how = match how {
                Some("setsid") => "escaped with setsid",
                Some("reparented") => "escaped by reparenting",
                _ => "escaped",
            };
            crate::say!("  pid {pid}  {cmd}   {how}");
        }
    }
    if !clean {
        return; // the deadline or internal-error message says why the tree is not clean
    }
    match degraded {
        None if !quiet => {
            crate::say!("sheepr: tree clean. Machine-readable: --status-fd.");
            crate::note("report clean".to_string());
        }
        None => {}
        Some(r) => {
            crate::say!("sheepr: no members left that sheepr could see (tracking degraded: {r}).");
            crate::note("report degraded".to_string());
        }
    }
}

/// How long one write of the status line may go with the fd accepting nothing before sheepr
/// gives up on its reader (issue #14; debug seam SHEEPR_TEST_STATUS_IDLE_MS). A reader that
/// drains its end takes the line at once; one that has stopped must never hold sheepr after the
/// job is killed. Long, so a caller that is alive but stalled (a busy event loop, swap) still
/// gets its line once it holds more than its pipe (64 KiB for a pipe or node's stdio pair, 8 KiB
/// for a raw macOS socketpair).
const IDLE: Duration = Duration::from_secs(10);
/// The longest the line may take in all, however steadily its reader reads (debug seam
/// SHEEPR_TEST_STATUS_CAP_MS).
const CAP: Duration = Duration::from_secs(30);
/// An EINTR this close to the end of its window counts as the timer's tick.
const TICK_SLACK: Duration = Duration::from_millis(10);
/// The notice that the line did not arrive waits at most this long for stderr.
const NOTICE: Duration = Duration::from_secs(1);
/// One write's size: PIPE_BUF, which a pipe takes whole or not at all (as a macOS socket does
/// below its 2048-byte low-water mark), so no write waits on with part of it taken, and a reader
/// that takes part of a write and stops is cut one idle later, not two (review of fa07b06, P3-1).
/// Each write at a new offset starts a new idle window, so a buffer that fills at once is
/// progress, and a reader that reads steadily keeps every window short.
#[cfg(target_os = "macos")]
const CHUNK: usize = 512;
#[cfg(not(target_os = "macos"))]
const CHUNK: usize = 4096;

/// Write the status line (once) and return `code`. A line its reader does not take (the fd
/// accepts nothing for IDLE, the line takes longer than CAP, or the reader has closed its
/// end) is said in one stderr line, also under `--quiet` (the machine-readable record is gone),
/// which waits at most NOTICE; the exit code stays the command's.
pub fn write(code: i32) -> i32 {
    let Some((fd, line)) = line(code) else { return code };
    // sheepr's end: only an exit or a raise follows (die_like resets the one signal it raises).
    // A closed reader must be EPIPE, never a SIGPIPE death, also before setup_signals blocked it
    // (a usage error, an early panic)
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_IGN) };
    let idle = crate::seam_ms("SHEEPR_TEST_STATUS_IDLE_MS").map_or(IDLE, Duration::from_millis);
    let cap = crate::seam_ms("SHEEPR_TEST_STATUS_CAP_MS").map_or(CAP, Duration::from_millis);
    let (total, start) = (line.len(), Instant::now());
    if let Err((cause, why, sent)) = bounded_write(fd, line.as_bytes(), idle, cap) {
        crate::note(format!("status-undelivered {cause} {} {sent}/{total}", start.elapsed().as_millis()));
        // a cut line has no newline: when stderr is the same file (a shell's `3>&2`), the notice
        // still starts a line of its own
        let nl = if sent > 0 && sent < total && same_file(fd, 2) { "\n" } else { "" };
        let said = format!("{nl}sheepr: the status line did not reach fd {fd}: {why} ({sent} of {total} bytes written).\n");
        let notice = idle.min(NOTICE);
        let _ = bounded_write(2, said.as_bytes(), notice, notice);
    }
    code
}

/// Whether fds `a` and `b` are the same file (device and inode).
fn same_file(a: i32, b: i32) -> bool {
    let (mut x, mut y): (libc::stat, libc::stat) = unsafe { (std::mem::zeroed(), std::mem::zeroed()) };
    let read = unsafe { libc::fstat(a, &mut x) == 0 && libc::fstat(b, &mut y) == 0 };
    read && x.st_dev == y.st_dev && x.st_ino == y.st_ino
}

extern "C" fn on_alarm(_: libc::c_int) {}

fn arm(first: Duration, every: Duration) {
    let tv = |d: Duration| libc::timeval { tv_sec: d.as_secs() as _, tv_usec: d.subsec_micros() as _ };
    let it = libc::itimerval { it_value: tv(first), it_interval: tv(every) };
    unsafe { libc::setitimer(libc::ITIMER_REAL, &it, std::ptr::null_mut()) };
}

/// Write all of `b` to `fd` from this thread, in CHUNKs, as plain blocking writes: the fd is
/// never changed (its open file description may be shared, e.g. a shell's `3>&2`), so any kind
/// works. Before each write a SIGALRM timer is set to `idle` (or what is left of `cap`), and it
/// repeats; the handler does nothing and has no SA_RESTART, so the tick ends a write that is
/// waiting: one that took some bytes returns their count, one that took none returns EINTR, and
/// then sheepr gives up. Nothing is left writing when this returns, so the count is exact, and
/// no thread is needed (a job can use up the pids). SIGALRM is unblocked for this thread only,
/// while it writes; sheepr's other thread (macOS `tcc-probe`) starts after setup_signals and
/// blocks it, so the tick reaches this one. A non-blocking fd (the caller's choice; its open file
/// description is shared, so it is left as it is): an EAGAIN waits in poll for room, in the same
/// window, and the tick ends that wait as it ends a write. A signal from outside that ends a
/// write or a poll before the window is up is not the tick: the write is tried again. Stated: a tick that falls between setting the timer
/// and the write itself is missed, and the next one ends the write (twice the limit at worst);
/// a file on a hard network mount can hold a write that no signal ends.
/// Err: (cause, why, bytes written).
fn bounded_write(fd: i32, b: &[u8], idle: Duration, cap: Duration) -> Result<(), (&'static str, String, usize)> {
    let (mut old_act, mut old_mask): (libc::sigaction, libc::sigset_t) = unsafe { (std::mem::zeroed(), std::mem::zeroed()) };
    unsafe {
        let mut act: libc::sigaction = std::mem::zeroed();
        act.sa_sigaction = on_alarm as extern "C" fn(libc::c_int) as libc::sighandler_t;
        libc::sigemptyset(&mut act.sa_mask);
        libc::sigaction(libc::SIGALRM, &act, &mut old_act);
        let mut alrm: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut alrm);
        libc::sigaddset(&mut alrm, libc::SIGALRM);
        libc::pthread_sigmask(libc::SIG_UNBLOCK, &alrm, &mut old_mask);
    }
    let errno = || std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    // debug seam: every write of the status line says EAGAIN and writes nothing, while poll says
    // the fd is ready (a FUSE file can do both)
    let eagain = fd != 2 && crate::seam_flag("SHEEPR_TEST_STATUS_EAGAIN");
    let start = Instant::now();
    // (offset, since, length): the window of the write at that offset; a retry after EAGAIN keeps it
    let (mut off, mut window): (usize, Option<(usize, Instant, Duration)>) = (0, None);
    let r = loop {
        if off == b.len() {
            break Ok(());
        }
        let (since, w) = match window {
            Some((o, since, w)) if o == off => (since, w),
            _ => {
                // past the cap a write gets 1 ms: one that waits ends as `cap` below
                let w = cap.saturating_sub(start.elapsed()).min(idle).max(Duration::from_millis(1));
                // the clock before the timer: the timer's tick then comes a full window after
                // `since`, so an EINTR before that is a signal from outside (below)
                let since = Instant::now();
                arm(w, w);
                window = Some((off, since, w));
                (since, w)
            }
        };
        let end = (off + CHUNK).min(b.len());
        let (n, mut e) = if eagain {
            (-1, libc::EAGAIN)
        } else {
            let n = unsafe { libc::write(fd, b[off..end].as_ptr() as *const libc::c_void, end - off) };
            (n, errno())
        };
        if n > 0 {
            off += n as usize;
            continue;
        }
        if n < 0 && (e == libc::EAGAIN || e == libc::EWOULDBLOCK) {
            // wait for room for what is left of this window: a poll that says ready while each
            // write says EAGAIN (a FUSE file can) still ends with the window, as the tick ends a
            // write (review of 03da4f8, F2)
            let left = w.saturating_sub(since.elapsed());
            let r = if left.is_zero() {
                0
            } else {
                let mut p = libc::pollfd { fd, events: libc::POLLOUT, revents: 0 };
                unsafe { libc::poll(&mut p, 1, (left.as_millis() as libc::c_int).saturating_add(1)) }
            };
            if r > 0 {
                continue;
            }
            e = if r == 0 { libc::EINTR } else { errno() };
        }
        // an EINTR before the window is up is a signal sent from outside (SIGALRM is unblocked
        // while the line is written), not the timer's tick: try again in the same window, so the
        // signal never cuts the line (issue #14). TICK_SLACK takes a tick that comes a little early
        // for the tick.
        if n < 0 && e == libc::EINTR && since.elapsed() + TICK_SLACK < w {
            continue;
        }
        break Err(match (n, e) {
            (0, _) => ("error", "a write took nothing".to_string(), off),
            (_, libc::EINTR) if start.elapsed() >= cap => ("cap", format!("its reader did not take it all within {} ms", cap.as_millis()), off),
            (_, libc::EINTR) => ("timeout", format!("the fd accepted nothing for {} ms", since.elapsed().as_millis()), off),
            (_, libc::EPIPE) => ("closed", "its reader has closed it".to_string(), off),
            (_, e) => ("error", std::io::Error::from_raw_os_error(e).to_string(), off),
        });
    };
    arm(Duration::ZERO, Duration::ZERO);
    unsafe {
        libc::pthread_sigmask(libc::SIG_SETMASK, &old_mask, std::ptr::null_mut());
        libc::sigaction(libc::SIGALRM, &old_act, std::ptr::null_mut());
    }
    r
}

/// The status line, and the fd it goes to (taken: the line is written once).
fn line(code: i32) -> Option<(i32, String)> {
    with(|s| {
        let fd = s.fd.take()?;
        // debug seam: a note of this many bytes, so a test's line is larger than any pipe buffer
        // (a real job reaches that size with a few hundred killed members; seam_ms reads a number)
        if let Some(n) = crate::seam_ms("SHEEPR_TEST_STATUS_PAD") {
            s.notes.push("x".repeat(n as usize));
        }
        let js = crate::journal::json_str;
        let opt = |v: &Option<String>| v.as_deref().map_or_else(|| "null".to_string(), js);
        let killed: Vec<String> = s
            .killed
            .iter()
            .map(|k| format!("{{\"pid\":{},\"cmd\":{},\"escaped\":{}}}", k.pid, js(&k.cmd), k.escaped.map_or_else(|| "null".to_string(), js)))
            .collect();
        let survivors: Vec<String> = s.survivors.iter().map(|p| p.to_string()).collect();
        let notes: Vec<String> = s.notes.iter().map(|n| js(n)).collect();
        let (trigger, at) = match s.trigger {
            Some((t, at)) => (js(t), at.to_string()),
            None => ("null".to_string(), "null".to_string()),
        };
        let line = format!(
            "{{\"v\":1,\"job\":{},\"root\":\"{}\",\"code\":{code},\"trigger\":{trigger},\"trigger_at\":{at},\"deadline_missed\":{},\"killed\":[{}],\"survivors\":[{}],\"tracking\":\"{}\",\"degraded\":{},\"error\":{},\"notes\":[{}]}}\n",
            opt(&s.job),
            s.root,
            s.deadline_missed,
            killed.join(","),
            survivors.join(","),
            s.tracking,
            opt(&s.degraded),
            opt(&s.error),
            notes.join(",")
        );
        Some((fd, line))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The limits README.md, CHANGELOG.md and PLAN.md state (whole-branch review P2-2): 10 s
    /// idle, 30 s cap, 1 s notice. tests/status.rs proves the idle end to end with no seam.
    #[test]
    fn the_stated_limits() {
        assert_eq!((IDLE, CAP, NOTICE), (Duration::from_secs(10), Duration::from_secs(30), Duration::from_secs(1)));
    }
}
