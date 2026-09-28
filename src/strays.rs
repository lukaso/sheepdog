//! `sheepdog strays` (PLAN.md §3.0, PHASE2.md D8): list this user's orphans, biggest memory
//! first, with the best-known origin of each; `--kill` kills the listed rows that match a filter,
//! each as `sheepdog kill PID:ID` with the identity read at the listing.
//!
//! What counts as an orphan: macOS: parent launchd, `puniq` not 1 (it was reparented), and not a
//! helper of a live app (its responsible process is alive, is not itself, and the stray's
//! executable is inside that app's `.app` bundle). Linux: parent PID 1 or a known reaper; when PID
//! 1 is not an init, a child of PID 1 is marked (`pid1-child`) and `--kill` skips it unless
//! named. A row of a running supervised job is marked with the job and skipped unless named.
//!
//! Exit codes: 0 listed (or every row killed or gone); 1 refused (`--kill` in a non-tty without
//! `--yes`, or a row that `kill` refused); 2 usage error (also `--kill` without a filter); 125 a
//! row's kill deadline passed.

#[cfg(target_os = "linux")]
use crate::linux as os;
#[cfg(target_os = "macos")]
use crate::macos as os;
use crate::{parse_duration, say};
use sheepdog::ident::same;
use std::collections::HashMap;
use std::ffi::{CString, OsString};
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::time::Duration;

/// Linux parents that adopt orphans on purpose (PLAN §3.0, by name: nothing in /proc shows the
/// subreaper flag).
#[cfg(target_os = "linux")]
const REAPERS: [&str; 6] = ["systemd", "tini", "docker-init", "dumb-init", "catatonit", "s6-svscan"];

/// Of those, the inits that start a program of their own (a container's, a supervision tree's):
/// a child of one is marked and skipped by `--kill` unless named, whatever its pid (PHASE2.md D9).
/// Only an adoptee of an OS init (`systemd`, `init`) is an unmarked stray.
#[cfg_attr(target_os = "macos", allow(dead_code))]
const PROGRAM_INITS: [&str; 5] = ["tini", "docker-init", "dumb-init", "catatonit", "s6-svscan"];

/// Linux: is PID 1 (by its command name) an OS init, whose adoptees are strays? In a container,
/// tini, docker-init and the like start the container's own program, so their children are
/// marked and skipped unless named (PHASE2.md D9).
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn pid1_is_os_init(name: &str) -> bool {
    name == "systemd" || name == "init"
}

/// Linux: where a cgroup file (`/proc/<pid>/cgroup`, v2 or the v1 name=systemd line) places a
/// process (PHASE2.md D9).
#[cfg_attr(target_os = "macos", allow(dead_code))]
#[derive(Debug, PartialEq)]
enum Cgroup {
    /// a systemd unit's (`*.service` or `init.scope` leaf): adopted on purpose, never a stray
    Service,
    /// a desktop app's scope (`app-*.scope`): listed, but skipped by `--kill` unless named
    AppScope,
    Other,
}

#[cfg_attr(target_os = "macos", allow(dead_code))]
fn cgroup_kind(text: &str) -> Cgroup {
    let line = text.lines().find(|l| l.starts_with("0::")).or_else(|| text.lines().find(|l| l.contains(":name=systemd:")));
    let Some(path) = line.and_then(|l| l.splitn(3, ':').nth(2)) else { return Cgroup::Other };
    let leaf = path.rsplit('/').next().unwrap_or("");
    if leaf.ends_with(".service") || leaf == "init.scope" {
        Cgroup::Service
    } else if leaf.starts_with("app-") && leaf.ends_with(".scope") {
        Cgroup::AppScope
    } else {
        Cgroup::Other
    }
}

struct Args {
    min_mem: u64,
    older_than: Duration,
    cmd: Option<String>,
    pids: Vec<(i32, u64)>,
    json: bool,
    kill: bool,
    yes: bool,
}

fn usage() -> i32 {
    say!("usage: sheepdog strays [--min-mem SIZE] [--older-than DURATION] [--cmd REGEX] [--pid PID:ID]... [--json] [--kill [--yes]]");
    2
}

