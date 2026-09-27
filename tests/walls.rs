//! Phase-2 test walls (PHASE2.md §0): the cargo runner, the signal door's latch and tag check,
//! the state resolver's debug rule, and the release refusal.

mod common;

use std::os::unix::fs::PermissionsExt;
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
    let d = std::env::temp_dir().join(format!("sd-walls-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A fake target layout for the runner: `<d>/p/deps/<name>` is `script` (a shell script), with
/// the real fixture and sheepdog next to `deps`, as cargo lays them out.
fn fake_test_binary(d: &Path, name: &str, script: &str) -> PathBuf {
    let deps = d.join("p").join("deps");
    std::fs::create_dir_all(&deps).unwrap();
    std::os::unix::fs::symlink(fixture(), d.join("p").join("sd-fixture")).unwrap();
    std::os::unix::fs::symlink(sheepdog(), d.join("p").join("sheepdog")).unwrap();
    let bin = deps.join(name);
    std::fs::write(&bin, format!("#!/bin/sh\n{script}\n")).unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    bin
}

fn runner() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/test-env")
}

/// Run the runner on `bin` with a time bound; its exit code.
fn run_runner(bin: &Path) -> Option<i32> {
    let mut c = Command::new(runner()).arg(bin).spawn().unwrap();
    let end = Instant::now() + Duration::from_secs(20);
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

/// PHASE2.md §0.4: the runner passes the test binary's exit code on, and fails a run that wrote
/// to its canary state directory or its withheld sink (the controls: a quiet binary exits 0,
/// and an exit code the binary chose comes through unchanged).
#[test]
fn the_runner_checks_the_canary_and_the_sink() {
    let d = scratch("runner");
    let quiet = fake_test_binary(&d.join("q"), "quiet", "exit 0");
    let code = fake_test_binary(&d.join("c"), "code", "exit 7");
    let canary = fake_test_binary(&d.join("k"), "canary", r#"echo x > "$SHEEPDOG_TEST_STATE/f""#);
    let sink = fake_test_binary(&d.join("s"), "sink", r#"echo "withheld 42 9" >> "$SHEEPDOG_TEST_SINK""#);
    assert_eq!(run_runner(&quiet), Some(0), "control: a quiet test binary");
    assert_eq!(run_runner(&code), Some(7), "the test binary's own code");
    assert_eq!(run_runner(&canary), Some(1), "a write to the canary fails the run");
    assert_eq!(run_runner(&sink), Some(1), "a withheld line fails the run");
    let _ = std::fs::remove_dir_all(&d);
}

/// PHASE2.md §0.4: the runner removes an inherited SHEEPDOG_* (here a tag a shell exported by
/// hand, and a real-looking state directory), adopts only SHEEPDOG_LEG_TAG, and puts the debug
/// sheepdog first on PATH.
#[test]
fn the_runner_sets_the_test_environment() {
    let d = scratch("env");
    let out = d.join("env.txt");
    let bin = fake_test_binary(
        &d,
        "show",
        &format!(
            r#"{{ echo "tag=$SHEEPDOG_TEST_TAG"; echo "state=${{SHEEPDOG_STATE:-unset}}"; echo "xdg=${{XDG_STATE_HOME:-unset}}"; [ "$(command -v sheepdog)" -ef "{}" ] && echo debug-first; }} > "{}""#,
            sheepdog(),
            out.display()
        ),
    );
    let st = Command::new(runner())
        .arg(&bin)
        .env("SHEEPDOG_TEST_TAG", "0123456789abcdef0123456789abcdef")
        .env("SHEEPDOG_STATE", d.join("real"))
        .env("XDG_STATE_HOME", d.join("xdg"))
        .env("SHEEPDOG_LEG_TAG", "feedfacefeedfacefeedfacefeedface")
        .status()
        .unwrap();
    assert!(st.success());
    let text = std::fs::read_to_string(&out).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "tag=feedfacefeedfacefeedfacefeedface", "the leg tag, not the inherited one");
    assert_eq!(lines[1], "state=unset");
    assert_eq!(lines[2], "xdg=unset");
    assert_eq!(lines.get(3), Some(&"debug-first"), "the debug sheepdog first on PATH");
    // without a leg tag it makes a fresh one, never the inherited one
    let st = Command::new(runner()).arg(&bin).env("SHEEPDOG_TEST_TAG", "0123456789abcdef0123456789abcdef").env_remove("SHEEPDOG_LEG_TAG").status().unwrap();
    assert!(st.success());
    let text = std::fs::read_to_string(&out).unwrap();
    let tag = text.lines().next().unwrap().trim_start_matches("tag=");
    assert!(tag.len() == 32 && tag != "0123456789abcdef0123456789abcdef", "a fresh tag: {tag}");
    let _ = std::fs::remove_dir_all(&d);
}

/// PHASE2.md §0.4: a TERM to the runner reaches the test binary (it dies of TERM and the runner
/// reports it), so ctrl-C and test-all's cleanup still end a run.
#[test]
fn the_runner_forwards_term() {
    let d = scratch("term");
    let ready = d.join("ready");
    let bin = fake_test_binary(&d, "sleeper", &format!(r#"touch "{}"; exec sleep 30"#, ready.display()));
    let mut c = Command::new(runner()).arg(&bin).spawn().unwrap();
    let end = Instant::now() + Duration::from_secs(10);
    while !ready.exists() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(ready.exists(), "the test binary started");
    assert!(common::send_child(&mut c, libc::SIGTERM));
    let end = Instant::now() + Duration::from_secs(10);
    let code = loop {
        if let Some(s) = c.try_wait().unwrap() {
            break s.code();
        }
        if Instant::now() > end {
            common::send_child(&mut c, libc::SIGKILL);
            let _ = c.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(code, Some(128 + libc::SIGTERM), "the binary died of the forwarded TERM");
    let _ = std::fs::remove_dir_all(&d);
}

// ---- the signal door: the latch and the tag check (PHASE2.md §0.1, §0.3) ----

/// The `(pid, identity)` lines a fixture recorded, once `n` are there (10 s at most).
fn records(r: &Path, n: usize) -> Vec<(i32, u64)> {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        let text = std::fs::read_to_string(r).unwrap_or_default();
        let v: Vec<(i32, u64)> = text
            .lines()
            .filter_map(|l| {
                let mut w = l.split_whitespace();
                Some((w.next()?.parse().ok()?, w.next()?.parse().ok()?))
            })
            .collect();
        if v.len() >= n || Instant::now() > end {
            return v;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Run sheepdog to its end (30 s bound); its exit code.
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

/// Lines of the cell's own withheld sink.
fn sink_lines(sink: &Path) -> Vec<(String, i32, i32)> {
    std::fs::read_to_string(sink)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let w: Vec<&str> = l.split_whitespace().collect();
            Some((w.first()?.to_string(), w.get(1)?.parse().ok()?, w.get(2)?.parse().ok()?))
        })
        .collect()
}

/// The shape for the door cells: sheepdog's root starts a tagged `sigcount` member (TAGGED, with
/// `tag` in its environment; the suite's own tag when None) and an untagged one (`env -i`), waits
/// for both records, and exits, so the end-of-job kill aims at exactly those two.
fn two_members(d: &Path, sheepdog_env: &[(&str, &str)], remove_own_tag: bool, tag: Option<&str>) -> (Option<i32>, (i32, u64), (i32, u64)) {
    let (r1, r2) = (d.join("tagged"), d.join("untagged"));
    let tag_prefix = tag.map(|t| format!("SHEEPDOG_TEST_TAG={t} ")).unwrap_or_default();
    let script = format!(
        r#"{tag_prefix}"$FX" sigcount "$R1" & /usr/bin/env -i "$FX" sigcount "$R2" & while [ ! -s "$R1" ] || [ ! -s "$R2" ]; do sleep 0.01; done"#
    );
    let mut cmd = Command::new(sheepdog());
    cmd.args(["run", "--grace", "0", "--", "/bin/sh", "-c", &script])
        .env("FX", fixture())
        .env("R1", &r1)
        .env("R2", &r2)
        .env("SHEEPDOG_TEST_DEADLINE_MS", "1500")
        .env("SHEEPDOG_TEST_SIGNAL_LOG", d.join("log"))
        .env("SHEEPDOG_TEST_SINK", d.join("sink"));
    if remove_own_tag {
        cmd.env_remove("SHEEPDOG_TEST_TAG");
    }
    for (k, v) in sheepdog_env {
        cmd.env(k, v);
    }
    let code = finish(cmd.spawn().unwrap());
    let t = records(&r1, 1)[0];
    let u = records(&r2, 1)[0];
    (code, t, u)
}

fn counted(r: &Path) -> usize {
    std::fs::read_to_string(format!("{}.sig", r.display())).map(|s| s.lines().count()).unwrap_or(0)
}

/// Control: without the latch the door works as in phase 1: both members die, nothing is
/// withheld. This is what makes the withheld cells below evidence of the latch.
#[test]
fn without_the_latch_both_members_die() {
    let d = scratch("nolatch");
    let (code, t, u) = two_members(&d, &[], false, None);
    let (ta, ua) = (common::alive(t), common::alive(u));
    common::send(t.0, t.1, libc::SIGKILL);
    common::send(u.0, u.1, libc::SIGKILL);
    assert_eq!(code, Some(0));
    assert!(!ta && !ua);
    assert!(sink_lines(&d.join("sink")).is_empty());
    let _ = std::fs::remove_dir_all(&d);
}

/// PHASE2.md §0.1: with the latch on, the door signals only a target whose environment carries
/// sheepdog's own tag. The untagged member gets no signal at all (its own counter stays 0, it is
/// alive and not stopped); every withheld line names it; the tagged member dies.
#[test]
fn the_latch_withholds_every_signal_to_an_untagged_target() {
    let d = scratch("latch");
    let (code, t, u) = two_members(&d, &[("SHEEPDOG_TEST_LATCH", "1")], false, None);
    let lines = sink_lines(&d.join("sink"));
    let (tagged_alive, untagged_alive) = (common::alive(t), common::alive(u));
    common::send(t.0, t.1, libc::SIGKILL);
    common::send(u.0, u.1, libc::SIGKILL);
    assert!(!tagged_alive, "the tagged member dies");
    assert!(untagged_alive, "the untagged member survives");
    assert_eq!(counted(&d.join("untagged")), 0, "no catchable signal reached it");
    assert!(!lines.is_empty(), "withheld lines");
    assert!(lines.iter().all(|(k, p, _)| k == "withheld" && *p == u.0), "{lines:?}");
    assert_eq!(code, Some(125), "the untagged member outlives the deadline");
    let _ = std::fs::remove_dir_all(&d);
}

/// PHASE2.md §0.3: the panic path goes through the same door. The kill loop panics right after
/// its first freeze; the untagged member, never stopped, gets no SIGKILL from the panic path.
#[test]
fn the_panic_path_goes_through_the_door() {
    let d = scratch("panic");
    let (code, t, u) = two_members(&d, &[("SHEEPDOG_TEST_LATCH", "1"), ("SHEEPDOG_TEST_PANIC_AFTER_STOP", "1")], false, None);
    let lines = sink_lines(&d.join("sink"));
    let (tagged_alive, untagged_alive) = (common::alive(t), common::alive(u));
    common::send(t.0, t.1, libc::SIGKILL);
    common::send(u.0, u.1, libc::SIGKILL);
    assert!(!tagged_alive, "the tagged member dies by the panic path");
    assert!(untagged_alive, "the panic path did not kill the untagged member");
    assert!(lines.iter().any(|(k, p, s)| k == "withheld" && *p == u.0 && *s == libc::SIGKILL), "{lines:?}");
    assert!(lines.iter().all(|(_, p, _)| *p == u.0), "{lines:?}");
    assert_eq!(code, Some(125));
    let _ = std::fs::remove_dir_all(&d);
}

/// PHASE2.md §0.1: a door with no tag of its own withholds every target once the latch is on,
/// a tagged one and an untagged one alike (two absent tags never match).
#[test]
fn a_door_without_its_own_tag_withholds_everything() {
    let d = scratch("notag");
    let own = std::env::var("SHEEPDOG_TEST_TAG").unwrap();
    let (code, t, u) = two_members(&d, &[("SHEEPDOG_TEST_LATCH", "1")], true, Some(&own));
    let lines = sink_lines(&d.join("sink"));
    let (ta, ua) = (common::alive(t), common::alive(u));
    common::send(t.0, t.1, libc::SIGKILL);
    common::send(u.0, u.1, libc::SIGKILL);
    assert!(ta && ua, "both survive: tagged {ta}, untagged {ua}");
    assert!(lines.iter().any(|(_, p, _)| *p == u.0) && lines.iter().any(|(_, p, _)| *p == t.0), "{lines:?}");
    assert_eq!(code, Some(125));
    let _ = std::fs::remove_dir_all(&d);
}

/// PHASE2.md §0.1: the stress cell. 50 tagged fixture members, half of them exiting on TERM
/// while the kill runs, killed through the full TERM, STOP, KILL, CONT sequence with the latch
/// on, 20 times: every member dies and not one line is withheld or unreadable (an exiting
/// process is `gone`, and a verdict is taken once per identity, so the CONT after a KILL does
/// not re-read a dying process).
#[test]
fn the_latch_never_withholds_from_a_tagged_member_that_is_dying() {
    let d = scratch("stress");
    for i in 0..20 {
        let r = d.join(format!("rec{i}"));
        let sink = d.join(format!("sink{i}"));
        let c = Command::new(sheepdog())
            .args(["run", "--grace", "0.05", "--", fixture(), "swarm", "50"])
            .arg(&r)
            .env("SHEEPDOG_TEST_LATCH", "1")
            .env("SHEEPDOG_TEST_SINK", &sink)
            .spawn()
            .unwrap();
        let code = finish(c);
        let recs = records(&r, 50);
        let alive: Vec<_> = recs.iter().filter(|p| common::alive(**p)).copied().collect();
        for p in &alive {
            common::send(p.0, p.1, libc::SIGKILL);
        }
        assert_eq!(recs.len(), 50, "run {i}: 50 members recorded");
        assert!(alive.is_empty(), "run {i}: survivors {alive:?}");
        assert_eq!(std::fs::read_to_string(&sink).unwrap_or_default(), "", "run {i}: withheld lines");
        assert_eq!(code, Some(0), "run {i}");
    }
    let _ = std::fs::remove_dir_all(&d);
}

/// PHASE2.md §0.1: the verdict is taken once per (pid, identity). A debug seam makes every read
/// after a target's first signal return an empty environment; with the verdict reused, the later
/// signals (the KILL and CONT after the STOP) still reach the tagged member.
#[test]
fn the_verdict_is_taken_once_per_identity() {
    let d = scratch("once");
    let (code, t, u) = two_members(&d, &[("SHEEPDOG_TEST_LATCH", "1"), ("SHEEPDOG_TEST_ENV_EMPTY_AFTER_FIRST", "1")], false, None);
    let lines = sink_lines(&d.join("sink"));
    let tagged_alive = common::alive(t);
    common::send(t.0, t.1, libc::SIGKILL);
    common::send(u.0, u.1, libc::SIGKILL);
    assert!(!tagged_alive, "the tagged member dies");
    assert!(lines.iter().all(|(_, p, _)| *p == u.0), "a later signal to the tagged member was withheld: {lines:?}");
    assert_eq!(code, Some(125));
    let _ = std::fs::remove_dir_all(&d);
}

/// PHASE2.md §0.4: a fixture script that calls `sheepdog` by name gets this debug build (the
/// test PATH), never an installed release: the nested run writes the debug start note.
#[test]
fn a_nested_sheepdog_found_by_name_is_this_debug_build() {
    let d = scratch("nested");
    let t = d.join("trace");
    let st = Command::new("/bin/sh")
        .args(["-c", "sheepdog run -- true"])
        .env("SHEEPDOG_TEST_TRACE", &t)
        .status()
        .unwrap();
    assert!(st.success(), "{st:?}");
    let notes = std::fs::read_to_string(&t).unwrap_or_default();
    assert!(notes.lines().any(|l| l.starts_with("start debug ")), "{notes:?}");
    let _ = std::fs::remove_dir_all(&d);
}
