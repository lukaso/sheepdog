//! Cells 3 and 7-lite (PLAN.md §6): escapees that must not survive `sheepdog run`.
//!
//! The checker counts a survivor if EITHER a recorded `<pid> <identity>` is still that same
//! process (sheepdog::ident), OR a live process carries the iteration's unique argv marker.
//! The identity path is required (control: an escapee without a marker); the marker path also
//! catches processes the fixture could not record. Cleanup kills only by those two facts,
//! re-checked immediately before each signal, and runs from a drop guard, so it also runs when
//! an assertion panics.
//!
//! Controls that must come out the other way: the fixture without sheepdog, the round-2 design
//! (macOS `--mode root-disclaim`) and no subreaper (Linux `--mode none`).
//!
//! SD_STRESS_N sets the iteration count of the stress cells (default 200; the gate is 10000).

use sheepdog::ident::same;
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
fn stress_n() -> usize {
    std::env::var("SD_STRESS_N").ok().and_then(|v| v.parse().ok()).unwrap_or(200)
}

/// One iteration's facts: a unique marker and a record file. Dropping it kills survivors.
struct Iteration {
    marker: String,
    rec: PathBuf,
}

impl Iteration {
    fn new() -> Self {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let marker = format!("29.{}{:06}", std::process::id(), n);
        let dir = std::env::temp_dir().join(format!("sd-cell3-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let rec = dir.join(&marker);
        let _ = std::fs::remove_file(&rec);
        Iteration { marker, rec }
    }
    fn rec(&self) -> &str {
        self.rec.to_str().unwrap()
    }
    fn recorded(&self) -> Vec<(i32, u64)> {
        std::fs::read_to_string(&self.rec)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| {
                let mut w = l.split_whitespace();
                Some((w.next()?.parse().ok()?, w.next()?.parse().ok()?))
            })
            .collect()
    }
    /// Live pids carrying the marker. Panics if `ps` cannot run or fails: a checker that
    /// looked at nothing must never report "no survivors" (review round 3, F1).
    fn marked(&self) -> Vec<i32> {
        with_marker(&self.marker).expect("the survivor checker could not run ps")
    }
    /// For Drop only: a panic there would abort the suite, so failures mean "nothing found".
    fn marked_quiet(&self) -> Vec<i32> {
        with_marker(&self.marker).unwrap_or_default()
    }
    /// Survivors by identity, and in total by identity or marker (counted once).
    fn survivors(&self) -> (usize, usize) {
        let by_id: Vec<i32> =
            self.recorded().into_iter().filter(|&(p, id)| same(p, id)).map(|(p, _)| p).collect();
        let mut all = by_id.clone();
        for p in self.marked() {
            if !all.contains(&p) {
                all.push(p);
            }
        }
        (by_id.len(), all.len())
    }
}

impl Drop for Iteration {
    fn drop(&mut self) {
        for (p, id) in self.recorded() {
            if same(p, id) {
                unsafe { libc::kill(p, libc::SIGKILL) };
            }
        }
        for p in self.marked_quiet() {
            if self.marked_quiet().contains(&p) {
                unsafe { libc::kill(p, libc::SIGKILL) };
            }
        }
        let _ = std::fs::remove_file(&self.rec);
    }
}

/// Live pids whose argv contains `marker` as a whole word; Err if `ps` is missing or fails.
fn with_marker(marker: &str) -> Result<Vec<i32>, String> {
    let out = Command::new("ps").args(["-Ao", "pid=,args="]).output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("ps exited {:?}", out.status.code()));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| l.split_whitespace().any(|w| w == marker))
        .filter_map(|l| l.split_whitespace().next()?.parse().ok())
        .collect())
}

#[derive(Default)]
struct Outcome {
    survivors: usize,
    by_identity: usize,
    recorded: usize,
}

/// Run `n` iterations: launch, let late escapees appear, count, clean up.
fn iterate(n: usize, launch: impl Fn(&Iteration)) -> Outcome {
    let mut o = Outcome::default();
    for _ in 0..n {
        let it = Iteration::new();
        launch(&it);
        std::thread::sleep(Duration::from_millis(100));
        let (by_id, all) = it.survivors();
        o.by_identity += by_id;
        o.survivors += all;
        o.recorded += it.recorded().len();
    }
    o
}

fn run_sheepdog(mode: Option<&str>, fixture_args: &[&str]) {
    let mut c = Command::new(sheepdog());
    c.arg("run");
    if let Some(mode) = mode {
        c.args(["--mode", mode]);
    }
    c.arg("--").arg(fixture()).args(fixture_args);
    let st = c.status().unwrap();
    assert_eq!(st.code(), Some(0), "sheepdog did not exit 0 (deadline or error): {st:?}");
}

fn escape(mode: Option<&'static str>, shape: &'static str) -> impl Fn(&Iteration) {
    move |it: &Iteration| run_sheepdog(mode, &[shape, &it.marker, it.rec()])
}

// ---- controls -------------------------------------------------------------------------

/// The marker checker must find a live process that carries a known marker (review round 3,
/// F1: without `ps` the checker used to report "no survivors" having looked at nothing).
#[test]
fn control_the_marker_checker_finds_a_marked_process() {
    let it = Iteration::new();
    let mut c = Command::new("/bin/sleep").arg(&it.marker).spawn().unwrap();
    std::thread::sleep(Duration::from_millis(50));
    let found = it.marked();
    let _ = c.kill();
    let _ = c.wait();
    assert_eq!(found, vec![c.id() as i32]);
}

#[test]
fn control_without_sheepdog_every_escapee_survives() {
    let o = iterate(5, |it| {
        Command::new(fixture()).args(["escape", &it.marker, it.rec()]).status().unwrap();
    });
    assert_eq!(o.recorded, 5);
    assert_eq!(o.survivors, 5);
    assert_eq!(o.by_identity, 5);
}