fn parse(args: &[OsString]) -> Option<Args> {
    let mut a = Args { min_mem: 0, older_than: Duration::ZERO, cmd: None, pids: Vec::new(), json: false, kill: false, yes: false };
    let mut i = 0;
    while i < args.len() {
        let val = |i: usize| args.get(i + 1).map(|v| v.to_string_lossy().into_owned());
        match args[i].as_bytes() {
            b"--min-mem" => {
                a.min_mem = crate::caps::parse_size(&val(i)?)?;
                i += 1;
            }
            b"--older-than" => {
                a.older_than = parse_duration(&val(i)?)?;
                i += 1;
            }
            b"--cmd" => {
                a.cmd = Some(val(i)?);
                i += 1;
            }
            b"--pid" => {
                let v = val(i)?;
                let (p, id) = v.split_once(':')?;
                a.pids.push((p.parse().ok().filter(|&p: &i32| p > 1)?, id.parse().ok()?));
                i += 1;
            }
            b"--json" => a.json = true,
            b"--kill" => a.kill = true,
            b"--yes" => a.yes = true,
            _ => return None,
        }
        i += 1;
    }
    Some(a)
}

/// A POSIX extended regex (libc), matched against a whole command line.
struct Regex(Box<libc::regex_t>);

impl Regex {
    fn new(pat: &str) -> Option<Regex> {
        let c = CString::new(pat).ok()?;
        let mut r: Box<libc::regex_t> = Box::new(unsafe { std::mem::zeroed() });
        (unsafe { libc::regcomp(&mut *r, c.as_ptr(), libc::REG_EXTENDED | libc::REG_NOSUB) } == 0).then(|| Regex(r))
    }
    fn is_match(&self, s: &str) -> bool {
        let Ok(c) = CString::new(s.replace('\0', " ")) else { return false };
        unsafe { libc::regexec(&*self.0, c.as_ptr(), 0, std::ptr::null_mut(), 0) == 0 }
    }
}

impl Drop for Regex {
    fn drop(&mut self) {
        unsafe { libc::regfree(&mut *self.0) };
    }
}

struct Row {
    pid: i32,
    id: u64,
    mem: u64,
    age: Option<u64>,
    cpu: Option<f64>,
    cmd: String,
    origin: Vec<String>,
    /// how many processes its kill would take besides itself (its proved tree)
    tree: usize,
    /// named with --pid (read before the debug seam changes identities)
    named: bool,
    /// the running job it belongs to ("j-..." or "sheepdog PID")
    job: Option<String>,
    pid1_child: bool,
    /// Linux: in a desktop app's scope (skipped unless named)
    app_scope: bool,
    /// the whole command line, for `--cmd` (the shown `cmd` is capped and escaped)
    full: String,
}

/// The rows of running jobs: every live journal's members, by (pid, identity).
fn live_journal_members() -> HashMap<(i32, u64), String> {
    let mut m = HashMap::new();
    let Some(state) = crate::state::resolve(cfg!(debug_assertions), |k| std::env::var_os(k), |p| p.exists()) else { return m };
    let Some(dir) = crate::sweep::folder(&state) else { return m };
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.extension().map_or(true, |x| x != "journal") {
            continue;
        }
        let Some((job, sup)) = crate::sweep::read_header(&p) else { continue };
        if !same(sup.0, sup.1) {
            continue; // a dead job: its strays are listed as strays
        }
        m.insert(sup, job.clone());
        for k in crate::sweep::peek(&p) {
            m.insert(k, job.clone());
        }
    }
    m
}

