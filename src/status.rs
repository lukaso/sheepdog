//! `--status-fd N` (PLAN.md §3.1; PHASE2.md §1 decision 8): one JSON line, written once at the
//! end, by the supervisor only (a relay drops the fd), within IDLE_MS and CAP_MS (issue #14). Fields: `v`, `job`, `root` (`exited`,
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

/// How long the status line waits while its reader takes nothing (issue #14). A reader that
/// drains its end takes the line at once; one that has stopped must never hold sheepr after the
/// job is killed. Each chunk the reader takes starts the wait again.
const IDLE_MS: u64 = 2000;
/// The longest the line may take in all, however steadily its reader reads (debug seam
/// SHEEPR_TEST_STATUS_CAP_MS).
const CAP_MS: u64 = 10_000;
/// The writer's chunk: a reader that reads slowly but steadily shows progress at least this often.
const CHUNK: usize = 4096;

/// Write the status line (once) and return `code`. A line its reader does not take (it has
/// stopped reading, it is too slow for CAP_MS, or it has closed its end) is said in one stderr
/// line, also under `--quiet` (the machine-readable record is gone), under the same limits; the
/// exit code stays the command's.
pub fn write(code: i32) -> i32 {
    let Some((fd, line)) = line(code) else { return code };
    // sheepr's end: only an exit or a raise follows (die_like resets the one signal it raises).
    // A closed reader must be EPIPE, never a SIGPIPE death, also before setup_signals blocked it
    // (a usage error, an early panic)
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_IGN) };
    let cap = Duration::from_millis(crate::seam_ms("SHEEPR_TEST_STATUS_CAP_MS").unwrap_or(CAP_MS));
    let (total, start) = (line.len(), Instant::now());
    if let Err((cause, why, sent)) = bounded_write(fd, line.into_bytes(), cap) {
        crate::note(format!("status-undelivered {cause} {} {sent}/{total}", start.elapsed().as_millis()));
        let said = format!("sheepr: the status line did not reach fd {fd}: {why} ({sent} of {total} bytes written).\n");
        let _ = bounded_write(2, said.into_bytes(), cap);
    }
    code
}

enum Wrote {
    Progress(usize),
    Done(Result<(), i32>),
}

/// Write all of `b` to `fd`: a thread of its own writes it in CHUNKs while this one waits, and
/// the wait gives up when the reader takes nothing for IDLE_MS, or at `cap` in all. The fd is
/// never changed (its open file description may be shared, e.g. a shell's `3>&2`), so any kind
/// works: a pipe, a socket, a tty, a file. A writer still blocked when this returns ends with
/// the process, which ends right after. An EINTR is tried again (sheepr runs no signal handler,
/// so it is not expected). Err: (cause, why, bytes written).
fn bounded_write(fd: i32, b: Vec<u8>, cap: Duration) -> Result<(), (&'static str, String, usize)> {
    let (tx, rx) = std::sync::mpsc::channel();
    let writer = std::thread::Builder::new().name("status-write".into()).spawn(move || {
        let mut off = 0;
        while off < b.len() {
            let end = (off + CHUNK).min(b.len());
            let n = unsafe { libc::write(fd, b[off..end].as_ptr() as *const libc::c_void, end - off) };
            if n > 0 {
                off += n as usize;
                let _ = tx.send(Wrote::Progress(off));
                continue;
            }
            let e = if n == 0 { 0 } else { std::io::Error::last_os_error().raw_os_error().unwrap_or(0) };
            if e != libc::EINTR {
                let _ = tx.send(Wrote::Done(Err(e)));
                return;
            }
        }
        let _ = tx.send(Wrote::Done(Ok(())));
    });
    if let Err(e) = writer {
        return Err(("thread", format!("sheepr could not start a thread to write it ({e})"), 0));
    }
    let start = Instant::now();
    let (mut sent, mut idle_end) = (0, start + Duration::from_millis(IDLE_MS));
    loop {
        match rx.recv_timeout(idle_end.min(start + cap).saturating_duration_since(Instant::now())) {
            Ok(Wrote::Progress(n)) => (sent, idle_end) = (n, Instant::now() + Duration::from_millis(IDLE_MS)),
            Ok(Wrote::Done(Ok(()))) => return Ok(()),
            Ok(Wrote::Done(Err(libc::EPIPE))) => return Err(("closed", "its reader has closed it".into(), sent)),
            Ok(Wrote::Done(Err(0))) => return Err(("error", "a write took nothing".into(), sent)),
            Ok(Wrote::Done(Err(e))) => return Err(("error", std::io::Error::from_raw_os_error(e).to_string(), sent)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) if start.elapsed() >= cap => return Err(("cap", format!("its reader did not take it all within {} ms", cap.as_millis()), sent)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => return Err(("timeout", format!("its reader took nothing for {IDLE_MS} ms"), sent)),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Err(("error", "the writer stopped".into(), sent)),
        }
    }
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
