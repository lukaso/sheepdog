//! `--status-fd N` (PLAN.md §3.1; PHASE2.md §1 decision 8). P1 writes a minimal line: `job`,
//! `root` (`exited`, `signaled` or `not-started`), `code` and `notes`; P2 completes the schema.
//! One line, written once at the end.

use std::sync::Mutex;

struct Status {
    fd: Option<i32>,
    job: Option<String>,
    root: &'static str,
    /// set once the root's end is known for certain (an exec that failed): later guesses keep off
    root_final: bool,
    notes: Vec<String>,
}

static S: Mutex<Status> = Mutex::new(Status { fd: None, job: None, root: "not-started", root_final: false, notes: Vec::new() });

fn with<R>(f: impl FnOnce(&mut Status) -> R) -> R {
    f(&mut S.lock().unwrap_or_else(|e| e.into_inner()))
}

pub fn set_fd(fd: i32) {
    with(|s| s.fd = Some(fd));
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
pub fn add_note(n: &str) {
    with(|s| s.notes.push(n.to_string()));
}

/// This process is a relay (sheepdog started with children): only the supervisor writes the
/// status line, so the relay drops the fd from its state.
pub fn disable() {
    with(|s| s.fd = None);
}

/// Write the status line (once) and return `code`.
pub fn write(code: i32) -> i32 {
    with(|s| {
        let Some(fd) = s.fd.take() else { return };
        let job = s.job.as_deref().map_or_else(|| "null".to_string(), crate::journal::json_str);
        let notes: Vec<String> = s.notes.iter().map(|n| crate::journal::json_str(n)).collect();
        let line = format!(
            "{{\"v\":1,\"job\":{job},\"root\":\"{}\",\"code\":{code},\"notes\":[{}]}}\n",
            s.root,
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