/// This user's orphans (unfiltered), each with its facts.
fn scan() -> Vec<Row> {
    let uid = unsafe { libc::getuid() };
    let procs = os::procs();
    let by_pid: HashMap<i32, &crate::kill::Proc> = procs.iter().map(|p| (p.pid, p)).collect();
    let jobs = live_journal_members();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    // a child of PID 1 is marked (skipped unless named) unless PID 1 is an OS init: in a
    // container, tini, docker-init and the like start the container's own program (D9)
    #[cfg(target_os = "linux")]
    let pid1_app = !pid1_is_os_init(&os::comm(1).unwrap_or_default());
    let protected = crate::kill::protected().unwrap_or_default();
    let me = unsafe { libc::getpid() };
    let mut rows = Vec::new();
    for p in &procs {
        // never this process (its ancestors are rows: `--kill` refuses them by kill's checks)
        if p.uid != uid || p.pid <= 1 || p.pid == me {
            continue;
        }
        #[cfg(target_os = "macos")]
        let (orphan, pid1_child, app_scope) = (p.ppid == 1 && p.puniq != Some(1) && !app_helper(p.pid), false, false);
        #[cfg(target_os = "linux")]
        let (orphan, pid1_child, app_scope) = {
            let parent = os::comm(p.ppid).unwrap_or_default();
            let reaped = p.ppid == 1 || (p.ppid > 1 && REAPERS.contains(&parent.as_str()));
            let kind = std::fs::read_to_string(format!("/proc/{}/cgroup", p.pid)).map_or(Cgroup::Other, |t| cgroup_kind(&t));
            let orphan = reaped && kind != Cgroup::Service;
            let program_child = (p.ppid == 1 && pid1_app) || (p.ppid > 1 && PROGRAM_INITS.contains(&parent.as_str()));
            (orphan, orphan && program_child, orphan && kind == Cgroup::AppScope)
        };
        if !orphan {
            continue;
        }
        let mut origin = Vec::new();
        let leader = |q: i32, what: &str, origin: &mut Vec<String>| {
            if q > 1 && q != p.pid && by_pid.contains_key(&q) {
                origin.push(format!("{what} {q} = {}", crate::journal::text(os::cmdline(q).join(" ").as_bytes())));
            }
        };
        leader(p.pgid, "group leader", &mut origin);
        if p.sid != p.pgid {
            leader(p.sid, "session leader", &mut origin);
        }
        if let Some(t) = os::tty(p.pid) {
            origin.push(format!("tty {t}"));
        }
        if let Some(c) = os::cwd(p.pid) {
            origin.push(format!("cwd {c}"));
        }
        if let Some(u) = p.puniq.filter(|&u| u > 1) {
            match procs.iter().find(|q| q.pid != p.pid && q.puniq == Some(u)) {
                Some(q) => origin.push(format!("puniq {u} (dead), shared by live pid {}", q.pid)),
                None => origin.push(format!("puniq {u} (dead)")),
            }
        }
        // its proved tree: what `kill PID:ID` would take with it
        let mut t = crate::kill::Proved { known: HashMap::from([(p.pid, p.id)]), ever: std::collections::HashSet::from([p.id]), protected: protected.clone() };
        let tree_set = t.scan();
        let sup_in_tree = tree_set.iter().find(|&&(q, _)| q != p.pid && crate::kill::is_sheepdog(q)).map(|&(q, _)| q);
        // a row that is itself a live supervisor, a member of a running job, or holds a live
        // supervisor in its tree: skipped by `--kill` unless named
        let job = jobs
            .get(&(p.pid, p.id))
            .cloned()
            .or_else(|| {
                crate::kill::is_sheepdog(p.pid).then(|| match os::cmdline(p.pid).get(1).map(String::as_str) {
                    Some("run") => format!("sheepdog {}, a supervisor", p.pid),
                    Some(sub) => format!("sheepdog {} ({})", p.pid, crate::kill::clean(sub)),
                    None => format!("sheepdog {}", p.pid),
                })
            })
            .or_else(|| crate::kill::job_of(p.pid).map(|s| format!("sheepdog {s}")))
            .or_else(|| sup_in_tree.map(|s| format!("sheepdog {s} in its tree")));
        let full = os::cmdline(p.pid).join(" ");
        rows.push(Row {
            pid: p.pid,
            id: p.id,
            mem: crate::caps::mem_of(p.pid),
            age: os::start_secs(p.pid).map(|s| now.saturating_sub(s)),
            cpu: os::cpu_secs(p.pid),
            cmd: crate::journal::text(full.as_bytes()),
            origin,
            tree: tree_set.len().saturating_sub(1),
            named: false,
            job,
            pid1_child,
            app_scope,
            full,
        });
    }
    rows
}

