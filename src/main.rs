//! sheepdog, phase-0 spike (PLAN.md §7).
//!
//! `sheepdog run [--mode M] -- cmd...` runs cmd, waits for it, then kills every member of its
//! tree (PLAN.md §3.3: freeze, close, kill, repeat until the tree is empty, or the deadline).
//!
//! Modes (the non-default ones exist so tests can prove the default is what catches escapees):
//!   macOS: `responsible` (default: the supervisor re-execs itself with disclaim and members
//!          are processes whose responsible uniqueid is the supervisor's), `root-disclaim`
//!          (the round-2 design: the root disclaims; loses escapees once the root exits)
//!   Linux: `subreaper` (default: PR_SET_CHILD_SUBREAPER; members are the supervisor's
//!          descendants), `none` (no subreaper; orphans go to PID 1)
//!
//! `#![no_main]`: Rust's runtime sets SIGPIPE to ignored before a normal `main` runs, and the
//! job would inherit that. With our own C `main` the runtime does not touch any signal, so the
//! root inherits the caller's dispositions and mask (PLAN.md §3.1, cell 23), with one stated
//! exception: SIGCHLD is set to default, because a supervisor whose children are reaped
//! automatically cannot wait for them (phase-0 fix review, P1-B).
#![cfg_attr(not(test), no_main)]
#![cfg_attr(test, allow(dead_code))]

mod caps;
mod journal;
mod doctor;
mod kill;
mod strays;
#[cfg(target_os = "macos")]
mod register;
mod state;
mod status;
mod sweep;
mod wall;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

use sheepdog::ident::same;
use std::collections::HashMap;
use std::ffi::{CString, OsString};
use std::io::Write;
use std::os::raw::c_int;
use std::os::unix::ffi::OsStrExt;
use std::time::{Duration, Instant};

/// Write a line to stderr, ignoring errors: a closed stderr must never become a panic (and a
/// panic must never unwind out of the C `main`).
macro_rules! say {
    ($($t:tt)*) => {{
        let __m = format!($($t)*);
        crate::remember_say(&__m);
        let _ = writeln!(std::io::stderr(), "{}", __m);
    }};
}
pub(crate) use say;

/// `say!` a failure and record it as the command's error (the first one wins): a `--json`
/// error names it (PLAN.md §10.5). Every line said before a non-zero exit that states why is
/// a `fail!`; context said after it stays a `say!`.
macro_rules! fail {
    ($($t:tt)*) => {{
        let __m = format!($($t)*);
        crate::record_failure(&__m);
        let _ = writeln!(std::io::stderr(), "{}", __m);
    }};
}
pub(crate) use fail;

/// The last line `say!` wrote, and the first failure `fail!` recorded.
fn said() -> &'static std::sync::Mutex<(String, Option<String>)> {
    static L: std::sync::OnceLock<std::sync::Mutex<(String, Option<String>)>> = std::sync::OnceLock::new();
    L.get_or_init(|| std::sync::Mutex::new((String::new(), None)))
}

pub(crate) fn remember_say(m: &str) {
    if let Ok(mut l) = said().lock() {
        l.0 = m.to_string();
    }
}

pub(crate) fn record_failure(m: &str) {
    if let Ok(mut l) = said().lock() {
        l.1.get_or_insert_with(|| m.to_string());
    }
}

/// The command's error message: its first recorded failure (else the last line said).
fn error_message() -> String {
    said().lock().map(|l| l.1.clone().unwrap_or_else(|| l.0.clone())).unwrap_or_default()
}

/// The help screen (PLAN.md §10.5): examples first.
const HELP: &str = "sheepdog: run a command and make sure every process it starts is gone at the end,
including processes that escaped with setsid, double-forks or reparenting.

  sheepdog run --timeout 5m -- npm test        stop the whole tree after 5 minutes
  sheepdog run --max-mem 2G -- python3 job.py  stop it if the tree uses more than 2 GB
  sheepdog strays                              list leaked processes of yours, biggest first
  sheepdog kill 4242                           kill 4242 and the processes it provably started
                                               (see first: sheepdog ps 4242)

To stop a running job, send TERM to sheepdog. Exit 124 means a limit fired.
Commands: run, kill, strays, ps, sweep, doctor. `sheepdog help <command>` for details.";

const USAGE_RUN: &str = "sheepdog run [--timeout DURATION] [--max-mem SIZE] [--max-procs N] [--grace DURATION] [--kill-deadline DURATION] [--leave-strays] [--no-sweep] [--inherit-terminal-permissions] [--quiet] [--forward-int-to-root] [--owner NAME] [--status-fd N] [--mode M] -- command [args...]";

/// `sheepdog help <command>`: that command's usage (one home: each module's own text).
fn help(cmd: Option<&OsString>) -> i32 {
    let (what, usage) = match cmd.map(|c| c.as_bytes()) {
        None => {
            let _ = writeln!(std::io::stdout(), "{HELP}");
            return 0;
        }
        Some(b"run") => ("run a command; when it ends, end every process it started", USAGE_RUN),
        Some(b"kill") => ("kill a process and the processes it provably started", kill::USAGE_KILL),
        Some(b"ps") => ("list what `kill` would kill, and why (it signals nothing)", kill::USAGE_PS),
        Some(b"strays") => ("list your leaked processes, biggest first; --kill kills the matching ones", strays::USAGE),
        Some(b"sweep") => ("end the processes of your dead jobs, from their journals", sweep::USAGE),
        Some(b"doctor") => ("which mechanisms work on this machine, and what is degraded", doctor::USAGE),
        Some(_) => {
            say!("sheepdog: no such command. Commands: run, kill, strays, ps, sweep, doctor.");
            return 2;
        }
    };
    let _ = writeln!(std::io::stdout(), "{what}\n\nusage: {usage}");
    0
}

/// Whether `args` of subcommand `sub` hold the `--json` flag: the word as a flag, wherever it
/// stands, never as the value of an option that takes one (`strays --cmd --json`), so a usage
/// error before or after it still gets its JSON error.
fn json_flag(sub: &str, args: &[OsString]) -> bool {
    let takes_value: &[&[u8]] = match sub {
        "kill" => &[b"--grace"],
        "strays" => &[b"--min-mem", b"--older-than", b"--cmd", b"--pid"],
        "sweep" => &[b"--owner"],
        _ => &[],
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_bytes() {
            b"--json" => return true,
            b"--" => return false,
            x if takes_value.contains(&x) => {
                it.next();
            }
            _ => {}
        }
    }
    false
}

/// `s` as one shell word: as it is when it holds only characters no shell treats specially and
/// does not start with `=` (zsh expands `=cmd` to its path), else single-quoted (a `'` inside
/// becomes `'\''`).
fn shell_quote(s: &str) -> String {
    if !s.is_empty() && !s.starts_with('=') && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-./:=@%+,".contains(&b)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// `--json` errors (PLAN.md §10.5): a command whose parser took `--json` and that ends with a
/// non-zero code writes one line `{"v":1,"error":{"code","message","fix"}}` to stdout.
fn json_error(sub: &str, args: &[OsString], code: i32) -> i32 {
    if code == 0 || !json_flag(sub, args) {
        return code;
    }
    let (name, fix) = match code {
        2 => ("usage", format!("sheepdog help {sub}")),
        1 => ("refused", String::new()),
        125 => ("deadline", "sheepdog ps to see what is left; sheepdog sweep to end a dead job's processes".to_string()),
        _ => ("failed", String::new()),
    };
    let message = error_message();
    let j = journal::json_str;
    let _ = writeln!(std::io::stdout(), "{{\"v\":1,\"error\":{{\"code\":{},\"message\":{},\"fix\":{}}}}}", j(name), j(&message), j(&fix));
    code
}

pub struct Args {
    /// argv exactly as received (raw bytes), for the macOS self re-exec
    pub argv: Vec<OsString>,
    pub mode: Option<String>,
    pub cmd: Vec<OsString>,
    /// §3.3 step 1: how long members get after TERM before SIGKILL (default 2 s)
    pub grace: Duration,
    /// skip the kill when the root exits normally (a TERM still ends the job)
    pub leave_strays: bool,
    /// no D9 hint (PLAN.md §3.1)
    pub quiet: bool,
    /// forward an INT to the root as well (callers that signal only the sheepdog pid)
    pub forward_int_to_root: bool,
    /// the owner tag in the journal header (PLAN.md §3.5; default `default`)
    pub owner: String,
    /// `--status-fd N`: the status line goes to this fd at the end
    pub status_fd: Option<i32>,
    /// skip the auto-sweep before the command (PLAN.md §3.5)
    pub no_sweep: bool,
    /// macOS: no disclaim; the job keeps the terminal's privacy permissions and tracking falls
    /// back to parent ids (PLAN.md §4.4, DevEx D11). Linux: accepted, no effect.
    pub inherit: bool,
    /// the caps (PLAN.md §3.4) and the kill deadline (§3.3)
    pub timeout: Option<Duration>,
    pub max_mem: Option<u64>,
    pub max_procs: Option<usize>,
    pub kill_deadline: Option<Duration>,
}

/// A duration: `500ms`, or a number (fractions allowed) with `s`, `m`, `h`, `d` or no unit
/// (seconds), at most one day (a grace, a kill deadline). Anything else, including a sign, an
/// exponent, `inf` and `NaN`, is None (a usage error; `Duration::from_secs_f64` would panic on
/// some of them).
fn parse_duration(s: &str) -> Option<Duration> {
    parse_duration_max(s, Duration::from_secs(86_400))
}

/// A duration of a job's own length (`--timeout`, `strays --older-than`): at most 365 days.
fn parse_long_duration(s: &str) -> Option<Duration> {
    parse_duration_max(s, Duration::from_secs(365 * 86_400))
}

fn parse_duration_max(s: &str, max: Duration) -> Option<Duration> {
    let d = if let Some(ms) = s.strip_suffix("ms") {
        // digits only: `u64::from_str` would take a leading `+`
        if ms.is_empty() || !ms.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        Duration::from_millis(ms.parse().ok()?)
    } else {
        let (n, unit) = match s.as_bytes().last()? {
            b's' => (&s[..s.len() - 1], 1.0),
            b'm' => (&s[..s.len() - 1], 60.0),
            b'h' => (&s[..s.len() - 1], 3600.0),
            b'd' => (&s[..s.len() - 1], 86_400.0),
            _ => (s, 1.0),
        };
        // digits and one dot only: no sign, exponent, "inf" or "nan"
        if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
            return None;
        }
        let secs = n.parse::<f64>().ok()? * unit;
        if !(0.0..=max.as_secs_f64()).contains(&secs) {
            return None;
        }
        Duration::from_secs_f64(secs)
    };
    (d <= max).then_some(d)
}

/// Arguments as C strings, byte for byte (non-UTF-8 arguments pass unchanged). An argument
/// with an interior NUL cannot come from a C argv; if one ever arrives it is an error, never a
/// silently different argv (review round 3, F6).
pub fn cstrings(v: &[OsString]) -> Result<Vec<CString>, String> {
    v.iter()
        .map(|s| CString::new(s.as_bytes()).map_err(|_| format!("argument {:?} contains a NUL byte", s)))
        .collect()
}

