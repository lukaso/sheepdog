//! Cell 3 (PLAN.md §6): the root starts a setsid double-fork escapee and exits at once.
//! After `sheepdog run` returns, no escapee may be alive.
//!
//! The checker finds survivors by their unique argv marker (never by name), and kills only
//! processes whose argv still carries that marker. Two controls must come out the other way:
//! the fixture without sheepdog, and the round-2 design (the root disclaims, `--mode
//! root-disclaim`), which loses the escapee once the root exits.
//!
//! SD_STRESS_N sets the iteration count of the main cell (default 200; the phase-0 gate is 10000).

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

static SEQ: AtomicUsize = AtomicUsize::new(0);

fn fixture() -> &'static str {
    env!("CARGO_BIN_EXE_sd-fixture")
}
fn sheepdog() -> &'static str {
    env!("CARGO_BIN_EXE_sheepdog")
}

/// A number unique to this test process and iteration; `/bin/sleep` accepts it as seconds.
fn marker() -> String {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    format!("29.{}{:06}", std::process::id(), n)
}

fn record_file(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sd-cell3-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(tag)
}

/// Pids of live processes whose argv contains `marker`.
fn alive_with(marker: &str) -> Vec<i32> {
    let out = Command::new("ps").args(["-Ao", "pid=,args="]).output().unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| l.split_whitespace().any(|w| w == marker))
        .filter_map(|l| l.split_whitespace().next()?.parse().ok())
        .collect()
}

/// Kill survivors, re-checking the marker immediately before each signal.
fn cleanup(marker: &str) {
    for pid in alive_with(marker) {
        if alive_with(marker).contains(&pid) {
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
    }
}

struct Outcome {
    survivors: usize,
    spawned: usize,
}

fn iterate(n: usize, launch: impl Fn(&str, &PathBuf)) -> Outcome {
    let mut o = Outcome { survivors: 0, spawned: 0 };
    for _ in 0..n {
        let m = marker();
        let rec = record_file(&m);
        let _ = std::fs::remove_file(&rec);
        launch(&m, &rec);
        std::thread::sleep(Duration::from_millis(100)); // a late escapee has time to appear
        let alive = alive_with(&m);
        o.survivors += alive.len();
        o.spawned += std::fs::read_to_string(&rec).map(|s| s.lines().count()).unwrap_or(0);
        cleanup(&m);
        let _ = std::fs::remove_file(&rec);
    }
    o
}

fn under_sheepdog(mode: Option<&'static str>) -> impl Fn(&str, &PathBuf) {
    move |m: &str, rec: &PathBuf| {
        let mut c = Command::new(sheepdog());
        c.arg("run");
        if let Some(mode) = mode {
            c.args(["--mode", mode]);
        }
        c.args(["--", fixture(), "escape", m, rec.to_str().unwrap()]);
        let st = c.status().unwrap();
        assert!(st.code().is_some(), "sheepdog itself died from a signal: {st:?}");
    }
}

#[test]
fn control_without_sheepdog_the_escapee_survives() {
    let o = iterate(5, |m, rec| {
        Command::new(fixture()).args(["escape", m, rec.to_str().unwrap()]).status().unwrap();
    });
    assert_eq!(o.spawned, 5, "the fixture did not spawn its escapees");
    assert_eq!(o.survivors, 5, "without sheepdog every escapee must survive");
}

#[cfg(target_os = "macos")]
#[test]
fn control_root_disclaim_design_loses_escapees() {
    let o = iterate(20, under_sheepdog(Some("root-disclaim")));
    assert!(o.spawned >= 18, "the fixture did not spawn its escapees ({})", o.spawned);
    assert!(o.survivors > 0, "the round-2 design must lose escapees; it lost none");
}

#[cfg(target_os = "linux")]
#[test]
fn control_without_subreaper_loses_escapees() {
    let o = iterate(20, under_sheepdog(Some("none")));
    assert!(o.spawned >= 18, "the fixture did not spawn its escapees ({})", o.spawned);
    assert!(o.survivors > 0, "without the subreaper, escapees must be lost; none were");
}

#[test]
fn cell3_default_mode_leaves_no_survivor() {
    let n: usize = std::env::var("SD_STRESS_N").ok().and_then(|v| v.parse().ok()).unwrap_or(200);
    let o = iterate(n, under_sheepdog(None));
    // non-vacuity: the escapees really were created (C records each G right after forking it)
    assert!(o.spawned * 100 >= n * 99, "only {} of {n} escapees were created", o.spawned);
    assert_eq!(o.survivors, 0, "{} of {n} escapees survived sheepdog", o.survivors);
}