/// macOS: a helper of a live app: its responsible process is alive, is another process, and
/// the stray's executable is inside that process's `.app` bundle.
#[cfg(target_os = "macos")]
fn app_helper(pid: i32) -> bool {
    let Some(r) = os::responsible_pid(pid).filter(|&r| r != pid && r > 1) else { return false };
    let (Some(app), Some(me)) = (os::exe_path(r), os::exe_path(pid)) else { return false };
    let Some(end) = app.find(".app/") else { return false };
    me.starts_with(&app[..end + 5])
}

pub fn main(args: &[OsString]) -> i32 {
    let Some(a) = parse(args) else { return usage() };
    let re = match &a.cmd {
        Some(pat) => match Regex::new(pat) {
            Some(r) => Some(r),
            None => {
                say!("sheepdog: --cmd {pat:?} is not a valid regular expression");
                return 2;
            }
        },
        None => None,
    };
    if a.cmd.as_deref() == Some("") {
        say!("sheepdog: an empty --cmd matches every command; give a pattern.");
        return 2;
    }
    let filtered = a.min_mem > 0 || !a.older_than.is_zero() || re.is_some() || !a.pids.is_empty();
    if a.kill && !filtered {
        say!("sheepdog: --kill needs a filter (--min-mem, --older-than, --cmd or --pid), so that it never kills every stray at once. Run sheepdog strays first to see them.");
        return 2;
    }
    // a phase-2 source: disabled outright under the phase-1 opt-out
    if crate::seam_flag("SHEEPDOG_TEST_PHASE1") {
        crate::note("source disabled".into());
        return 0;
    }
    let mut rows: Vec<Row> = scan()
        .into_iter()
        .filter(|r| r.mem >= a.min_mem)
        .filter(|r| a.older_than.is_zero() || r.age.is_some_and(|s| Duration::from_secs(s) >= a.older_than))
        .filter(|r| re.as_ref().map_or(true, |re| re.is_match(&r.full)))
        .filter(|r| a.pids.is_empty() || a.pids.contains(&(r.pid, r.id)))
        .collect();
    rows.sort_by(|x, y| y.mem.cmp(&x.mem).then(x.pid.cmp(&y.pid)));
    for r in &mut rows {
        r.named = a.pids.contains(&(r.pid, r.id));
    }
    if crate::seam_flag("SHEEPDOG_TEST_STRAYS_WRONG_ID") {
        for r in &mut rows {
            r.id = r.id.wrapping_add(1 << 40);
        }
    }
    print(&rows, a.json);
    if !a.kill {
        return 0;
    }
    if rows.is_empty() {
        say!("sheepdog: no stray matches; nothing to kill.");
        return 0;
    }
    if !a.yes {
        if unsafe { libc::isatty(0) } != 1 {
            say!("sheepdog: --kill without --yes asks first, and stdin is not a terminal. Add --yes to kill these {} process(es). Nothing was signalled.", rows.len());
            return 1;
        }
        say!("Kill these {} process(es)? [y/N] ", rows.len());
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        if !matches!(line.trim(), "y" | "Y" | "yes") {
            say!("sheepdog: nothing was signalled.");
            return 1;
        }
    }
    let mut worst = 0;
    for r in &rows {
        let named = r.named;
        if let Some(j) = &r.job {
            if !named {
                say!("sheepdog: skipping pid {}: it belongs to a running job ({j}); name it with --pid {}:{} to kill it anyway.", r.pid, r.pid, r.id);
                continue;
            }
        }
        if r.app_scope && !named {
            say!("sheepdog: skipping pid {}: it runs in a desktop app's scope (a launched app); name it with --pid {}:{} to kill it.", r.pid, r.pid, r.id);
            continue;
        }
        if r.pid1_child && !named {
            say!("sheepdog: skipping pid {}: its parent is an init that runs a program of its own (or PID 1, which is not an OS init here), so it may be that program; name it with --pid {}:{} to kill it.", r.pid, r.pid, r.id);
            continue;
        }
        // as `sheepdog kill PID:ID`: the identity read at the listing, and kill's target checks
        let code = crate::kill::main(&[OsString::from(format!("{}:{}", r.pid, r.id))]);
        worst = worst.max(code);
    }
    worst
}