/// `--status-fd N` among `run`'s options (before `--`), if N is a valid fd number.
fn prescan_status_fd(argv: &[OsString]) -> Option<i32> {
    let args = &argv[1.min(argv.len())..];
    if args.first().map(|a| a.as_bytes()) != Some(b"run") {
        return None;
    }
    let end = args.iter().position(|a| a.as_bytes() == b"--").unwrap_or(args.len());
    // the last one counts, as in `parse`; 0-2 are the command's own streams, never the status fd
    let i = args[..end].iter().rposition(|a| a.as_bytes() == b"--status-fd")?;
    args.get(i + 1).filter(|_| i + 1 < end)?.to_str()?.parse().ok().filter(|&n: &i32| n >= 3)
}

fn usage() -> i32 {
    say!("usage: {USAGE_RUN}");
    125
}

/// A `run` usage error: what was wrong (the rejected word, the rule it broke, or the corrected
/// command), then the usage line. Exit 125.
fn usage_because(why: String) -> i32 {
    fail!("sheepdog: {why}");
    usage()
}

/// The rules a value keeps, as a usage error says them (one home for every command).
pub(crate) const RULE_GRACE: &str = "a duration: a number with ms, s, m, h or d (a bare number is seconds), at most 1d, for example 2s";
pub(crate) const RULE_AGE: &str = "a duration: a number with ms, s, m, h or d (a bare number is seconds), at most 365d, for example 10m";
pub(crate) const RULE_SIZE: &str = "a size: a whole number above 0 with K, M or G (binary: 1G is 1024M), for example 2G";

/// `run`'s options that take a value, with the rule a value must keep (said in a usage error).
fn value_rule(flag: &[u8]) -> Option<&'static str> {
    Some(match flag {
        b"--timeout" => "a duration: a number with ms, s, m, h or d (a bare number is seconds), above 0 and at most 365d, for example 5m",
        b"--kill-deadline" => "a duration: a number with ms, s, m, h or d (a bare number is seconds), above 0 and at most 1d, for example 30s",
        b"--grace" => RULE_GRACE,
        b"--max-mem" => RULE_SIZE,
        b"--max-procs" => "a count: a whole number above 0, for example 200",
        b"--status-fd" => "a file descriptor number, 3 or more",
        b"--owner" | b"--mode" => "a word",
        _ => return None,
    })
}

/// A word as a usage error shows it (control characters and bytes that are not UTF-8 escaped).
pub(crate) fn shown(a: &OsString) -> String {
    kill::clean(&a.to_string_lossy())
}

fn parse(argv: Vec<OsString>) -> Result<Args, i32> {
    let args = &argv[1.min(argv.len())..];
    if args.first().map(|a| a.as_bytes()) != Some(b"run") {
        return Err(usage());
    }
    // without `--` the options end at the first word that is not one: there the command starts
    let sep = args.iter().position(|a| a.as_bytes() == b"--");
    let end = sep.unwrap_or(args.len());
    let mut mode = None;
    let mut grace = Duration::from_secs(2);
    let mut leave_strays = false;
    let mut quiet = false;
    let mut forward_int_to_root = false;
    let mut owner = "default".to_string();
    let mut status_fd = None;
    let (mut timeout, mut max_mem, mut max_procs, mut kill_deadline) = (None, None, None, None);
    let mut no_sweep = false;
    let mut inherit = false;
    let mut i = 1;
    while i < end {
        let flag = args[i].as_bytes();
        if let Some(rule) = value_rule(flag) {
            let name = shown(&args[i]);
            let Some(raw) = args.get(i + 1).filter(|_| i + 1 < end) else {
                return Err(usage_because(format!("{name} needs a value before --. It must be {rule}.")));
            };
            let v = raw.to_string_lossy();
            let bad = || usage_because(format!("{name} {} is not valid. It must be {rule}.", shown(raw)));
            match flag {
                b"--mode" => mode = Some(v.into_owned()),
                b"--timeout" => timeout = Some(parse_long_duration(&v).filter(|d| !d.is_zero()).ok_or_else(bad)?),
                b"--kill-deadline" => kill_deadline = Some(parse_duration(&v).filter(|d| !d.is_zero()).ok_or_else(bad)?),
                b"--max-mem" => max_mem = Some(caps::parse_size(&v).ok_or_else(bad)?),
                b"--max-procs" => max_procs = Some(raw.to_str().and_then(|v| v.parse::<usize>().ok()).filter(|&n| n > 0).ok_or_else(bad)?),
                b"--owner" => owner = v.into_owned(),
                // 0-2 would be the command's own streams (the status fd is set close-on-exec)
                b"--status-fd" => status_fd = Some(raw.to_str().and_then(|v| v.parse::<i32>().ok()).filter(|&n| n >= 3).ok_or_else(bad)?),
                _ => grace = parse_duration(&v).ok_or_else(bad)?,
            }
            i += 2;
            continue;
        }
        match flag {
            b"--leave-strays" => leave_strays = true,
            b"--no-sweep" => no_sweep = true,
            b"--inherit-terminal-permissions" => inherit = true,
            b"--quiet" => quiet = true,
            b"--forward-int-to-root" => forward_int_to_root = true,
            _ if sep.is_none() && !flag.starts_with(b"-") => break,
            _ if flag.starts_with(b"-") => return Err(usage_because(format!("unknown option {}. Options go between run and --. `sheepdog help run` lists them.", shown(&args[i])))),
            _ => return Err(usage_because(format!("{} is not an option of run. The command goes after --.", shown(&args[i])))),
        }
        i += 1;
    }
    let Some(sep) = sep else {
        // no `--`: the corrected command, from what was typed (its shape only when a word cannot
        // be pasted back as typed)
        let typed: Vec<String> = args.iter().map(shown).collect();
        let exact = args.iter().zip(&typed).all(|(a, t)| a.as_bytes() == t.as_bytes());
        let q: Vec<String> = typed.iter().map(|t| shell_quote(t)).collect();
        let (opts, cmd) = (q[1..i].join(" "), q[i..].join(" "));
        let fix = match (exact, cmd.is_empty()) {
            (false, _) => "sheepdog run [options] -- COMMAND".to_string(),
            (true, true) => format!("sheepdog run {opts}{}-- COMMAND", if opts.is_empty() { "" } else { " " }),
            (true, false) => format!("sheepdog run {opts}{}-- {cmd}", if opts.is_empty() { "" } else { " " }),
        };
        let what = if cmd.is_empty() { "no command" } else { "no -- before the command, so nothing ran" };
        return Err(usage_because(format!("{what}. Put -- between the options and the command: {fix}")));
    };
    let cmd = args[sep + 1..].to_vec();
    if cmd.is_empty() {
        return Err(usage_because("no command after --.".to_string()));
    }
    Ok(Args { argv, mode, cmd, grace, leave_strays, quiet, forward_int_to_root, owner, status_fd, no_sweep, inherit, timeout, max_mem, max_procs, kill_deadline })
}

/// Exit code for a wait status: the command's code, or 128+signal.
pub fn code_of(status: c_int) -> i32 {
    if libc::WIFEXITED(status) {
        libc::WEXITSTATUS(status)
    } else if libc::WIFSIGNALED(status) {
        128 + libc::WTERMSIG(status)
    } else {
        125
    }
}

/// Send `sig` only if `pid` is still the process with identity `id` (PLAN.md §3.3; the
/// remaining window is the time between this check and the kill call).
pub fn signal(pid: i32, id: u64, sig: c_int) -> Sent {
    // never a process group or the broadcast: 0 is our own group, -1 every process we may
    // signal (members are real pids; this makes anything else impossible, not just unlikely)
    if pid <= 1 {
        return Sent::No;
    }
    // Test seam (debug builds only): SHEEPDOG_TEST_NOKILL=1 makes every signal fail, as EPERM
    // would after a member's setuid exec (cells 20 and 24-lite).
    if seam("SHEEPDOG_TEST_NOKILL") {
        return Sent::No;
    }
    // Test seam (debug builds only): SHEEPDOG_TEST_UNKILLABLE=<pid>: a SIGKILL to that pid fails
    // (as EPERM would); its other signals are sent (a STOP lands, so the pass must continue it)
    if sig == libc::SIGKILL && seam_ms("SHEEPDOG_TEST_UNKILLABLE") == Some(pid as u64) {
        return Sent::No;
    }
    // Test seam (debug builds only): SHEEPDOG_TEST_REUSE_PID=<pid> sends to that pid instead,
    // with the member's identity, as if the member's pid had been reused (S3).
    let pid = seam_ms("SHEEPDOG_TEST_REUSE_PID").map_or(pid, |p| p as i32);
    send_checked(pid, id, sig)
}

/// How a signal was delivered (PLAN.md §3.3 step 2).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sent {
    /// not sent: the process is gone or is no longer the member
    No,
    /// sent through a pidfd opened before the identity check: it reached the member
    Pinned,
    /// sent with `kill` after the identity check (and `kill` succeeded): a pid reused in between
    /// would have got it
    Unpinned,
}

/// Debug seam SHEEPDOG_TEST_SIGNAL_LOG: one line per signal decision, so a test can tell which
/// path a signal took.
pub fn trace(line: String) {
    if cfg!(debug_assertions) {
        if let Ok(p) = std::env::var("SHEEPDOG_TEST_SIGNAL_LOG") {
            trace_to(std::path::Path::new(&p), &line);
        }
    }
}

/// Debug seam SHEEPDOG_TEST_TRACE: one line per event that is not a signal (a start, a disabled
/// source), kept out of the signal log so its exact-content checks stay about signals.
pub fn note(line: String) {
    if cfg!(debug_assertions) {
        if let Ok(p) = std::env::var("SHEEPDOG_TEST_TRACE") {
            trace_to(std::path::Path::new(&p), &line);
        }
    }
}

/// Append one line to the log at `path`, in one write: the relay and the supervisor both write
/// this log, and a line written in two parts can be merged with the other writer's.
pub(crate) fn trace_to(path: &std::path::Path, line: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new().append(true).create(true).open(path) {
        let _ = f.write_all(format!("{line}\n").as_bytes());
    }
}

/// An unpinned STOP to member (`p`, `id`) was delivered: read who is at `p` now. If it is still
/// the member, the STOP reached it and there is nothing to roll back, ever. If it is another
/// process, the STOP landed on that process (a pid reused between the check and the kill):
/// record its identity for the rollback (PLAN.md §3.3 step 2). Debug seam
/// SHEEPDOG_TEST_FREEZE_PID_REUSED records the wrong-freeze seam's STOP as landing on another
/// process than the one now at the pid.
fn landed(frozen: &mut Vec<(i32, u64)>, p: i32, id: u64) {
    let mut on = sheepdog::ident::identity(p);
    if id == 0 && seam("SHEEPDOG_TEST_FREEZE_PID_REUSED") {
        on = on.map(|u| u.wrapping_add(1));
    }
    if let Some(on) = on.filter(|&on| on != id) {
        trace(format!("record {p}"));
        frozen.push((p, on));
    }
}

/// The rollback's CONT to the process a STOP of ours landed on by mistake (PLAN.md §3.3 step
/// 4), guarded by that process's own identity, the same way as any signal (on Linux through a
/// pidfd, so a pid that changes hands again cannot get it).
fn rollback(pid: i32, landed_on: u64) {
    if seam("SHEEPDOG_TEST_NOKILL") {
        return;
    }
    if same(pid, landed_on) {
        trace(format!("rollback {pid}"));
        // it undoes this supervisor's own STOP: identity-checked, never held back by the wall
        // (PHASE2.md D4); reported after it, if the process lacks the tag (the check may take a
        // moment, and the process must not wait stopped for it)
        let _ = send_checked_as(pid, landed_on, libc::SIGCONT, false);
        wall::rollback_tripwire(pid, landed_on);
    }
}

