//! Phase-2 P1b: the Linux root shim (PHASE2.md §1 decision 7): the root is started through
//! `sheepdog __root`, which sets PR_SET_PDEATHSIG(SIGKILL), checks its parent, answers a
//! handshake, waits for the go byte (sent after the root is journaled), restores the caller's
//! mask and searches PATH as posix_spawnp did.
#![cfg(target_os = "linux")]

mod common;

use common::json::{self, Json};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

fn sheepdog() -> &'static str {
    common::test_env();
    env!("CARGO_BIN_EXE_sheepdog")
}
fn fixture() -> &'static str {
    common::test_env();
    env!("CARGO_BIN_EXE_sd-fixture")
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("sd-shim-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn state(d: &Path) -> PathBuf {
    let s = d.join("state");
    std::fs::create_dir_all(&s).unwrap();
    std::fs::write(s.join(".sheepdog-test"), b"").unwrap();
    s
}

fn wait_for(p: &Path, secs: u64) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    while !p.exists() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(5));
    }
    p.exists()
}

fn finish(mut c: std::process::Child) -> Option<i32> {
    let end = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(s) = c.try_wait().unwrap() {
            return s.code();
        }
        if Instant::now() > end {
            common::send_child(&mut c, libc::SIGKILL);
            let _ = c.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The root's (pid, identity) from the job's journal.
fn journaled_root(s: &Path) -> Option<(i32, u64)> {
    for b in std::fs::read_dir(s.join("jobs")).ok()?.flatten() {
        for f in std::fs::read_dir(b.path()).ok()?.flatten() {
            for l in std::fs::read_to_string(f.path()).unwrap_or_default().lines() {
                // a line being appended while this reads does not parse: skip it
                let Ok(j) = json::parse(l) else { continue };
                if j.get("root") == Some(&Json::Bool(true)) {
                    return Some((j.get("pid")?.num()? as i32, j.get("id")?.num()? as u64));
                }
            }
        }
    }
    None
}

fn gone_within(p: (i32, u64), secs: u64) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    while common::alive(p) && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(10));
    }
    !common::alive(p)
}

