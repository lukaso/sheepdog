//! `--status-fd N` (PLAN.md §3.1; PHASE2.md §1 decision 8): one JSON line, written once at the
//! end, by the supervisor only (a relay drops the fd). Fields: `v`, `job`, `root` (`exited`,
//! `signaled`, `not-started`), `code`, `trigger` (null, `term`; P3 adds `timeout` and `cap`),
//! `trigger_at` (ms since start), `deadline_missed`, `killed` (`[{pid, cmd, escaped}]`, escaped:
//! null | `setsid` | `reparented`), `survivors`, `tracking`, `degraded`, `error`, `notes`. The fd
//! is set CLOEXEC once the supervisor runs (after the macOS SETEXEC), so the job never holds it.
//! The same record feeds the kill report on stderr (PLAN.md §10.4).

use std::io::Write;
use std::sync::Mutex;
use std::time::Instant;

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

/// This process is a relay (sheepdog started with children): only the supervisor writes the
/// status line, so the relay drops the fd from its state.
pub fn disable() {
    with(|s| s.fd = None);
}

/// The kill report (PLAN.md §10.4) for a job whose root ended with `root_status` (None: ended
/// by a TERM from outside) and whose kill ended `clean` (no deadline, no internal error).
/// Nothing is printed when nothing was killed. `--quiet` keeps only the degraded line.
pub fn report(root_status: Option<libc::c_int>, clean: bool) {
    // debug seam: the phase-1 hint cells count every stderr line, and the report is not theirs
    if crate::seam_flag("SHEEPDOG_TEST_NO_REPORT") {
        return;
    }
    let (killed, degraded, quiet) = with(|s| (s.killed.iter().map(|k| (k.pid, k.cmd.clone(), k.escaped)).collect::<Vec<_>>(), s.degraded.clone(), s.quiet));
    if killed.is_empty() {
        return;
    }
    if !quiet {
        let why = match root_status {
            Some(st) if libc::WIFSIGNALED(st) => format!("the command died of signal {}", libc::WTERMSIG(st)),
            Some(st) => format!("the command exited {}", libc::WEXITSTATUS(st)),
            None => "TERM from outside".to_string(),
        };
        let escaped: Vec<_> = killed.iter().filter(|k| k.2.is_some()).collect();
        crate::say!(
            "sheepdog: {why}: ended the job, {} process{} killed, {} of them had escaped{}",
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
            crate::say!("sheepdog: tree clean. Machine-readable: --status-fd.");
            crate::note("report clean".to_string());
        }
        None => {}
        Some(r) => {
            crate::say!("sheepdog: no members left that sheepdog could see (tracking degraded: {r}).");
            crate::note("report degraded".to_string());
        }
    }
}

/// Write the status line (once) and return `code`.
pub fn write(code: i32) -> i32 {
    with(|s| {
        let Some(fd) = s.fd.take() else { return };
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
        let b = line.as_bytes();
        let mut off = 0;
        while off < b.len() {
            let n = unsafe { libc::write(fd, b[off..].as_ptr() as *const libc::c_void, b.len() - off) };
            if n <= 0 {
                break;
            }
            off += n as usize;
        }
    });
    code
}