/// Send `sig` to `pid` only if it is the process with identity `id` (PLAN.md §3.3 step 2).
/// Linux: through a pidfd opened before the check, so a pid reused after the check can never
/// get the signal. If the pidfd cannot be opened or used for any reason but a gone process
/// (ENOSYS on an old kernel, EPERM under a seccomp filter, EMFILE; debug seams
/// SHEEPDOG_TEST_PIDFD_ENOSYS for the open, SHEEPDOG_TEST_PIDFD_SEND_ENOSYS for the send),
/// the check and `kill` remain; the window between them is the stated residual, and the
/// freeze's rollback covers a STOP that lands in it.
/// `send_checked_as` with the test wall (every sender but the rollback).
fn send_checked(pid: i32, id: u64, sig: c_int) -> Sent {
    send_checked_as(pid, id, sig, true)
}

#[cfg(target_os = "linux")]
fn send_checked_as(pid: i32, id: u64, sig: c_int, wall: bool) -> Sent {
    if pid <= 1 {
        return Sent::No; // every door refuses a group or the broadcast (the rollback enters here)
    }
    if inert(pid, sig) {
        return Sent::No;
    }
    let race = wrong_freeze_race(pid, sig);
    let fd = if race || seam("SHEEPDOG_TEST_PIDFD_ENOSYS") {
        None
    } else {
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) } as i32;
        if fd < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            return Sent::No; // gone
        }
        (fd >= 0).then_some(fd)
    };
    // the wrong-freeze seam simulates a pid reused after every check passed for the member
    if wall && !wall_bypass(race, id) && !wall::admit(pid, id, sig) {
        if let Some(fd) = fd {
            unsafe { libc::close(fd) };
        }
        return Sent::No;
    }
    if let Some(fd) = fd {
        trace(format!("pidfd {pid} {sig}"));
        if !same(pid, id) {
            unsafe { libc::close(fd) };
            return Sent::No;
        }
        let (r, err) = if seam("SHEEPDOG_TEST_PIDFD_SEND_ENOSYS") {
            (-1, Some(libc::ENOSYS))
        } else {
            let r = unsafe { libc::syscall(libc::SYS_pidfd_send_signal, fd, sig, std::ptr::null::<libc::siginfo_t>(), 0) };
            (r, std::io::Error::last_os_error().raw_os_error())
        };
        unsafe { libc::close(fd) };
        if r == 0 {
            return Sent::Pinned;
        }
        if err == Some(libc::ESRCH) {
            return Sent::No; // exited after the check
        }
    }
    trace(format!("kill {pid} {sig}"));
    if (race || same(pid, id)) && unsafe { libc::kill(pid, sig) } == 0 {
        return Sent::Unpinned;
    }
    Sent::No
}

/// Test seam (debug builds only): SHEEPDOG_TEST_INERT=1 sends nothing at all and logs each
/// signal it withholds, so a cell can aim `sheepdog kill` at a process it must never signal
/// (pid 1, its own caller) and still see a broken target check as a logged signal (S6). Every
/// door that sends to a member checks it: `send_checked` (under `signal` and the rollback) and
/// the panic path.
fn inert(pid: i32, sig: c_int) -> bool {
    let on = seam("SHEEPDOG_TEST_INERT");
    if on {
        trace(format!("inert {pid} {sig}"));
    }
    on
}

/// Test seam (debug builds only): SHEEPDOG_TEST_WRONG_FREEZE=<pid> makes the identity check of
/// the STOP to that pid pass on the kill path, as a pid reused between the check and the kill
/// would (S3): the STOP lands on a process that is not the member.
fn wrong_freeze_race(pid: i32, sig: c_int) -> bool {
    sig == libc::SIGSTOP && seam_ms("SHEEPDOG_TEST_WRONG_FREEZE") == Some(pid as u64)
}

/// The wrong-freeze seam also skips the test wall (PHASE2.md D4), but only for the kill loop's
/// injected entry (identity 0): another STOP to that pid (job control, a real member's pid) is
/// tag-checked as any signal. The race cell's own STOPs carry a real identity and so keep the
/// identity-check bypass above without the wall bypass.
fn wall_bypass(race: bool, id: u64) -> bool {
    race && id == 0
}

/// Send `sig` to `pid` only if it is the process with identity `id` (PLAN.md §3.3 step 2).
/// macOS: the uniqueid is re-read just before `kill` (the window between them is the stated
/// residual; the freeze's rollback covers a STOP that lands on a reused pid).
#[cfg(target_os = "macos")]
fn send_checked_as(pid: i32, id: u64, sig: c_int, wall: bool) -> Sent {
    if pid <= 1 {
        return Sent::No; // every door refuses a group or the broadcast (the rollback enters here)
    }
    if inert(pid, sig) {
        return Sent::No;
    }
    // the wrong-freeze seam simulates a pid reused after every check passed for the member
    let race = wrong_freeze_race(pid, sig);
    if wall && !wall_bypass(race, id) && !wall::admit(pid, id, sig) {
        return Sent::No;
    }
    trace(format!("kill {pid} {sig}"));
    if (race || same(pid, id)) && unsafe { libc::kill(pid, sig) } == 0 {
        return Sent::Unpinned;
    }
    Sent::No
}

/// Test seam (debug builds only): sleep for the number of ms in env var `name`. If
/// SHEEPDOG_TEST_READY_FILE is set, first create that file, so a test can act INSIDE the window
/// instead of guessing with a sleep (on macOS the first launch of a freshly built binary is
/// delayed by the security scan, and a sleep-based test then raced the scan, not the window).
pub fn seam_sleep(name: &str) {
    if let Some(ms) = std::env::var(name).ok().filter(|_| cfg!(debug_assertions)).and_then(|v| v.parse().ok()) {
        if let Ok(f) = std::env::var("SHEEPDOG_TEST_READY_FILE") {
            let _ = std::fs::File::create(f);
        }
        std::thread::sleep(Duration::from_millis(ms));
    }
}

/// Test seam (debug builds only): if env var `name` names a file, create
/// SHEEPDOG_TEST_READY_FILE (if set), then wait until that file exists (30 s at most), so a test
/// closes the window itself instead of a timer closing it.
pub fn seam_hold(name: &str) {
    if let Some(release) = std::env::var(name).ok().filter(|_| cfg!(debug_assertions)) {
        if let Ok(f) = std::env::var("SHEEPDOG_TEST_READY_FILE") {
            let _ = std::fs::File::create(f);
        }
        let end = Instant::now() + Duration::from_secs(30);
        while !std::path::Path::new(&release).exists() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

/// Test seam (debug builds only): the number of milliseconds in env var `name`, if set.
pub fn seam_ms(name: &str) -> Option<u64> {
    std::env::var(name).ok().filter(|_| cfg!(debug_assertions)).and_then(|v| v.parse().ok())
}

/// Test seam (debug builds only): is env var `name` set to "1"?
pub fn seam_flag(name: &str) -> bool {
    seam(name)
}

fn seam(name: &str) -> bool {
    cfg!(debug_assertions) && std::env::var(name).as_deref() == Ok("1")
}

/// Options of the kill loop. Production: `KillOpts::from_env()` (10 s, no seams). The debug
/// seams exist only in debug builds.
pub struct KillOpts {
    pub deadline: Duration,
    /// §3.3 step 1: TERM (then CONT) every member, and give them this long to exit
    pub grace: Duration,
    /// hide every member from all scans after the first one that reported it (cell 24-lite)
    pub forget: bool,
    /// every emptiness check answers "not empty" (the deadline-bound test)
    pub never_empty: bool,
    /// panic right after the first SIGSTOP pass (the panic-safety test)
    pub panic_after_stop: bool,
    /// debug seam: a pid put into the freeze as if its STOP had landed on a reused pid
    pub wrong_freeze: Option<i32>,
    /// signals blocked from the first freeze on (`sheepdog kill`: one that ended sheepdog
    /// between a freeze and its SIGKILL would leave the tree stopped); the caller restores the
    /// mask. The TERM grace freezes nothing, so they act at once there.
    pub hold: Vec<c_int>,
}

impl KillOpts {
    pub fn with_grace(mut self, grace: Duration) -> Self {
        self.grace = grace;
        self
    }

    /// `--kill-deadline` (the debug seam SHEEPDOG_TEST_DEADLINE_MS still wins in a debug build).
    pub fn with_deadline(mut self, d: Option<Duration>) -> Self {
        if let Some(d) = d {
            if seam_ms("SHEEPDOG_TEST_DEADLINE_MS").is_none() {
                self.deadline = d;
            }
        }
        self
    }

    pub fn from_env() -> Self {
        KillOpts {
            grace: Duration::ZERO,
            deadline: std::env::var("SHEEPDOG_TEST_DEADLINE_MS")
                .ok()
                .filter(|_| cfg!(debug_assertions))
                .and_then(|v| v.parse().ok())
                .map(Duration::from_millis)
                .unwrap_or(Duration::from_secs(10)),
            forget: seam("SHEEPDOG_TEST_FORGET"),
            never_empty: seam("SHEEPDOG_TEST_NEVER_EMPTY"),
            panic_after_stop: seam("SHEEPDOG_TEST_PANIC_AFTER_STOP"),
            wrong_freeze: seam_ms("SHEEPDOG_TEST_WRONG_FREEZE").map(|p| p as i32),
            hold: Vec::new(),
        }
    }
}

/// Membership while the job runs (PLAN.md §3.2): the sticky map of live members, every
/// identity ever seen as a member (macOS: the `puniq` fact looks parents up here, and it must
/// still hold after a parent has exited), and R, the responsible identities (macOS: the
/// supervisor plus members that became responsible for themselves).
#[derive(Default)]
pub struct Tracker {
    pub known: HashMap<i32, u64>,
    pub ever: std::collections::HashSet<u64>,
    pub r: std::collections::HashSet<u64>,
    /// macOS: the supervisor's ancestors (their uniqueids): never members, whatever a fact says
    /// (PLAN §3.2; they can share the supervisor's responsible process in inherit mode)
    pub never: std::collections::HashSet<u64>,
}

impl Tracker {
    /// Add freshly found members to the sticky map (replacing an entry only if the old
    /// process is gone) and drop members confirmed dead.
    pub fn refresh(&mut self, found: Vec<(i32, u64)>) {
        for (p, id) in found {
            self.ever.insert(id);
            match self.known.get(&p) {
                Some(&old) if same(p, old) => {}
                _ => {
                    self.known.insert(p, id);
                }
            }
        }
        self.known.retain(|&p, &mut id| same(p, id));
    }
}

/// Why the kill did not end clean.
#[derive(Debug, PartialEq)]
pub enum KillError {
    /// members still alive at the deadline (possibly none that a scan could list)
    Deadline(Vec<i32>),
    /// the kill loop panicked; the known members were sent SIGKILL and a message was printed
    Internal,
}

/// Exit code for a kill that did not end clean (PLAN.md §3.3 step 6: 125).
pub fn kill_failed(e: KillError) -> i32 {
    match e {
        KillError::Deadline(alive) => {
            status::set_deadline(&alive);
            deadline_missed(&alive)
        }
        KillError::Internal => {
            status::set_error("internal error while killing the tree");
            125
        }
    }
}

/// Set by `run` right before the kill of its own job: that `kill_tree` call (and only it) feeds
/// the caps and the run's status record.
pub static JOB_KILL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The session, parent and command of a member, read at its first signal, for `killed[]`.
fn member_info(pid: i32) -> (Option<i32>, Option<i32>, String) {
    let sid = unsafe { libc::getsid(pid) };
    #[cfg(target_os = "macos")]
    let (ppid, argv) = (macos::parent(pid), macos::cmdline(pid));
    #[cfg(target_os = "linux")]
    let (ppid, argv) = (linux::parent(pid), linux::report_cmd(pid));
    ((sid > 0).then_some(sid), ppid, journal::text(argv.join(" ").as_bytes()))
}

/// How a killed member escaped (PLAN.md §3.1 `killed[].escaped`): `setsid` if it is in another
/// session than the supervisor's; else `reparented` if its parent at its first signal was not a
/// process of the job (the supervisor as subreaper, or pid 1); the root never escaped.
fn escaped_how(pid: i32, sid: Option<i32>, ppid: Option<i32>) -> Option<&'static str> {
    let me = unsafe { libc::getpid() };
    if pid == status::root_pid() {
        return None;
    }
    if sid.is_some_and(|s| s != unsafe { libc::getsid(0) }) {
        return Some("setsid");
    }
    match ppid {
        Some(p) if p == me || p == 1 => Some("reparented"),
        None => Some("reparented"),
        _ => None,
    }
}

/// PLAN.md §3.3: the TERM grace (step 1), then the freeze-and-kill passes (steps 2-6).
///
/// - `members` returns the live members right now as (pid, identity).
/// - `reap` runs every pass (Linux reaps adopted orphans).
/// - `tree_empty` is an authoritative emptiness check where the platform has one: Linux with
///   the subreaper set answers Some(no children left) (a scan of /proc is not atomic and can
///   miss a tree that moves faster than the scan: review round 2, P1-A). Otherwise None, and
///   the scan-based rule applies (a stated residual, PLAN.md §7.1).
/// - `send` delivers a signal (production: `signal`, which re-checks identity first).
///
/// Membership is sticky: once seen, a process stays in `known` until it is confirmed dead.
/// Without an authoritative check, it takes two consecutive passes with nothing known alive,
/// both at the end of the loop and at the deadline (review round 3, F2). The deadline is
/// checked at the top of EVERY pass. If the loop panics, every known member is killed, so
/// none is left stopped (review round 3, F7).
pub fn kill_tree(
    opts: &KillOpts,
    members: impl FnMut() -> Vec<(i32, u64)>,
    reap: impl FnMut(),
    tree_empty: impl FnMut() -> Option<bool>,
    mut send: impl FnMut(i32, u64, c_int) -> Sent,
    initial: HashMap<i32, u64>,
) -> Result<(), KillError> {
    let known: std::cell::RefCell<HashMap<i32, u64>> = std::cell::RefCell::new(initial);
    // each member's session, parent and command, read at its first signal (for killed[])
    let captured: std::cell::RefCell<HashMap<(i32, u64), (Option<i32>, Option<i32>, String)>> = Default::default();
    let send = |p: i32, id: u64, sig: c_int| {
        captured.borrow_mut().entry((p, id)).or_insert_with(|| member_info(p));
        send(p, id, sig)
    };
    // the run's own kill (not the auto-sweep's, not `kill`'s or `sweep`'s): the caps are
    // evaluated on every scan while the job is being ended (a later one is a note), and what it
    // kills goes into the run's `killed[]`
    let own = JOB_KILL.swap(false, std::sync::atomic::Ordering::SeqCst);
    let mut members = members;
    let members = move || {
        let f = members();
        if own {
            let _ = caps::check(&f);
        }
        f
    };
    let result = kill_tree_inner(opts, members, reap, tree_empty, send, &known);
    for (&(p, id), (sid, ppid, cmd)) in captured.borrow().iter() {
        if own && id != 0 && !same(p, id) {
            status::add_killed(status::Killed { pid: p, cmd: cmd.clone(), escaped: escaped_how(p, *sid, *ppid) });
        }
    }
    result
}

fn kill_tree_inner(
    opts: &KillOpts,
    members: impl FnMut() -> Vec<(i32, u64)>,
    reap: impl FnMut(),
    tree_empty: impl FnMut() -> Option<bool>,
    send: impl FnMut(i32, u64, c_int) -> Sent,
    known: &std::cell::RefCell<HashMap<i32, u64>>,
) -> Result<(), KillError> {
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        kill_loop(opts, members, reap, tree_empty, send, known)
    }));
    match r {
        Ok(result) => result.map_err(KillError::Deadline),
        Err(_) => {
            let known = known.borrow();
            for (&p, &id) in known.iter() {
                let _ = send_checked(p, id, libc::SIGKILL); // the one door: pid <= 1, inert, wall, identity
            }
            fail!("sheepdog: internal error while killing the tree; sent SIGKILL to the {} member(s) it knew. The tree may NOT be clean.", known.len());
            Err(KillError::Internal)
        }
    }
}