#[test]
fn control_the_identity_checker_finds_an_escapee_without_a_marker() {
    let o = iterate(5, |it| {
        Command::new(fixture()).args(["escape-nomarker", it.rec()]).status().unwrap();
    });
    assert_eq!(o.recorded, 5);
    assert_eq!(o.by_identity, 5, "only the identity check can find these escapees");
}

#[cfg(target_os = "macos")]
#[test]
fn control_root_disclaim_design_loses_escapees() {
    let o = iterate(20, escape(Some("root-disclaim"), "escape"));
    assert!(o.recorded >= 19);
    assert!(o.survivors > 0, "the round-2 design must lose escapees; it lost none");
}

#[cfg(target_os = "linux")]
#[test]
fn control_without_subreaper_loses_escapees() {
    let o = iterate(20, escape(Some("none"), "escape"));
    assert!(o.recorded >= 19);
    assert!(o.survivors > 0, "without the subreaper, escapees must be lost; none were");
}

// ---- cells ----------------------------------------------------------------------------

#[test]
fn cell3_root_waits_no_survivor() {
    let n = stress_n();
    let o = iterate(n, escape(None, "escape"));
    // non-vacuity: the root records every G it created (it is never killed before it exits)
    assert!(o.recorded * 100 >= n * 99, "only {} of {n} escapees were created", o.recorded);
    assert_eq!(o.survivors, 0, "{} of {n} escapees survived sheepdog", o.survivors);
}

#[test]
fn cell3_root_exits_at_once_no_survivor() {
    let n = stress_n();
    let o = iterate(n, escape(None, "escape-fast"));
    // C records and can be killed first, so this is a lower bound on creation
    assert!(o.recorded * 2 >= n, "only {} of {n} escapees were recorded", o.recorded);
    assert_eq!(o.survivors, 0, "{} of {n} escapees survived sheepdog", o.survivors);
}

#[test]
fn cell7_a_breeding_escapee_leaves_no_survivor() {
    let n = (stress_n() / 10).max(20);
    let o = iterate(n, |it| run_sheepdog(None, &["breed", &it.marker, it.rec(), "300"]));
    assert!(o.recorded >= n, "the breeder did not start ({} records)", o.recorded);
    assert_eq!(o.survivors, 0, "{} processes of {n} breeding trees survived", o.survivors);
}

#[test]
fn cell7_a_fork_chain_leaves_no_survivor() {
    let n = (stress_n() / 10).max(20);
    let o = iterate(n, |it| run_sheepdog(None, &["chain", &it.marker, it.rec(), "4000"]));
    assert!(o.recorded >= n, "the chain did not start ({} records)", o.recorded);
    assert_eq!(o.survivors, 0, "{} chains of {n} survived", o.survivors);
}

/// Cell 7 (fast chain): each generation forks its successor at once and exits, so every
/// process lives microseconds while one scan takes milliseconds. A scan-only emptiness check
/// sees only dead or not-yet-listed pids and declares a live tree clean (phase-0 fix review,
/// P1-A). SD_CHAIN_N sets the generation count (default 20000).
#[test]
fn cell7_a_fast_fork_chain_leaves_no_survivor() {
    let gens = std::env::var("SD_CHAIN_N").unwrap_or_else(|_| "20000".into());
    let o = iterate(30, |it| run_sheepdog(None, &["chain", &it.marker, it.rec(), &gens, "0"]));
    assert!(o.recorded >= 30, "the chain did not start ({} records)", o.recorded);
    assert_eq!(o.survivors, 0, "{} fast chains of 30 survived", o.survivors);
}

/// Cells 24-lite and 20 (PLAN.md §3.2 sticky membership, §3.3 step 6 deadline): a member
/// sheepdog saw but could not kill must be reported at the deadline (exit 125, its pid on
/// stderr), never declared clean. Debug-only seams: SHEEPDOG_TEST_FORGET=1 hides every member
/// after its first sighting (it "lost its fact"), SHEEPDOG_TEST_NOKILL=1 makes every signal
/// fail (as EPERM would after a setuid exec), SHEEPDOG_TEST_DEADLINE_MS shortens the deadline.
/// A loop that is not sticky forgets the member and exits 0 while it is alive.
#[test]
fn cell24_a_member_that_cannot_be_killed_is_reported_not_declared_clean() {
    for _ in 0..5 {
        let it = Iteration::new();
        // stderr goes to a file: the unkillable escapee inherits it, and waiting for a pipe's
        // EOF would wait for the escapee's 29 s sleep, not for sheepdog
        let errf = it.rec.with_extension("stderr");
        let st = Command::new(sheepdog())
            .args(["run", "--", fixture(), "escape", &it.marker, it.rec()])
            .env("SHEEPDOG_TEST_FORGET", "1")
            .env("SHEEPDOG_TEST_NOKILL", "1")
            .env("SHEEPDOG_TEST_DEADLINE_MS", "300")
            .stdout(std::process::Stdio::null())
            .stderr(std::fs::File::create(&errf).unwrap())
            .status()
            .unwrap();
        let err = std::fs::read_to_string(&errf).unwrap_or_default();
        let _ = std::fs::remove_file(&errf);
        let g = it.recorded().first().copied().expect("the escapee was recorded").0;
        assert_eq!(st.code(), Some(125), "sheepdog declared clean with a member alive; stderr: {err}");
        assert!(err.contains("NOT clean") && err.contains(&g.to_string()), "the survivor {g} was not reported: {err}");
        drop(it); // cleanup kills the escapee
    }
}