/// PR_SET_PDEATHSIG(SIGKILL): a supervisor killed by SIGKILL takes its root with it.
#[test]
fn the_root_dies_with_its_supervisor() {
    let d = scratch("pdeath");
    let s = state(&d);
    let ready = d.join("ready");
    let mut c = Command::new(sheepdog())
        .args(["run", "--", "/bin/sh", "-c", &format!(r#"touch "{}"; exec sleep 30"#, ready.display())])
        .env("SHEEPDOG_TEST_STATE", &s)
        .spawn()
        .unwrap();
    let started = wait_for(&ready, 20);
    let root = journaled_root(&s);
    common::send_child(&mut c, libc::SIGKILL);
    let _ = c.wait();
    let gone = root.is_some_and(|r| gone_within(r, 5));
    if let Some(r) = root {
        common::send(r.0, r.1, libc::SIGKILL);
    }
    assert!(started && root.is_some());
    assert!(gone, "the root outlived its SIGKILLed supervisor");
    let _ = std::fs::remove_dir_all(&d);
}

/// The shim never execs when its supervisor is gone before the go byte (EOF on the go pipe).
/// PDEATHSIG is switched off in the shim by a seam, so the EOF path is the only one tested.
#[test]
fn a_shim_that_loses_its_supervisor_never_execs() {
    let d = scratch("eof");
    let s = state(&d);
    let (rel, ready, ran) = (d.join("release"), d.join("ready"), d.join("ran"));
    let mut c = Command::new(sheepdog())
        .args(["run", "--", "/bin/sh", "-c", &format!(r#"touch "{}""#, ran.display())])
        .env("SHEEPDOG_TEST_STATE", &s)
        .env("SHEEPDOG_TEST_HOLD_BEFORE_GO", &rel)
        .env("SHEEPDOG_TEST_READY_FILE", &ready)
        .env("SHEEPDOG_TEST_SHIM_NO_PDEATHSIG", "1")
        .spawn()
        .unwrap();
    let held = wait_for(&ready, 20);
    let root = journaled_root(&s);
    common::send_child(&mut c, libc::SIGKILL);
    let _ = c.wait();
    let gone = root.is_some_and(|r| gone_within(r, 5));
    std::thread::sleep(Duration::from_millis(200));
    let never_ran = !ran.exists();
    if let Some(r) = root {
        common::send(r.0, r.1, libc::SIGKILL);
    }
    assert!(held && root.is_some(), "held before the go byte");
    assert!(gone, "the shim waited on");
    assert!(never_ran, "the shim ran the command without its go byte");
    let _ = std::fs::remove_dir_all(&d);
}

/// A shim that cannot set PDEATHSIG, or that finds another parent than its supervisor, never
/// runs the command: the root is `not-started` and sheepdog exits 125 (never 127, "not found").
#[test]
fn a_shim_failure_is_not_started() {
    for seam in ["SHEEPDOG_TEST_SHIM_PRCTL_FAIL", "SHEEPDOG_TEST_SHIM_WRONG_PARENT"] {
        let d = scratch(&seam.to_lowercase());
        let (out, ran) = (d.join("status"), d.join("ran"));
        let code = finish(
            Command::new("/bin/sh")
                .args(["-c", &format!(r#"exec "$SD" run --status-fd 3 -- /bin/sh -c 'touch "{}"' 3>"{}""#, ran.display(), out.display())])
                .env("SD", sheepdog())
                .env(seam, "1")
                .spawn()
                .unwrap(),
        );
        let st = json::parse(std::fs::read_to_string(&out).unwrap_or_default().trim_end()).ok();
        assert_eq!(code, Some(125), "{seam}");
        assert!(!ran.exists(), "{seam}: the command ran");
        assert_eq!(st.as_ref().and_then(|j| j.get("root")).and_then(Json::str), Some("not-started"), "{seam}");
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// The shim searches PATH as phase 1's posix_spawnp did, measured on musl and glibc before the
/// change (2026-09-28): no `/bin/sh` fallback for a script with no shebang line (126); a PATH
/// entry that is not executable is skipped (0) and, alone, gives 126; an empty element is the
/// working directory; an unset PATH uses the libc's default; not found is 127.
#[test]
fn the_path_search_matches_phase_one() {
    let d = scratch("path");
    let w = |p: &Path, body: &str, mode: u32| {
        std::fs::write(p, body).unwrap();
        std::fs::set_permissions(p, std::os::unix::fs::PermissionsExt::from_mode(mode)).unwrap();
    };
    for sub in ["a", "b", "c"] {
        std::fs::create_dir_all(d.join(sub)).unwrap();
    }
    w(&d.join("noshebang"), &format!("echo x > {}/ns.out\n", d.display()), 0o755);
    w(&d.join("a/tool"), "#!/bin/sh\necho ran-a\n", 0o644);
    w(&d.join("b/tool"), "#!/bin/sh\necho ran-b\n", 0o755);
    w(&d.join("c/cwdtool"), "#!/bin/sh\necho ran-cwd\n", 0o755);
    let run = |path: Option<String>, cwd: &Path, cmd: &str| -> (Option<i32>, String) {
        let mut c = Command::new(sheepdog());
        c.args(["run", "--", cmd]).current_dir(cwd).stdout(std::process::Stdio::piped());
        match path {
            Some(p) => c.env("PATH", p),
            None => c.env_remove("PATH"),
        };
        let o = c.output().unwrap();
        (o.status.code(), String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    let base = "/usr/bin:/bin";
    let dd = d.display();
    assert_eq!(run(Some(base.into()), &d, &format!("{dd}/noshebang")), (Some(126), String::new()), "no shebang line");
    assert!(!d.join("ns.out").exists(), "a /bin/sh fallback ran it");
    assert_eq!(run(Some(format!("{dd}/a:{dd}/b:{base}")), &d, "tool"), (Some(0), "ran-b".into()), "EACCES, then found");
    assert_eq!(run(Some(format!("{dd}/a:{base}")), &d, "tool"), (Some(126), String::new()), "EACCES only");
    assert_eq!(run(Some(format!(":{base}")), &d.join("c"), "cwdtool"), (Some(0), "ran-cwd".into()), "empty element = cwd");
    assert_eq!(run(None, &d, "sh").0, Some(0), "unset PATH: the libc default");
    assert_eq!(run(Some(base.into()), &d, "no-such-tool-xyz").0, Some(127), "not found");
    let _ = std::fs::remove_dir_all(&d);
}

/// The command holds no fd of sheepdog's (the shim's go and error pipes, the journal): a command
/// that lists its own open fds (`exec ls -l /proc/self/fd`) finds no fd from 3 up that is a pipe,
/// a socket or a journal. The same listing without sheepdog is the control (it must pass the same
/// check, or the harness hands out such fds itself). Under an emulator the translator keeps fds
/// of the binaries it ran (measured under Rosetta: busybox, sheepdog); those are files, not pipes.
#[test]
fn the_command_holds_no_shim_fd() {
    let d = scratch("fds");
    let leaked = |under: bool| -> Vec<String> {
        let out = d.join(if under { "under" } else { "plain" });
        let script = format!(r#"exec ls -l /proc/self/fd > "{}""#, out.display());
        let code = if under {
            finish(Command::new(sheepdog()).args(["run", "--", "/bin/sh", "-c", &script]).spawn().unwrap())
        } else {
            finish(Command::new("/bin/sh").args(["-c", &script]).spawn().unwrap())
        };
        assert_eq!(code, Some(0));
        std::fs::read_to_string(&out)
            .unwrap()
            .lines()
            .filter_map(|l| {
                let (lhs, target) = l.split_once(" -> ")?;
                let fd: i32 = lhs.split_whitespace().last()?.parse().ok()?;
                let bad = target.starts_with("pipe:") || target.starts_with("socket:") || target.contains(".journal") || target.contains(".tmp-j-");
                (fd >= 3 && bad).then(|| l.to_string())
            })
            .collect()
    };
    assert!(leaked(false).is_empty(), "control: the harness itself hands the command a pipe");
    let under = leaked(true);
    assert!(under.is_empty(), "the command holds sheepdog's fds: {under:?}");
    let _ = std::fs::remove_dir_all(&d);
}

/// An inner `sheepdog run` replaces the PDEATHSIG(SIGKILL) it inherits from the outer's shim
/// with SIGTERM: when the outer is SIGKILLed, the inner gets TERM and kills its own job (its
/// escapee dies although the inner's root would run for 30 s more). The inner is the outer's
/// command itself (`exec`): a forked child inherits no PDEATHSIG (stated, PHASE2.md decision 7),
/// and whether a shell execs its last command differs (BusyBox does, dash does not).
#[test]
fn an_inner_run_kills_its_job_when_the_outer_is_sigkilled() {
    let d = scratch("nested");
    let r = d.join("rec");
    let m = format!("29.{:09}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos());
    let inner = format!(r#"exec sheepdog run -- /bin/sh -c '"$FX" escape {m} "$R"; sleep 30'"#);
    let mut c = Command::new(sheepdog()).args(["run", "--", "/bin/sh", "-c", &inner]).env("FX", fixture()).env("R", &r).spawn().unwrap();
    let end = Instant::now() + Duration::from_secs(20);
    let mut g = None;
    while g.is_none() && Instant::now() < end {
        g = std::fs::read_to_string(&r).ok().and_then(|t| {
            let mut w = t.split_whitespace();
            Some((w.next()?.parse().ok()?, w.next()?.parse().ok()?))
        });
        std::thread::sleep(Duration::from_millis(10));
    }
    common::send_child(&mut c, libc::SIGKILL);
    let _ = c.wait();
    let g: (i32, u64) = g.expect("the inner job's escapee started");
    let gone = gone_within(g, 10);
    common::send(g.0, g.1, libc::SIGKILL);
    common::kill_marked(&[&m]);
    assert!(gone, "the inner job's escapee outlived the outer's SIGKILL");
    let _ = std::fs::remove_dir_all(&d);
}

/// A signal that reaches the shim between the go byte and its exec (a ctrl-C in that window)
/// ends the shim by that signal: the root is reported `signaled`, never `not-started` or 127.
#[test]
fn a_signal_in_the_shim_window_is_the_root_s_death() {
    let d = scratch("window");
    let s = state(&d);
    let (rel, ready, ran, out) = (d.join("release"), d.join("ready"), d.join("ran"), d.join("status"));
    let c = Command::new("/bin/sh")
        .args(["-c", &format!(r#"exec "$SD" run --status-fd 3 -- /bin/sh -c 'touch "{}"' 3>"{}""#, ran.display(), out.display())])
        .env("SD", sheepdog())
        .env("SHEEPDOG_TEST_STATE", &s)
        .env("SHEEPDOG_TEST_HOLD_SHIM", &rel)
        .env("SHEEPDOG_TEST_READY_FILE", &ready)
        .spawn()
        .unwrap();
    let held = wait_for(&ready, 20);
    let root = journaled_root(&s);
    if let Some(r) = root {
        common::send(r.0, r.1, libc::SIGINT);
    }
    std::fs::write(&rel, b"").unwrap();
    let code = finish(c);
    let st = json::parse(std::fs::read_to_string(&out).unwrap_or_default().trim_end()).ok();
    assert!(held && root.is_some());
    assert!(!ran.exists(), "the command ran");
    assert_eq!(st.as_ref().and_then(|j| j.get("root")).and_then(Json::str), Some("signaled"));
    assert_eq!(code, Some(128 + libc::SIGINT));
    let _ = std::fs::remove_dir_all(&d);
}

/// A root killed before its go byte (a SIGKILL from outside while the supervisor journals it)
/// never takes the supervisor with it: the go byte to a dead reader must not raise SIGPIPE in
/// sheepdog. The run ends as a root killed by SIGKILL (137), with its status line written.
/// SIGPIPE is at its default in sheepdog here, as for a shell's command (the test binary ignores
/// it, and a disposition ignored on entry would hide the defect).
#[test]
fn a_root_killed_before_its_go_byte_does_not_kill_the_supervisor() {
    use std::os::unix::process::CommandExt;
    let d = scratch("gopipe");
    let s = state(&d);
    let (rel, ready, out) = (d.join("release"), d.join("ready"), d.join("status"));
    let mut cmd = Command::new("/bin/sh");
    unsafe {
        cmd.pre_exec(|| {
            libc::signal(libc::SIGPIPE, libc::SIG_DFL);
            Ok(())
        });
    }
    let c = cmd
        .args(["-c", &format!(r#"exec "$SD" run --status-fd 3 -- /bin/sh -c 'exit 0' 3>"{}""#, out.display())])
        .env("SD", sheepdog())
        .env("SHEEPDOG_TEST_STATE", &s)
        .env("SHEEPDOG_TEST_HOLD_BEFORE_GO", &rel)
        .env("SHEEPDOG_TEST_READY_FILE", &ready)
        .spawn()
        .unwrap();
    let held = wait_for(&ready, 20);
    let root = journaled_root(&s);
    if let Some(r) = root {
        common::send(r.0, r.1, libc::SIGKILL);
        gone_within(r, 5);
    }
    std::fs::write(&rel, b"").unwrap();
    let code = finish(c);
    let st = json::parse(std::fs::read_to_string(&out).unwrap_or_default().trim_end()).ok();
    assert!(held && root.is_some());
    assert_eq!(code, Some(128 + libc::SIGKILL), "sheepdog itself died (SIGPIPE?)");
    assert_eq!(st.as_ref().and_then(|j| j.get("root")).and_then(Json::str), Some("signaled"));
    let _ = std::fs::remove_dir_all(&d);
}

/// With PATH unset, the shim searches the libc's own default, as phase 1's posix_spawnp did: a
/// tool that exists only in /usr/local/bin gives the same exit code under sheepdog as under a
/// direct posix_spawnp (the fixture's `spawnp`) on this libc (musl searches /usr/local/bin,
/// glibc does not). Runs where /usr/local/bin is writable (the containers' root).
#[test]
fn an_unset_path_searches_what_posix_spawnp_searches() {
    let name = format!("sd-localtool-{}", std::process::id());
    let tool = Path::new("/usr/local/bin").join(&name);
    if std::fs::write(&tool, "#!/bin/sh\nexit 7\n").is_err() {
        // only the unprivileged leg may lack it; as root this is a red, never a skip
        assert_ne!(unsafe { libc::geteuid() }, 0, "root cannot write /usr/local/bin");
        eprintln!("skipped: /usr/local/bin is not writable for this user");
        return;
    }
    std::fs::set_permissions(&tool, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let code = |prog: &str, args: &[&str]| Command::new(prog).args(args).env_remove("PATH").status().unwrap().code();
    let direct = code(fixture(), &["spawnp", &name]);
    let under = code(sheepdog(), &["run", "--", &name]);
    let _ = std::fs::remove_file(&tool);
    assert!(direct == Some(7) || direct == Some(127), "control: {direct:?}");
    assert_eq!(under, direct, "sheepdog {under:?}, posix_spawnp {direct:?}");
}

/// An empty command name is "not found" (127), as posix_spawnp gives it.
#[test]
fn an_empty_command_name_is_not_found() {
    let under = Command::new(sheepdog()).args(["run", "--", ""]).status().unwrap().code();
    let direct = Command::new(fixture()).args(["spawnp", ""]).status().unwrap().code();
    assert_eq!(under, direct, "sheepdog {under:?}, posix_spawnp {direct:?}");
    assert_eq!(under, Some(127));
}

/// A TERM that reaches sheepdog before the go byte (while it journals the root) ends the job
/// before the command runs: the root is `not-started` and sheepdog dies of that TERM.
#[test]
fn a_term_before_the_go_byte_means_the_command_never_runs() {
    let d = scratch("lateterm");
    let s = state(&d);
    let (rel, ready, ran, out) = (d.join("release"), d.join("ready"), d.join("ran"), d.join("status"));
    let c = Command::new("/bin/sh")
        .args(["-c", &format!(r#"exec "$SD" run --status-fd 3 -- /bin/sh -c 'touch "{}"' 3>"{}""#, ran.display(), out.display())])
        .env("SD", sheepdog())
        .env("SHEEPDOG_TEST_STATE", &s)
        .env("SHEEPDOG_TEST_HOLD_BEFORE_GO", &rel)
        .env("SHEEPDOG_TEST_READY_FILE", &ready)
        .spawn()
        .unwrap();
    let held = wait_for(&ready, 20);
    let sd = c.id() as i32; // the shell exec'd sheepdog: same pid
    let sd_id = sheepdog::ident::identity(sd);
    if let Some(id) = sd_id {
        common::send(sd, id, libc::SIGTERM);
    }
    std::thread::sleep(Duration::from_millis(100));
    std::fs::write(&rel, b"").unwrap();
    let code = finish(c);
    std::thread::sleep(Duration::from_millis(200));
    let st = json::parse(std::fs::read_to_string(&out).unwrap_or_default().trim_end()).ok();
    assert!(held && sd_id.is_some());
    assert!(!ran.exists(), "the command ran after the TERM");
    assert_eq!(code, None, "sheepdog dies of the TERM");
    assert_eq!(st.as_ref().and_then(|j| j.get("root")).and_then(Json::str), Some("not-started"));
    let _ = std::fs::remove_dir_all(&d);
}

/// The TERM-before-go path never hangs on a shim that something else stopped: the shim is
/// SIGSTOPped while the go byte is held, then sheepdog gets TERM; it still ends (bounded wait,
/// then SIGKILL to the shim, its own unreaped child).
#[test]
fn a_term_before_go_ends_even_with_a_stopped_shim() {
    let d = scratch("stoppedshim");
    let s = state(&d);
    let (rel, ready) = (d.join("release"), d.join("ready"));
    let mut c = Command::new(sheepdog())
        .args(["run", "--", "/bin/sh", "-c", "exit 0"])
        .env("SHEEPDOG_TEST_STATE", &s)
        .env("SHEEPDOG_TEST_HOLD_BEFORE_GO", &rel)
        .env("SHEEPDOG_TEST_READY_FILE", &ready)
        .spawn()
        .unwrap();
    let held = wait_for(&ready, 20);
    let root = journaled_root(&s);
    if let Some(r) = root {
        common::send(r.0, r.1, libc::SIGSTOP);
    }
    common::send_child(&mut c, libc::SIGTERM);
    std::fs::write(&rel, b"").unwrap();
    let end = Instant::now() + Duration::from_secs(15);
    let mut ended = false;
    while Instant::now() < end {
        if c.try_wait().unwrap().is_some() {
            ended = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    if !ended {
        common::send_child(&mut c, libc::SIGKILL);
        let _ = c.wait();
    }
    if let Some(r) = root {
        common::send(r.0, r.1, libc::SIGKILL);
    }
    assert!(held && root.is_some());
    assert!(ended, "sheepdog hung on the stopped shim");
    let _ = std::fs::remove_dir_all(&d);
}

