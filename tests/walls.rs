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