fn kill_loop(
    opts: &KillOpts,
    mut members: impl FnMut() -> Vec<(i32, u64)>,
    mut reap: impl FnMut(),
    mut tree_empty: impl FnMut() -> Option<bool>,
    mut send: impl FnMut(i32, u64, c_int) -> Sent,
    known: &std::cell::RefCell<HashMap<i32, u64>>,
) -> Result<(), Vec<i32>> {
    let deadline = Instant::now() + opts.deadline;
    let mut seen: std::collections::HashSet<(i32, u64)> = std::collections::HashSet::new();
    let forget = opts.forget;
    let mut scan = move || -> Vec<(i32, u64)> {
        let found = members();
        if !forget {
            return found;
        }
        let fresh: Vec<(i32, u64)> = found.into_iter().filter(|m| !seen.contains(m)).collect();
        seen.extend(fresh.iter().copied());
        fresh
    };
    let never_empty = opts.never_empty;
    let mut empty_check = move || if never_empty { Some(false) } else { tree_empty() };
    let refresh = |known: &mut HashMap<i32, u64>, found: Vec<(i32, u64)>| {
        for (p, id) in found {
            match known.get(&p) {
                Some(&old) if same(p, old) => {}
                _ => {
                    known.insert(p, id);
                }
            }
        }
        known.retain(|&p, &mut id| same(p, id));
    };
    // §3.3 step 1, the grace: TERM then CONT every member (a stopped member acts on TERM only
    // once it runs), newly found ones too, until all are gone or the grace is over
    if !opts.grace.is_zero() {
        let grace_end = Instant::now() + opts.grace;
        let mut termed: std::collections::HashSet<(i32, u64)> = std::collections::HashSet::new();
        loop {
            reap();
            refresh(&mut known.borrow_mut(), scan());
            let now: Vec<(i32, u64)> = known.borrow().iter().map(|(&p, &id)| (p, id)).collect();
            for &(p, id) in &now {
                if termed.insert((p, id)) {
                    let _ = send(p, id, libc::SIGTERM);
                    let _ = send(p, id, libc::SIGCONT);
                }
            }
            if now.is_empty() && empty_check() != Some(false) {
                break;
            }
            if Instant::now() > grace_end {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    let deadline = deadline + opts.grace;
    if !opts.hold.is_empty() {
        unsafe {
            let mut set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            for &sg in &opts.hold {
                libc::sigaddset(&mut set, sg);
            }
            libc::sigprocmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
        }
    }
    let mut empty = 0;
    let mut wrong_freeze = opts.wrong_freeze;
    loop {
        reap();
        refresh(&mut known.borrow_mut(), scan());
        if Instant::now() > deadline {
            // let the last SIGKILLs take effect, then require the same evidence as the normal
            // end: authoritative empty, or two consecutive passes with nothing known alive
            std::thread::sleep(Duration::from_millis(100));
            let mut empties = 0;
            for _ in 0..2 {
                reap();
                refresh(&mut known.borrow_mut(), scan());
                match empty_check() {
                    Some(true) => return Ok(()),
                    Some(false) => {}
                    None if known.borrow().is_empty() => empties += 1,
                    None => {}
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            if empties == 2 {
                return Ok(());
            }
            return Err(known.borrow().keys().copied().collect());
        }
        let authoritative = empty_check();
        if authoritative == Some(true) {
            return Ok(());
        }
        if known.borrow().is_empty() {
            // an authoritative "not empty" never counts as empty
            empty = if authoritative.is_none() { empty + 1 } else { 0 };
            if empty >= 2 {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(1));
            continue;
        }
        empty = 0;
        // freeze what we know, then close over members created meanwhile, then kill all. After a
        // STOP delivered unpinned (a pid reused after its check would have got it), `landed`
        // records the process it landed on if that is not the member (§3.3 step 2), for the
        // rollback; a pinned STOP reached the member and has nothing to roll back.
        let mut before: Vec<(i32, u64)> = known.borrow().iter().map(|(&p, &id)| (p, id)).collect();
        if let Some(d) = wrong_freeze.take() {
            before.push((d, 0)); // debug seam: its STOP lands as on a reused pid (see signal())
        }
        let mut frozen: Vec<(i32, u64)> = Vec::new();
        for &(p, id) in &before {
            if send(p, id, libc::SIGSTOP) == Sent::Unpinned {
                landed(&mut frozen, p, id);
            }
        }
        if opts.panic_after_stop {
            panic!("test seam: panic after the freeze");
        }
        seam_hold("SHEEPDOG_TEST_HOLD_AFTER_FREEZE");
        refresh(&mut known.borrow_mut(), scan());
        let all: Vec<(i32, u64)> = known.borrow().iter().map(|(&p, &id)| (p, id)).collect();
        for &(p, id) in &all {
            // by pid AND identity: a new member at a pid the first pass stopped for another process
            // gets its own STOP
            if !before.iter().any(|&(b, bid)| b == p && bid == id) {
                if send(p, id, libc::SIGSTOP) == Sent::Unpinned {
                    landed(&mut frozen, p, id);
                }
            }
        }
        // §3.3 step 4, verify: the process our STOP landed on, when it was not the member, gets
        // SIGCONT if it is still that very process (a later process at the pid is left as it
        // is). Whether it was stopped before our STOP cannot be known (its pid changed hands
        // after our check), so it is always resumed: leaving it stopped for good is the worse
        // error. It need not show as stopped yet: SIGCONT also discards a STOP still pending.
        // A recorded process that is a member now (another member's pid came to it) stays
        // frozen until its KILL: a resumed member could fork.
        for &(p, landed_on) in &frozen {
            if !all.iter().any(|&(q, qid)| q == p && qid == landed_on) {
                rollback(p, landed_on);
            }
        }
        for &(p, id) in &all {
            let _ = send(p, id, libc::SIGKILL);
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Signal set-up of a run (PLAN.md §3.1). sheepdog never catches TERM or SIGCHLD: it BLOCKS
/// them and waits for them synchronously (Linux: sigtimedwait; macOS: kqueue plus a sigpending
/// check). A blocked signal stays pending until the wait takes it, across the macOS self
/// re-exec and across any gap between a check and a blocking call, so no TERM can be lost
/// (review round 6: a caught TERM was lost across the re-exec, and between the flag check and
/// the blocking wait).
pub struct Signals {
    /// the mask sheepdog was started with: the root gets exactly this
    pub caller_mask: libc::sigset_t,
    /// false when the caller ignored TERM: then TERM stays ignored, for sheepdog and the root
    pub watch_term: bool,
    /// INT, HUP and QUIT, each watched only if the caller left it at its default (S4; QUIT since
    /// the phase-1 review: ctrl-\ ended sheepdog before its kill)
    pub watch_int: bool,
    pub watch_hup: bool,
    pub watch_quit: bool,
    /// the stop signals TSTP, TTIN, TTOU and CONT, each watched only at its default (S5)
    pub watch_stop: [bool; 3],
    pub watch_cont: bool,
}

/// The stop signals job control handles, in the order `watch_stop` lists them.
pub const STOPS: [c_int; 3] = [libc::SIGTSTP, libc::SIGTTIN, libc::SIGTTOU];

/// Left unblocked. The faults (SEGV, BUS, ILL, FPE, TRAP, SYS): a real fault in sheepdog must
/// still end it (a fault while its signal is blocked is undefined on some systems). ABRT (which
/// `abort()` delivers even when blocked): it stays the way a watchdog ends a hung sheepdog
/// (systemd's WatchdogSignal is SIGABRT). Sent with `kill`, these (with KILL) are the signals
/// that still end sheepdog without its kill: PHASE1.md §4 names that residual.
pub const FAULTS: [c_int; 7] = [libc::SIGSEGV, libc::SIGBUS, libc::SIGILL, libc::SIGFPE, libc::SIGTRAP, libc::SIGSYS, libc::SIGABRT];

/// Add to `set` every signal except the faults (and KILL and STOP, which cannot be blocked):
/// sheepdog blocks all of them for itself (`run` and `kill`), so that no signal whose default
/// action ends a process can end it before or during its kill. A rule, not a list: a hand-copied
/// list missed EMT, PWR, IO, STKFLT and the real-time signals (phase-1 review, round 2). Those it
/// waits on are consumed as before; the rest stay pending, never acted on. The root gets the
/// caller's mask (SETSIGMASK).
pub fn block_all_but_faults(set: &mut libc::sigset_t) {
    unsafe {
        libc::sigfillset(set);
        for s in FAULTS {
            libc::sigdelset(set, s);
        }
    }
}

impl Signals {
    /// The signal set the event loop waits on: CHLD plus every watched signal.
    pub fn wait_set(&self) -> libc::sigset_t {
        unsafe {
            let mut set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            libc::sigaddset(&mut set, libc::SIGCHLD);
            let watched = [
                (self.watch_term, libc::SIGTERM),
                (self.watch_int, libc::SIGINT),
                (self.watch_hup, libc::SIGHUP),
                (self.watch_quit, libc::SIGQUIT),
                (self.watch_stop[0], STOPS[0]),
                (self.watch_stop[1], STOPS[1]),
                (self.watch_stop[2], STOPS[2]),
                (self.watch_cont, libc::SIGCONT),
            ];
            for (on, s) in watched {
                if on {
                    libc::sigaddset(&mut set, s);
                }
            }
            set
        }
    }
}

fn setup_signals() -> Signals {
    unsafe {
        // sheepdog must see its children's exits (review round 2, P1-B); the root inherits it
        libc::signal(libc::SIGCHLD, libc::SIG_DFL);
        let at_default = |s: c_int| {
            let mut a: libc::sigaction = std::mem::zeroed();
            libc::sigaction(s, std::ptr::null(), &mut a);
            a.sa_sigaction == libc::SIG_DFL
        };
        let sig = Signals {
            caller_mask: std::mem::zeroed(),
            watch_term: at_default(libc::SIGTERM),
            watch_int: at_default(libc::SIGINT),
            watch_hup: at_default(libc::SIGHUP),
            watch_quit: at_default(libc::SIGQUIT),
            watch_stop: STOPS.map(|s| at_default(s)),
            watch_cont: at_default(libc::SIGCONT),
        };
        // every signal but the faults is blocked for sheepdog (those it waits on are in the wait
        // set): sheepdog must not die without its kill (a write to a closed stderr is EPIPE, not
        // SIGPIPE). The root gets the caller's mask (SETSIGMASK).
        let mut block: libc::sigset_t = std::mem::zeroed();
        block_all_but_faults(&mut block);
        let mut caller_mask: libc::sigset_t = std::mem::zeroed();
        libc::sigprocmask(libc::SIG_BLOCK, &block, &mut caller_mask);
        Signals { caller_mask, ..sig }
    }
}

/// Consume `sig` if it is pending (PHASE1.md §1.1): discard it with the SIG_IGN/SIG_DFL
/// toggle (POSIX discards a pending signal whose action becomes SIG_IGN, even while it is
/// blocked) and return true. Never `sigwait`: on macOS a signal that `sigpending` showed can
/// vanish before a `sigwait` (a CONT removes a pending TSTP, and back), and `sigwait` then
/// blocks forever (phase-1 plan review, rounds 2 and 3, probed). Never for SIGCHLD: SIG_IGN
/// on SIGCHLD turns on automatic reaping.
pub fn consume(sig: c_int) -> bool {
    assert_ne!(sig, libc::SIGCHLD, "SIGCHLD must never be toggled");
    unsafe {
        let mut p: libc::sigset_t = std::mem::zeroed();
        libc::sigpending(&mut p);
        if libc::sigismember(&p, sig) != 1 {
            return false;
        }
        // debug seam: hold between seeing a TSTP pending and clearing it (a CONT sent here
        // discards that TSTP; the S1 cell for "never sigwait")
        if sig == libc::SIGTSTP {
            seam_sleep("SHEEPDOG_TEST_SLEEP_IN_CONSUME_MS");
        }
        libc::signal(sig, libc::SIG_IGN);
        libc::signal(sig, libc::SIG_DFL);
        true
    }
}

/// Does this process already have children (for example a shell's background job before it
/// `exec`ed sheepdog)? waitid with WNOWAIT answers atomically without /proc and without reaping
/// anything (review round 4, P3-3).
pub fn has_children() -> bool {
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    let r = unsafe { libc::waitid(libc::P_ALL, 0, &mut info, libc::WEXITED | libc::WNOHANG | libc::WNOWAIT) };
    r == 0
}

/// Die the way the supervisor died: death by signal N stays death by signal N for our caller,
/// which shells rely on (for example to stop a loop on ctrl-C: review round 4, P2-1).
pub fn die_like(status: libc::c_int) -> i32 {
    if libc::WIFSIGNALED(status) {
        status::write(code_of(status)); // this process ends in the raise below
        let sig = libc::WTERMSIG(status);
        unsafe {
            let no_core = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
            libc::setrlimit(libc::RLIMIT_CORE, &no_core);
            libc::signal(sig, libc::SIG_DFL);
            let mut one: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut one);
            libc::sigaddset(&mut one, sig);
            libc::sigprocmask(libc::SIG_UNBLOCK, &one, std::ptr::null_mut());
            libc::raise(sig); // raw signal site: this process (PHASE2.md §0.3)
        }
    }
    code_of(status)
}

/// The scan tick of the event loop: 250 ms. Debug seam SHEEPDOG_TEST_TICK_MS (under 1 s) widens
/// it, so a cell can tell "acted at the event" from "acted at the next tick" by a wide margin.
pub fn tick_ms() -> u64 {
    seam_ms("SHEEPDOG_TEST_TICK_MS").filter(|&ms| ms < 1000).unwrap_or(250)
}

/// A wait status for a plain exit with `code` (the encoding both OSes use).
pub fn exit_status(code: i32) -> c_int {
    (code & 0xff) << 8
}

/// INT and HUP while the job runs (PLAN.md §3.1, PHASE1.md §1.3 and S4). They never end the
/// job. Each one is forwarded only to members outside sheepdog's own process group: a signal
/// from the terminal, or one sent to the group, already reached every member inside it, so
/// this gives exactly one delivery in every case. Two exceptions: HUP goes to every member when
/// sheepdog (or its relay) is the session leader, since the kernel then sends the terminal's
/// HUP to the leader only; and `--forward-int-to-root` also sends INT to the root.
pub struct Interrupts {
    got_int: bool,
    got_hup: bool,
    got_quit: bool,
    leader: bool,
    /// sheepdog's process group is its own: sheepdog or its relay leads it (a shell job, a
    /// terminal). Otherwise it is the caller's group (a harness that did not make a new one).
    own_group: bool,
    int_to_root: bool,
    quiet: bool,
    /// the D9 hint: when it is due, and for which signal
    hint: Option<(Instant, c_int)>,
    hinted: bool,
}

impl Interrupts {
    /// `relay` is the relay's pid when this supervisor has one (it keeps the pid from the fork;
    /// the parent pid changes once the relay dies). They share a session, so the test is exact.
    pub fn new(a: &Args, relay: Option<i32>) -> Self {
        let sid = unsafe { libc::getsid(0) };
        let leader = sid == unsafe { libc::getpid() } || relay.is_some_and(|r| sid == r);
        let pg = unsafe { libc::getpgrp() };
        let own_group = pg == unsafe { libc::getpid() } || relay == Some(pg);
        Interrupts { got_int: false, got_hup: false, got_quit: false, leader, own_group, int_to_root: a.forward_int_to_root, quiet: a.quiet, hint: None, hinted: false }
    }

    /// No forwarding and no hint (the root-disclaim mode).
    pub fn none() -> Self {
        Interrupts { got_int: false, got_hup: false, got_quit: false, leader: false, own_group: false, int_to_root: false, quiet: true, hint: None, hinted: true }
    }

    /// Record a consumed INT, HUP or QUIT (it decides death by signal at the end).
    pub fn note(&mut self, sig: c_int) {
        self.got_int |= sig == libc::SIGINT;
        self.got_hup |= sig == libc::SIGHUP;
        self.got_quit |= sig == libc::SIGQUIT;
    }

    /// One INT or HUP consumed while the job runs: rescan, forward, and repeat the scan until
    /// it finds no new target (fresh membership at the event, PHASE1.md §1.3; never the last
    /// timed scan alone). `members` rescans and returns the live members.
    pub fn forward(&mut self, sig: c_int, root: i32, members: &mut dyn FnMut() -> Vec<(i32, u64)>) {
        self.note(sig);
        let own = unsafe { libc::getpgrp() };
        let all = sig == libc::SIGHUP && self.leader;
        let with_root = sig == libc::SIGINT && self.int_to_root;
        let mut sent: std::collections::HashSet<(i32, u64)> = std::collections::HashSet::new();
        let mut root_got = false;
        for _ in 0..32 {
            let fresh: Vec<(i32, u64)> = members()
                .into_iter()
                .filter(|m| !sent.contains(m))
                .filter(|&(p, _)| {
                    let g = unsafe { libc::getpgid(p) };
                    all || (with_root && p == root) || (g >= 0 && g != own)
                })
                .collect();
            if fresh.is_empty() {
                break;
            }
            for (p, id) in fresh {
                if signal(p, id, sig) != Sent::No && p == root {
                    root_got = true;
                }
                sent.insert((p, id));
            }
        }
        // In its own group and in the terminal's foreground, the INT most likely came from the
        // terminal and reached the root too: no hint. In the caller's group (a harness in a
        // terminal) the foreground says nothing about who sent it (S4 review round 2).
        // (no hint for QUIT: the hint's advice is about INT and HUP)
        if sig != libc::SIGQUIT && !root_got && !self.hinted && self.hint.is_none() && !(self.own_group && in_foreground()) {
            let ms = seam_ms("SHEEPDOG_TEST_HINT_MS").unwrap_or(3000);
            self.hint = Some((Instant::now() + Duration::from_millis(ms), sig));
        }
    }

    /// The D9 hint: once per job, when it is due. Only called while the job runs: the root was
    /// running at this pass's check (it can exit in the instant after; stated, harmless).
    /// `group_is_ours(pg)` answers, at that moment, whether every process in group `pg` is
    /// sheepdog, its relay or a member: only then is the group named.
    pub fn tick(&mut self, group_is_ours: &mut dyn FnMut(i32) -> bool) {
        if let Some((due, sig)) = self.hint {
            if Instant::now() >= due {
                self.hint = None;
                self.hinted = true;
                if !self.quiet {
                    let pg = unsafe { libc::getpgrp() };
                    let named = (self.own_group && pg > 1 && group_is_ours(pg)).then_some(pg);
                    say!("{}", hint_text(sig, named));
                }
            }
        }
    }

    /// Consume an INT or HUP that is still pending at the end (one that came during the kill):
    /// it counts too ("consumed at any point before it exits").
    pub fn drain_pending(&mut self, sig: &Signals) {
        if sig.watch_int && consume(libc::SIGINT) {
            self.got_int = true;
        }
        if sig.watch_hup && consume(libc::SIGHUP) {
            self.got_hup = true;
        }
        if sig.watch_quit && consume(libc::SIGQUIT) {
            self.got_quit = true;
        }
    }
}

/// Is every process in `group` (the pids of one process group) sheepdog, its relay, or a known
/// member? A caller's process in the group (a background job started before sheepdog with no
/// job control, an orphan the caller left behind) means `kill -INT -<pgid>` would reach it, so
/// the hint must not name the group (S4 review round 3). An empty list (the scan failed) is no.
pub fn only_ours(group: &[i32], relay: Option<i32>, known: &HashMap<i32, u64>) -> bool {
    let me = unsafe { libc::getpid() };
    !group.is_empty()
        && group.iter().all(|&p| p == me || Some(p) == relay || known.get(&p).is_some_and(|&id| same(p, id)))
}

/// The D9 hint for `sig`, naming the process group `pg`.
/// `own` is sheepdog's own process group, or None when the group is the caller's (signalling it
/// would reach the caller too). A group of 1 or 0 is never named either: `kill -INT -1` is the
/// broadcast (every process the reader may signal) and `-0` the reader's own group; that
/// happens as PID 1 in a container.
fn hint_text(sig: c_int, own: Option<i32>) -> String {
    let (name, flag) = if sig == libc::SIGINT { ("SIGINT", "INT") } else { ("SIGHUP", "HUP") };
    let head = format!("sheepdog: got {name}; the command is still running. A signal sent only to sheepdog's pid does not reach it: send TERM to end the job");
    if let Some(pg) = own.filter(|&pg| pg > 1) {
        format!("{head}, or signal the process group (kill -{flag} -{pg}).")
    } else {
        format!("{head}.")
    }
}

/// Is sheepdog's process group the foreground group of its controlling terminal? Then an INT
/// or HUP came from that terminal (or from someone signalling the group), and it reached the
/// root too, so the pid-only hint would be false (a REPL or an editor that handles ctrl-C).
fn in_foreground() -> bool {
    unsafe {
        let fd = libc::open(b"/dev/tty\0".as_ptr() as *const libc::c_char, libc::O_RDONLY | libc::O_NOCTTY | libc::O_CLOEXEC | libc::O_NONBLOCK);
        if fd < 0 {
            return false;
        }
        let fg = libc::tcgetpgrp(fd);
        libc::close(fd);
        fg >= 0 && fg == libc::getpgrp()
    }
}

/// Job control (PHASE1.md §1.4): a ctrl-Z (TSTP), or a background job touching the terminal
/// (TTIN, TTOU), stops the whole job, escapees included, and stops sheepdog itself so the shell
/// sees the job stopped. The members sheepdog stopped are continued when it is continued. A
/// member that was already stopped when sheepdog looked is not recorded: one stopped by the
/// user stays stopped, and a group member that stopped by itself (the terminal's TSTP) gets its
/// CONT with the group's, as without sheepdog.
#[derive(Default)]
pub struct JobControl {
    /// members sheepdog stopped and has not continued yet
    stopped_by_us: Vec<(i32, u64)>,
    /// the relay, when this supervisor has one
    relay: Option<i32>,
}

impl JobControl {
    pub fn new(relay: Option<i32>) -> Self {
        JobControl { stopped_by_us: Vec::new(), relay }
    }

    /// sheepdog was continued (its self-stop returned, or its loop took a CONT): continue the
    /// relay too. The relay mirrors the supervisor's stop, and a supervisor continued on its own
    /// (a debugger, a signal to its pid) must not leave the relay stopped: the caller waits on
    /// the relay. Only while the relay is still our parent (a parent's pid cannot be reused
    /// while we live).
    /// Consume a pending CONT; if there was one, sheepdog was continued, so continue the relay
    /// too. Every CONT the supervisor takes goes through here (S5 review round 4).
    fn take_cont(&self, sigs: &Signals) -> bool {
        let c = sigs.watch_cont && consume(libc::SIGCONT);
        if c {
            self.continue_relay();
        }
        c
    }

    /// Level-triggered, every loop pass: a relay that is stopped while this supervisor runs is
    /// always wrong (it mirrors a stop that is over), so continue it. This closes every window in
    /// which the relay mirrors a stop the supervisor has already left (S5 review round 5).
    pub fn keep_relay_running(&self, stopped: fn(i32) -> bool) {
        if let Some(r) = self.relay.filter(|&r| r > 1) {
            if unsafe { libc::getppid() } == r && stopped(r) {
                trace("relay-continued".into());
                unsafe { libc::kill(r, libc::SIGCONT) }; // raw signal site: this supervisor's parent, the relay (getppid checked) (PHASE2.md §0.3)
            }
        }
    }

    pub fn continue_relay(&self) {
        if let Some(r) = self.relay.filter(|&r| r > 1) {
            if unsafe { libc::getppid() } == r {
                unsafe { libc::kill(r, libc::SIGCONT) }; // raw signal site: this supervisor's parent, the relay (getppid checked) (PHASE2.md §0.3)
            }
        }
    }

    /// One stop (all stop signals of one wake are one stop): `sig` is the one sheepdog raises on
    /// itself, so the shell reports the right reason ("Stopped (tty input)" for TTIN).
    /// `members` rescans and returns the live members; `stopped(pid)` reads a process's state.
    pub fn stop(&mut self, sig: c_int, sigs: &Signals, root: i32, members: &mut dyn FnMut() -> Vec<(i32, u64)>, stopped: fn(i32) -> bool) {
        let own = unsafe { libc::getpgrp() };
        let mut seen: std::collections::HashSet<(i32, u64)> = std::collections::HashSet::new();
        let mut group: Vec<(i32, u64)> = Vec::new();
        // The job is over (TERM comes first in the fixed order; the root has exited): do not
        // stop. Neither consumes anything: the event loop's next pass takes the TERM or the exit.
        let over = || (sigs.watch_term && term_pending()) || root_ended(root);
        // Escapees (outside sheepdog's group) get SIGSTOP at once, after a fresh rescan, repeated
        // until no new member appears (a breeding escapee). Group members got the stop from
        // the terminal; they stop by themselves below. Already stopped: not ours to continue.
        for _ in 0..32 {
            let fresh: Vec<(i32, u64)> = members().into_iter().filter(|m| !seen.contains(m)).collect();
            if fresh.is_empty() {
                break;
            }
            for (p, id) in fresh {
                seen.insert((p, id));
                if stopped(p) {
                    continue;
                }
                if unsafe { libc::getpgid(p) } == own {
                    group.push((p, id));
                } else if signal(p, id, libc::SIGSTOP) != Sent::No {
                    self.stopped_by_us.push((p, id));
                }
            }
        }
        // Group members stop by themselves (a pager restores the terminal in its TSTP handler
        // first), bounded to 1 s; then those still running get SIGSTOP, a TSTP-ignoring member
        // included (sheepdog never leaves a member running while its own enforcement stops).
        let until = Instant::now() + Duration::from_millis(seam_ms("SHEEPDOG_TEST_STOP_WAIT_MS").unwrap_or(1000));
        trace("stop-wait".into());
        // A CONT, a TERM or the root's exit ends the wait early: the job was continued, or is
        // over, so no group member is stopped after all (peeked, not consumed: taken below and
        // by the loop).
        let cont = || sigs.watch_cont && pending(libc::SIGCONT);
        while Instant::now() < until && !over() && !cont() && group.iter().any(|&(p, id)| same(p, id) && !stopped(p)) {
            std::thread::sleep(Duration::from_millis(5));
        }
        // One decision, on a consumed fact: "continued" skips both the group SIGSTOPs and the
        // self-stop (a peek here and a consume later could disagree when a stop signal lands in
        // between, and leave a member running while sheepdog is stopped).
        let continued = self.take_cont(sigs);
        if continued {
            trace("decision continued".into());
        }
        let abort = over() || continued;
        seam_sleep("SHEEPDOG_TEST_SLEEP_AFTER_CONT_DECISION_MS");
        for &(p, id) in &group {
            if !abort && same(p, id) && !stopped(p) {
                let _ = signal(p, id, libc::SIGSTOP);
            }
            self.stopped_by_us.push((p, id));
        }
        // Members born during the wait (a group member still running its handler can fork):
        // rescan until stable again, and stop every new one, in the group or not.
        for _ in 0..(if abort { 0 } else { 32 }) {
            let fresh: Vec<(i32, u64)> = members().into_iter().filter(|m| !seen.contains(m)).collect();
            if fresh.is_empty() {
                break;
            }
            for (p, id) in fresh {
                seen.insert((p, id));
                if !stopped(p) && signal(p, id, libc::SIGSTOP) != Sent::No {
                    self.stopped_by_us.push((p, id));
                }
            }
        }
        // Every stop signal pending now belongs to this stop (a handler that re-sends TSTP to
        // its group, a second ctrl-Z during the wait): consume them, so they do not stop the
        // job again after the resume. Not when this stop was called off: a stop signal after
        // the continue is a new stop, for the loop's next pass.
        for (i, &s) in STOPS.iter().enumerate() {
            if sigs.watch_stop[i] && !abort {
                consume(s);
            }
        }
        // Not stopping after all: the job is over, or a CONT already came (someone continued the
        // job during the wait; raising the stop now would remove that CONT and leave sheepdog
        // stopped for good). A CONT in the instant between this check and the raise is lost
        // that way (stated).
        let late = self.take_cont(sigs);
        if !abort && !late && !over() {
            self_stop(sig);
            self.continue_relay();
        }
        // Resumed by a CONT, or the stop was discarded (an orphaned group: the kernel decides).
        // Either way, continue the members sheepdog stopped. A CONT may be pending or not (a
        // TSTP right after the resume removes it): it is consumed if there, never waited for.
        self.take_cont(sigs);
        for (p, id) in std::mem::take(&mut self.stopped_by_us) {
            let _ = signal(p, id, libc::SIGCONT);
        }
    }
}

/// The supervisor is about to exit: a relay left stopped (it mirrored a stop the job has since
/// left, while the supervisor was in its kill, where no loop pass continues it) is continued, so
/// that it sees the exit and ends as the supervisor did (phase-1 review).
pub fn release_relay(relay: Option<i32>, stopped: fn(i32) -> bool) {
    if let Some(r) = relay.filter(|&r| r > 1) {
        if unsafe { libc::getppid() } == r && stopped(r) {
            trace("relay-released".into());
            unsafe { libc::kill(r, libc::SIGCONT) }; // raw signal site: this supervisor's parent, the relay (getppid checked) (PHASE2.md §0.3)
        }
    }
}

/// Is `sig` pending (blocked, not yet consumed)? Only looks.
pub fn pending(sig: c_int) -> bool {
    unsafe {
        let mut p: libc::sigset_t = std::mem::zeroed();
        libc::sigpending(&mut p);
        libc::sigismember(&p, sig) == 1
    }
}

/// Has the root exited (a zombie not yet reaped)? Reads without reaping (WNOWAIT), so the event
/// loop still reaps it and takes its status.
fn root_ended(root: i32) -> bool {
    if root <= 1 {
        return false;
    }
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    let r = unsafe { libc::waitid(libc::P_PID, root as libc::id_t, &mut info, libc::WEXITED | libc::WNOHANG | libc::WNOWAIT) };
    #[cfg(target_os = "linux")]
    let pid = unsafe { info.si_pid() };
    #[cfg(target_os = "macos")]
    let pid = info.si_pid;
    // macOS also reports a STOPPED child here (measured: a root sheepdog had just SIGSTOPped
    // counted as ended), so the state must be an exit
    let ended = [libc::CLD_EXITED, libc::CLD_KILLED, libc::CLD_DUMPED].contains(&info.si_code);
    r == 0 && pid == root && ended
}

/// Stop sheepdog itself with `sig` at its default action: unblock only that signal and raise
/// it; the kernel stops the process, or discards the stop when the process group is orphaned.
/// Re-blocked once it returns.
pub fn self_stop(sig: c_int) {
    unsafe {
        libc::signal(sig, libc::SIG_DFL);
        let mut one: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut one);
        libc::sigaddset(&mut one, sig);
        libc::raise(sig); // raw signal site: this process (PHASE2.md §0.3)
        // sheepdog is stopped from the unblock until a CONT: that span does not run the job, so it
        // does not count against --timeout; a stop the kernel discards (an orphaned group) takes
        // no time here, so then all time counts
        let t0 = Instant::now();
        libc::sigprocmask(libc::SIG_UNBLOCK, &one, std::ptr::null_mut());
        caps::exclude(t0.elapsed());
        libc::sigprocmask(libc::SIG_BLOCK, &one, std::ptr::null_mut());
    }
}

/// How the supervisor ends once the job is over (PHASE1.md §1.3): a TERM means death by TERM;
/// a kill that did not end clean means 125; a root that died of INT or HUP, when sheepdog also
/// got that signal, means death by that signal (so a shell loop stops on ctrl-C); otherwise the
/// root's exit code. `status` is the root's wait status, None when TERM ended the job.
pub fn finish(status: Option<c_int>, result: Result<(), KillError>, ints: &mut Interrupts, sig: &Signals) -> i32 {
    ints.drain_pending(sig);
    match status {
        Some(st) => status::set_root(if libc::WIFSIGNALED(st) { "signaled" } else { "exited" }),
        None if !caps::triggered() => status::set_trigger("term"), // ended by a TERM from outside
        None => {}
    }
    status::report(status, result.is_ok());
    // a cap or the timeout ended the job: 124 (below 125, above the command's own code)
    if status.is_none() && caps::triggered() {
        return match result {
            Err(e) => kill_failed(e),
            Ok(()) => 124,
        };
    }
    match (status, result) {
        (_, Err(e)) => kill_failed(e),
        (None, Ok(())) => die_by_term(143),
        (Some(st), Ok(())) => {
            let same_signal = libc::WIFSIGNALED(st)
                && ((libc::WTERMSIG(st) == libc::SIGINT && ints.got_int)
                    || (libc::WTERMSIG(st) == libc::SIGHUP && ints.got_hup)
                    || (libc::WTERMSIG(st) == libc::SIGQUIT && ints.got_quit));
            if same_signal {
                die_like(st)
            } else {
                code_of(st)
            }
        }
    }
}

/// Before the root is spawned: a TERM that is already pending (the caller blocked TERM and it
/// arrived) ends sheepdog now, so the root never runs (round-7 P3-F2). Returns the exit code
/// if sheepdog must end.
pub fn term_before_spawn(sig: &Signals) -> Option<i32> {
    seam_sleep("SHEEPDOG_TEST_SLEEP_BEFORE_SPAWN_MS");
    (sig.watch_term && term_pending()).then(|| die_by_term(143))
}

/// Is a TERM pending (blocked, not yet taken by a wait)?
pub fn term_pending() -> bool {
    unsafe {
        let mut p: libc::sigset_t = std::mem::zeroed();
        libc::sigpending(&mut p);
        libc::sigismember(&p, libc::SIGTERM) == 1
    }
}

/// After the tree was killed for a TERM: die of SIGTERM, so the caller sees death by signal.
pub fn die_by_term(fallback: i32) -> i32 {
    status::write(fallback); // this process ends in the raise below
    unsafe {
        libc::signal(libc::SIGTERM, libc::SIG_DFL);
        let mut one: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut one);
        libc::sigaddset(&mut one, libc::SIGTERM);
        libc::sigprocmask(libc::SIG_UNBLOCK, &one, std::ptr::null_mut());
        libc::raise(libc::SIGTERM); // raw signal site: this process (PHASE2.md §0.3)
    }
    fallback
}

/// Report a missed deadline (PLAN.md §3.3 step 6) and return exit code 125. The debug signal log
/// gets `deadline <pid>...` (the cells read that, never the message).
pub fn deadline_missed(alive: &[i32]) -> i32 {
    trace(std::iter::once("deadline".to_string()).chain(alive.iter().map(|p| p.to_string())).collect::<Vec<_>>().join(" "));
    if alive.is_empty() {
        fail!("sheepdog: members are still alive at the kill deadline, but none could be listed. The tree is NOT clean.");
    } else {
        fail!(
            "sheepdog: {} process(es) still alive at the kill deadline: pids {:?}. The tree is NOT clean.",
            alive.len(),
            alive
        );
    }
    125
}

fn run(argv: Vec<OsString>) -> i32 {
    let rest = argv.get(2..).unwrap_or(&[]);
    match argv.get(1).map(|a| a.as_bytes()) {
        None => {
            say!("{HELP}");
            return 2;
        }
        Some(b"--help" | b"-h") => return help(None),
        Some(b"help") => return help(argv.get(2)),
        Some(b"--version") => {
            #[cfg(target_os = "macos")]
            let api = if macos::spi_active() { ", responsibility API: active" } else { ", responsibility API: MISSING" };
            #[cfg(target_os = "linux")]
            let api = "";
            let _ = writeln!(std::io::stdout(), "sheepdog {} ({}, {}{api})", env!("CARGO_PKG_VERSION"), env!("SHEEPDOG_COMMIT"), std::env::consts::OS);
            return 0;
        }
        Some(b"doctor") => return json_error("doctor", rest, doctor::main(rest)),
        Some(b"kill") => return json_error("kill", rest, kill::main(rest)),
        Some(b"strays") => return json_error("strays", rest, strays::main(rest)),
        Some(b"ps") => return json_error("ps", rest, kill::ps(rest)),
        Some(b"sweep") => return json_error("sweep", rest, sweep::main(rest)),
        Some(b"run") => {}
        Some(_) => {
            let typed: Vec<String> = argv[1..].iter().map(|a| kill::clean(&a.to_string_lossy())).collect();
            // shell-quoted, so the line pasted into a shell runs what was typed
            let q: Vec<String> = typed.iter().map(|t| shell_quote(t)).collect();
            let dash = typed.iter().position(|t| t == "--");
            let sub = typed[..dash.unwrap_or(typed.len())].iter().position(|t| ["run", "kill", "strays", "ps", "sweep", "doctor"].contains(&t.as_str()));
            // a word shown escaped (a control character, bytes that are not UTF-8) cannot be
            // pasted back as typed: then no runnable line, only its shape
            let exact = argv[1..].iter().zip(&typed).all(|(a, t)| a.as_bytes() == t.as_bytes());
            let fix = match sub {
                _ if !exact => "sheepdog run -- COMMAND".to_string(),
                // options, then a subcommand: that subcommand, with the rest as typed
                Some(i) if typed[0].starts_with('-') => {
                    let mut rest = q.clone();
                    let name = rest.remove(i);
                    format!("sheepdog {name} {}", rest.join(" "))
                }
                // options (or `--`) first: they are run's, so `run` goes in front of them
                _ if typed[0].starts_with('-') => {
                    let tail = match dash {
                        None => " -- COMMAND",
                        Some(d) if d + 1 == typed.len() => " COMMAND",
                        Some(_) => "",
                    };
                    format!("sheepdog run {}{tail}", q.join(" "))
                }
                _ => format!("sheepdog run -- {}", q.join(" ")),
            };
            fail!("sheepdog: '{}' is not a sheepdog command. To run it under sheepdog: {fix}", typed[0]);
            return 2;
        }
    }
    status::start();
    // read --status-fd before the rest, so a usage error elsewhere still gets its status line
    // (a usage error in --status-fd itself writes nothing: PHASE2.md decision 8)
    if let Some(fd) = prescan_status_fd(&argv) {
        status::set_fd(fd);
    }
    let args = match parse(argv) {
        Ok(a) => a,
        Err(code) => {
            status::set_error("usage");
            return status::write(code);
        }
    };
    status::set_quiet(args.quiet);
    caps::set(args.timeout, args.max_mem, args.max_procs);
    if seam("SHEEPDOG_TEST_PANIC_IN_RUN") {
        panic!("test seam: panic in run");
    }
    let sig = setup_signals();
    #[cfg(target_os = "macos")]
    let code = macos::run(&args, &sig);
    #[cfg(target_os = "linux")]
    let code = linux::run(&args, &sig);
    status::write(code)
}

#[cfg(not(test))]
#[no_mangle]
pub extern "C" fn main(argc: c_int, argv: *const *const std::os::raw::c_char) -> c_int {
    use std::os::unix::ffi::OsStringExt;
    let argv: Vec<OsString> = (0..argc.max(0) as usize)
        .map(|i| OsString::from_vec(unsafe { std::ffi::CStr::from_ptr(*argv.add(i)) }.to_bytes().to_vec()))
        .collect();
    // the Linux root shim runs before anything else: it must not touch a signal disposition
    #[cfg(target_os = "linux")]
    if argv.get(1).map(|a| a.as_bytes()) == Some(b"__root") {
        return linux::root_shim(&argv);
    }
    // PHASE2.md §0.5: a release build has none of the test walls, so it refuses to run in a test
    // environment rather than act on this machine's real state and processes
    if !cfg!(debug_assertions) && (std::env::var_os("SHEEPDOG_TEST_TAG").is_some() || std::env::var_os("SHEEPDOG_TEST_STATE").is_some()) {
        say!("sheepdog: a release build does not run in a test environment (SHEEPDOG_TEST_TAG or SHEEPDOG_TEST_STATE is set)");
        return 125;
    }
    // doctor's disclaimed probe children (macOS): after the release refusal, like every entry
    #[cfg(target_os = "macos")]
    if let Some(code) = macos::doctor_probe(&argv) {
        return code;
    }
    note(format!("start debug {}", std::process::id()));
    // a panic must not unwind out of an extern "C" fn (undefined behaviour before Rust 1.81)
    std::panic::catch_unwind(|| run(argv)).unwrap_or_else(|_| status::write(125))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first failure a command reports is its JSON error message; lines said after it
    /// (context, a second usage line) do not replace it.
    #[test]
    fn the_first_failure_is_the_error_message() {
        fail!("sheepdog: the first failure");
        say!("sheepdog: a later line");
        fail!("sheepdog: a later failure");
        assert_eq!(error_message(), "sheepdog: the first failure");
    }

    /// Durations take `ms`, `s`, `m`, `h` and `d` (a bare number is seconds), as the help screen
    /// and PLAN's examples use them (`--timeout 5m`, `--older-than 10m`); anything else, a
    /// negative value and one past the cap are refused.
    #[test]
    fn durations_take_every_unit_the_help_uses() {
        let d = |s: &str| parse_duration(s);
        assert_eq!(d("250ms"), Some(Duration::from_millis(250)));
        assert_eq!(d("1.5"), Some(Duration::from_millis(1500)));
        assert_eq!(d("2s"), Some(Duration::from_secs(2)));
        assert_eq!(d("5m"), Some(Duration::from_secs(300)));
        assert_eq!(d("1.5m"), Some(Duration::from_secs(90)));
        assert_eq!(d("1h"), Some(Duration::from_secs(3600)));
        assert_eq!(d("1d"), Some(Duration::from_secs(86_400)));
        assert_eq!(d("0"), Some(Duration::ZERO));
        for bad in ["", "5x", "m", "-1s", "-5m", "1e999s", "nan", "inf", "5 m", "5mm", "1.5ms", "86401", "2d", "+5ms", "+5s", "+5"] {
            assert_eq!(d(bad), None, "{bad:?} was accepted");
        }
        // a job's own length (--timeout, --older-than) may be up to 365 days
        assert_eq!(parse_long_duration("2d"), Some(Duration::from_secs(2 * 86_400)));
        assert_eq!(parse_long_duration("365d"), Some(Duration::from_secs(365 * 86_400)));
        assert_eq!(parse_long_duration("366d"), None);
    }

    /// S4 review (A-P1-1, round 2 A-P2-2): the hint names only sheepdog's own group, and
    /// never a group of 1 or 0. As PID 1 in a container the group is 1, and `kill -INT -1`
    /// signals every process the reader may signal; 0 is the reader's own group; a caller's
    /// group (None) would reach the caller. A real own group (control) is named.
    #[test]
    fn the_hint_names_only_a_safe_own_group() {
        let targets = |own: Option<i32>| -> Vec<i64> {
            hint_text(libc::SIGINT, own)
                .split(|c: char| c.is_whitespace() || "(),.".contains(c))
                .filter_map(|w| w.strip_prefix('-')?.parse::<i64>().ok())
                .collect()
        };
        assert_eq!(targets(Some(4242)), vec![4242], "control: the hint names a real own group");
        for own in [Some(0), Some(1), None] {
            assert!(targets(own).is_empty(), "{own:?}: the hint named a target: {:?}", targets(own));
        }
    }

    /// No signal ever goes to pid 1 or lower: 0 is sheepdog's own process group and -1 is
    /// every process the user owns (the 2026-09-26 host incident was a kill(-1) elsewhere).
    /// Signal 0 and the real identities, so only the guard can say No.
    #[test]
    fn no_signal_reaches_pid_one_or_below() {
        for p in [-1, 0, 1] {
            let id = sheepdog::ident::identity(p).unwrap_or(0);
            assert_eq!(signal(p, id, 0), Sent::No, "pid {p} (identity {id}) was signalled");
        }
    }

    /// Review round 3, F2: with no authoritative check, the deadline must not call the tree
    /// clean after ONE empty scan. The scan misses a live member in the pass that reaches the
    /// deadline and in the first deadline scan, then sees it in the second: the result must be
    /// Err. (Calling it clean after one empty scan returns Ok here.)
    #[test]
    fn the_deadline_needs_two_empty_scans_without_an_authoritative_check() {
        let mut child = std::process::Command::new("/bin/sleep").arg("5").spawn().unwrap();
        let pid = child.id() as i32;
        let id = sheepdog::ident::identity(pid).unwrap();
        let mut calls = 0;
        let opts = KillOpts { deadline: Duration::ZERO, grace: Duration::ZERO, forget: false, never_empty: false, panic_after_stop: false, wrong_freeze: None, hold: Vec::new() };
        let r = kill_tree(
            &opts,
            || {
                calls += 1;
                if calls <= 2 { vec![] } else { vec![(pid, id)] } // missed twice, then seen
            },
            || {},
            || None,
            |_, _, _| Sent::No, // never signal: the member stays alive
            HashMap::new(),
        );
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(r, Err(KillError::Deadline(vec![pid])));
    }

    fn proc_state(pid: i32) -> char {
        std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|s| s.rfind(')').and_then(|i| s[i + 1..].split_whitespace().next().and_then(|f| f.chars().next())))
            .unwrap_or('?')
    }

    /// Two processes (the relay and the supervisor) append to one signal log: a line must never
    /// be merged with another writer's (S5 review round 8, P3-2).
    #[test]
    fn trace_lines_from_concurrent_writers_are_never_merged() {
        let log = std::env::temp_dir().join(format!("sd-trace-merge-{}", std::process::id()));
        let _ = std::fs::remove_file(&log);
        let writers: Vec<_> = ["relay-continued", "kill 12345 19"]
            .into_iter()
            .map(|line| {
                let log = log.clone();
                std::thread::spawn(move || {
                    for _ in 0..5000 {
                        trace_to(&log, line);
                    }
                })
            })
            .collect();
        for w in writers {
            w.join().unwrap();
        }
        let s = std::fs::read_to_string(&log).unwrap();
        let _ = std::fs::remove_file(&log);
        let bad = s.lines().filter(|l| *l != "relay-continued" && *l != "kill 12345 19").count();
        assert_eq!(bad, 0, "merged or split lines in the log");
        assert_eq!(s.lines().count(), 10_000);
    }

    /// PLAN.md §3.3 steps 2 and 4, the real race (Linux, needs --privileged for ns_last_pid; run
    /// with SD_REUSE_TEST=1, as the Linux matrix does): the member's pid is reused by a stranger
    /// between the identity check and the kill (the WRONG_FREEZE seam makes the check pass, as
    /// that race does), so the STOP lands on the stranger. The identity is read AFTER the STOP,
    /// so the stranger is recorded, and it is resumed whatever its state was: SD_MEMBER_STOPPED
    /// (the user had stopped the member, the stranger runs), SD_OTHER_STOPPED (another actor had
    /// stopped the stranger: resumed too, the stated cost of an unknowable prior state), neither
    /// (both running).
    #[cfg(target_os = "linux")]
    #[test]
    fn a_stop_that_lands_on_a_reused_pid_is_rolled_back() {
        if std::env::var("SD_REUSE_TEST").is_err() {
            return;
        }
        std::env::set_var("SHEEPDOG_TEST_PIDFD_ENOSYS", "1");
        let log = std::env::temp_dir().join(format!("sd-race-log-{}", std::process::id()));
        std::env::set_var("SHEEPDOG_TEST_SIGNAL_LOG", &log);
        for leg in ["member-stopped", "other-stopped", "both-running", "stranger-is-member"] {
            let _ = std::fs::remove_file(&log);
            let mut m = std::process::Command::new("/bin/sleep").arg("300").spawn().unwrap();
            let mp = m.id() as i32;
            std::thread::sleep(Duration::from_millis(50));
            let mid = sheepdog::ident::identity(mp).unwrap();
            if leg == "member-stopped" {
                unsafe { libc::kill(mp, libc::SIGSTOP) };
                while proc_state(mp) != 'T' {
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
            let stranger: std::cell::RefCell<Option<std::process::Child>> = std::cell::RefCell::new(None);
            let sent_stop: std::cell::Cell<Option<Sent>> = std::cell::Cell::new(None);
            let mut calls = 0;
            let opts = KillOpts { deadline: Duration::from_secs(2), grace: Duration::ZERO, forget: false, never_empty: false, panic_after_stop: false, wrong_freeze: None, hold: Vec::new() };
            let r = kill_tree(
                &opts,
                || {
                    calls += 1;
                    if calls == 1 {
                        vec![(mp, mid)]
                    } else if leg == "stranger-is-member" && stranger.borrow().is_some() {
                        // the stranger is itself a member of the job (another member forked it)
                        sheepdog::ident::identity(mp).map(|sid| vec![(mp, sid)]).unwrap_or_default()
                    } else {
                        vec![]
                    }
                },
                || {},
                || None,
                |p, id, sig| {
                    if sig == libc::SIGSTOP && p == mp && stranger.borrow().is_none() {
                        // the identity check passed (the member was there); before the kill the
                        // member dies and its pid goes to a stranger
                        unsafe { libc::kill(mp, libc::SIGKILL) };
                        let _ = m.wait();
                        std::fs::write("/proc/sys/kernel/ns_last_pid", format!("{}", mp - 1)).unwrap();
                        let s = std::process::Command::new("/bin/sleep").arg("301").spawn().unwrap();
                        let (spid, got_pid) = (s.id() as i32, s.id() as i32 == mp);
                        // kept before the controls, so a failed control cannot leak it (the
                        // cleanup below kills it)
                        *stranger.borrow_mut() = Some(s);
                        if !got_pid {
                            unsafe { libc::kill(spid, libc::SIGKILL) };
                        }
                        assert!(got_pid, "control: the stranger did not get the member's pid");
                        std::thread::sleep(Duration::from_millis(20));
                        if sheepdog::ident::same(mp, mid) {
                            unsafe { libc::kill(mp, libc::SIGKILL) };
                        }
                        assert!(!sheepdog::ident::same(mp, mid), "control: the stranger shares the member's start tick");
                        if leg == "other-stopped" {
                            unsafe { libc::kill(mp, libc::SIGSTOP) };
                            while proc_state(mp) != 'T' {
                                std::thread::sleep(Duration::from_millis(1));
                            }
                        }
                        std::env::set_var("SHEEPDOG_TEST_WRONG_FREEZE", mp.to_string());
                        let sent = signal(p, id, sig);
                        std::env::remove_var("SHEEPDOG_TEST_WRONG_FREEZE");
                        sent_stop.set(Some(sent));
                        return sent;
                    }
                    signal(p, id, sig)
                },
                HashMap::new(),
            );
            std::thread::sleep(Duration::from_millis(100));
            let state = proc_state(mp);
            let mut s = stranger.borrow_mut().take().expect("control: the reuse was never staged");
            let alive = matches!(s.try_wait(), Ok(None));
            let _ = s.kill();
            let _ = s.wait();
            let trace = std::fs::read_to_string(&log).unwrap_or_default();
            assert_eq!(sent_stop.get(), Some(Sent::Unpinned), "{leg}: control: the STOP went by kill");
            assert_eq!(r, Ok(()), "{leg}");
            if leg == "stranger-is-member" {
                // a member is killed, never resumed on the way
                assert!(!alive, "{leg}: the member was not killed");
                assert!(!trace.contains("rollback "), "{leg}: a member was resumed before its KILL: {trace}");
            } else {
                assert!(alive, "{leg}: the stranger our STOP landed on was killed: {trace}");
                assert!(matches!(state, 'S' | 'R'), "{leg}: the stranger our STOP landed on was left in state {state}");
            }
        }
        let _ = std::fs::remove_file(&log);
        // the variables are process-wide: leave none behind for the other unit cells
        std::env::remove_var("SHEEPDOG_TEST_PIDFD_ENOSYS");
        std::env::remove_var("SHEEPDOG_TEST_SIGNAL_LOG");
        // ./test-all's Linux legs require this line (the cell returns early without SD_REUSE_TEST)
        eprintln!("race cell ran: 4 legs");
    }
}