/// A shown command line that was cut (at the journal's cap) ends with `…`: `--cmd` matched the
/// whole one, so the match can be in the part not shown.
fn shown(cmd: &str, full: &str) -> String {
    if full.len() > crate::journal::CMD_CAP {
        format!("{cmd}…")
    } else {
        cmd.to_string()
    }
}

fn print(rows: &[Row], json: bool) {
    for r in rows {
        let line = if json {
            let origin: Vec<String> = r.origin.iter().map(|o| crate::journal::json_str(o)).collect();
            format!(
                "{{\"pid\":{},\"id\":{},\"mem\":{},\"age_s\":{},\"cpu_s\":{},\"cmd\":{},\"origin\":[{}],\"tree\":{},\"job\":{},\"pid1_child\":{},\"app_scope\":{}}}",
                r.pid,
                r.id,
                r.mem,
                r.age.map_or("null".into(), |a| a.to_string()),
                r.cpu.map_or("null".into(), |c| format!("{c:.2}")),
                crate::journal::json_str(&shown(&r.cmd, &r.full)),
                origin.join(","),
                r.tree,
                r.job.as_deref().map_or("null".into(), crate::journal::json_str),
                r.pid1_child,
                r.app_scope
            )
        } else {
            let mut flags = String::new();
            if r.tree > 0 {
                flags.push_str(&format!(" (+{} in its tree)", r.tree));
            }
            if let Some(j) = &r.job {
                flags.push_str(&format!(" (job {j}, running)"));
            }
            if r.pid1_child {
                flags.push_str(" (child of PID 1 app?)");
            }
            if r.app_scope {
                flags.push_str(" (desktop app?)");
            }
            format!(
                "{}\t{}\t{}\t{}\t{}{flags}\t{}",
                r.pid,
                crate::kill::human(r.mem),
                r.age.map_or("?".into(), |a| format!("{a}s")),
                r.cpu.map_or("?".into(), |c| format!("{c:.1}s")),
                crate::kill::clean(&shown(&r.cmd, &r.full)),
                crate::kill::clean(&r.origin.join("; "))
            )
        };
        if writeln!(std::io::stdout(), "{line}").is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_os_init_as_pid1_makes_its_children_strays() {
        assert!(pid1_is_os_init("systemd") && pid1_is_os_init("init"));
        for n in ["docker-init", "tini", "dumb-init", "catatonit", "sh", "sheepdog", ""] {
            assert!(!pid1_is_os_init(n), "{n}");
        }
    }

    #[test]
    fn cgroups_place_units_apps_and_the_rest() {
        use Cgroup::*;
        assert_eq!(cgroup_kind("0::/user.slice/user-1000.slice/user@1000.service/app.slice/pipewire.service\n"), Service);
        assert_eq!(cgroup_kind("0::/user.slice/user-1000.slice/user@1000.service/init.scope\n"), Service);
        assert_eq!(cgroup_kind("12:pids:/x\n1:name=systemd:/system.slice/foo.service\n"), Service);
        assert_eq!(cgroup_kind("0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-gnome-firefox-1234.scope\n"), AppScope);
        assert_eq!(cgroup_kind("0::/user.slice/user-1000.slice/session-3.scope\n"), Other);
        assert_eq!(cgroup_kind("0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-org.gnome.Terminal.slice/vte-spawn-5a.scope\n"), Other);
        assert_eq!(cgroup_kind("0::/\n"), Other);
        assert_eq!(cgroup_kind(""), Other);
    }
}
