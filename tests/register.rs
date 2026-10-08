//! Phase-2 P5: registration of nested runs (PLAN.md §3.2, PHASE2.md decision 13 and D5).
//!
//! Cell 15: an inner `sheepr run` started through a fast double fork (and through a Node and a
//! Python launcher on macOS) is killed with SIGKILL; then the outer is ended; the inner job's
//! escapee must not survive. Three levels too: O -> A -> B, with A and B killed first. On macOS
//! only registration can tie the inner job to the outer (the double fork hides the inner from
//! the outer's scan); on Linux the subreaper chain holds it (no registration). The other cells
//! are macOS-only: the socket path fallback (cell 25), the chain cap, malformed and changed
//! peers, silent clients, the spoof, a stolen nonce, and the wall control.
//!
//! Every process a kill may aim at is built here from the fixture binary, tagged by the runner
//! (the wall control's registrant is untagged on purpose and must be withheld).

mod common;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
#[cfg(target_os = "macos")]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
static SEQ: AtomicUsize = AtomicUsize::new(0);

fn sheepr() -> &'static str {
    common::test_env();
    env!("CARGO_BIN_EXE_sheepr")
}
fn fixture() -> &'static str {
    common::test_env();
    env!("CARGO_BIN_EXE_sr-fixture")
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("sr-reg-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A marker unique to one iteration (`/bin/sleep <marker>` ends by itself within 29 s).
#[cfg(target_os = "macos")]
fn marker() -> String {
    format!("29.{:04}{:03}", std::process::id() % 10_000, SEQ.fetch_add(1, Ordering::SeqCst) % 1000)
}

fn records(r: &Path) -> Vec<(i32, u64)> {
    std::fs::read_to_string(r)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            Some((w.next()?.parse().ok()?, w.next()?.parse().ok()?))
        })
        .collect()
}

fn wait_until(secs: u64, mut f: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    f()
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_default()
}

fn finished(c: &mut Child, secs: u64) -> Option<std::process::ExitStatus> {
    let end = Instant::now() + Duration::from_secs(secs);
    loop {
        if let Some(st) = c.try_wait().unwrap() {
            return Some(st);
        }
        if Instant::now() > end {
            common::send_child(c, libc::SIGKILL);
            let _ = c.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Kill every recorded process of these record files (and their `.g` / `.root` forms).
fn cleanup(recs: &[&Path]) {
    for r in recs {
        for suffix in ["", ".g", ".root", ".kid", ".hold"] {
            for p in records(&PathBuf::from(format!("{}{suffix}", r.display()))) {
                common::send(p.0, p.1, libc::SIGKILL);
            }
        }
    }
}

/// One nesting trial: O runs `launch` (which starts the inner run); the inner run's job is an
/// escapee (`escapee-and-wait`, recorded in `esc`). `sups` are the files that hold each inner
/// supervisor's pid, killed with SIGKILL (in order) before O gets TERM, so only O can end the
/// escapee. True if the escapee is gone within 5 s of O's end.
fn trial(d: &Path, esc: &Path, sups: &[PathBuf], launch: &[String]) -> Result<bool, String> {
    let trace = PathBuf::from(format!("{}.trace", esc.display()));
    let mut o = Command::new(sheepr()).args(["run", "--"]).args(launch).env("SHEEPR_TEST_TRACE", &trace).stdin(Stdio::null()).spawn().unwrap();
    let ok = wait_until(20, || records(esc).len() == 1 && sups.iter().all(|s| !records(s).is_empty()));
    let g = records(esc).first().copied();
    // past the stated loss window (PLAN.md §3.2: one 250 ms scan): a child born to a registered
    // inner is tied to the outer by a scan while the inner lives; the inner is killed only after
    std::thread::sleep(Duration::from_millis(800));
    let sup_ids: Vec<(i32, u64)> = sups.iter().filter_map(|s| records(s).first().copied()).collect();
    for s in &sup_ids {
        common::send(s.0, s.1, libc::SIGKILL);
    }
    // the inner supervisors are gone before O's TERM (so only O can reach the escapee)
    let dead = sup_ids.iter().all(|&s| wait_until(5, || !common::alive(s)));
    common::send_child(&mut o, libc::SIGTERM);
    let ended = finished(&mut o, 20).is_some();
    let gone = g.is_some_and(|g| wait_until(5, || !common::alive(g)));
    cleanup(&[esc]);
    cleanup(&sups.iter().map(PathBuf::as_path).collect::<Vec<_>>());
    let _ = d;
    if !ok || g.is_none() {
        return Err(format!("the trial did not start (escapee {g:?}, supervisors {sup_ids:?})"));
    }
    if !dead || !ended {
        return Err(format!("setup: inner supervisors dead {dead}, outer ended {ended}"));
    }
    if !gone {
        eprintln!("escapee {g:?} survived; trace:\n{}", read(&trace));
    }
    Ok(gone)
}

/// The outer run of a cell, ended when the cell ends (a panic too): a live outer holds the test's
/// output pipe, and cargo would wait for it. TERM first (and CONT, for an outer the cell stopped),
/// so the outer ends its own job, and kills a root it has not resumed yet: a SIGKILL alone left
/// such a root suspended for ever (issue #11). SIGKILL after 5 s.
struct Outer(Child);
impl Drop for Outer {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            common::send_child(&mut self.0, libc::SIGTERM);
            common::send_child(&mut self.0, libc::SIGCONT);
            let c = &mut self.0;
            if !wait_until(5, || c.try_wait().ok().flatten().is_some()) {
                common::send_child(&mut self.0, libc::SIGKILL);
                let _ = self.0.wait();
            }
        }
    }
}

/// Record files whose processes are SIGKILLed (by identity) when the cell ends, a panic too.
struct Recs(Vec<PathBuf>);
impl Drop for Recs {
    fn drop(&mut self) {
        cleanup(&self.0.iter().map(PathBuf::as_path).collect::<Vec<_>>());
    }
}

/// A marker whose processes are SIGKILLed when the cell ends, a panic too.
#[cfg(target_os = "macos")]
struct Marked(String);
#[cfg(target_os = "macos")]
impl Drop for Marked {
    fn drop(&mut self) {
        common::kill_marked(&[&self.0]);
    }
}

fn fx() -> String {
    fixture().to_string()
}
fn sd() -> String {
    sheepr().to_string()
}

/// Cell 15, two levels: O -> (fast double fork) -> inner run; the inner supervisor is SIGKILLed,
/// then O is ended: the inner job's escapee does not survive. 10 trials.
#[test]
fn cell15_an_inner_run_behind_a_double_fork_is_held_by_the_outer() {
    let d = scratch("c15");
    let mut lost = 0;
    for i in 0..10 {
        let (esc, sup) = (d.join(format!("esc{i}")), d.join(format!("sup{i}")));
        let launch = vec![fx(), "dfork-exec".into(), sup.display().to_string(), sd(), "run".into(), "--".into(), fx(), "escapee-and-wait".into(), esc.display().to_string()];
        match trial(&d, &esc, &[sup], &launch) {
            Ok(true) => {}
            Ok(false) => lost += 1,
            Err(e) => panic!("trial {i}: {e}"),
        }
    }
    assert_eq!(lost, 0, "{lost} of 10 inner escapees survived their outer");
    let _ = std::fs::remove_dir_all(&d);
}

/// Cell 15, three levels: O -> A -> B, every link a fast double fork; A and B are SIGKILLed
/// (A first), then O is ended: B's escapee does not survive. 10 trials.
#[test]
fn cell15_three_levels_leave_the_innermost_job_reachable() {
    let d = scratch("c15x3");
    let mut lost = 0;
    for i in 0..10 {
        let (esc, a, b) = (d.join(format!("esc{i}")), d.join(format!("a{i}")), d.join(format!("b{i}")));
        let launch = vec![
            fx(), "dfork-exec".into(), a.display().to_string(), sd(), "run".into(), "--".into(),
            fx(), "dfork-exec".into(), b.display().to_string(), sd(), "run".into(), "--".into(),
            fx(), "escapee-and-wait".into(), esc.display().to_string(),
        ];
        match trial(&d, &esc, &[a, b], &launch) {
            Ok(true) => {}
            Ok(false) => lost += 1,
            Err(e) => panic!("trial {i}: {e}"),
        }
    }
    assert_eq!(lost, 0, "{lost} of 10 innermost escapees survived");
    let _ = std::fs::remove_dir_all(&d);
}

/// Cell 15 through a Homebrew `node` and a Homebrew `python3` launcher (both close inherited
/// fds; the env path survives). The launcher writes the inner supervisor's pid and exits.
#[cfg(target_os = "macos")]
#[test]
fn cell15_through_node_and_python_launchers() {
    let d = scratch("c15l");
    let mut ran = 0;
    for (name, prog) in [("node", "/opt/homebrew/bin/node"), ("python3", "/opt/homebrew/bin/python3")] {
        if !Path::new(prog).exists() {
            eprintln!("{name}: not installed, skipped");
            continue;
        }
        ran += 1;
        for i in 0..3 {
            let (esc, sup, pidf) = (d.join(format!("{name}-esc{i}")), d.join(format!("{name}-sup{i}")), d.join(format!("{name}-pid{i}")));
            let inner = [sd(), "run".into(), "--".into(), fx(), "escapee-and-wait".into(), esc.display().to_string()];
            let list = inner.iter().map(|s| format!("{s:?}")).collect::<Vec<_>>().join(",");
            let code = if name == "node" {
                format!("const a=[{list}];const c=require('child_process').spawn(a[0],a.slice(1),{{detached:true,stdio:'ignore'}});require('fs').writeFileSync({:?},String(c.pid));c.unref()", pidf.display().to_string())
            } else {
                format!("import subprocess;p=subprocess.Popen([{list}],start_new_session=True,stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL);open({:?},'w').write(str(p.pid))", pidf.display().to_string())
            };
            let flag = if name == "node" { "-e" } else { "-c" };
            // the launcher exits; O's root then waits (as this fixture, so its env can be read)
            let hold = d.join(format!("{name}-hold{i}"));
            let sh = format!("{prog} {flag} '{}' && exec {} sigcount {}", code.replace('\'', "'\\''"), fx(), hold.display());
            let launch = vec!["/bin/sh".to_string(), "-c".into(), sh];
            // the supervisor's record, written from the launcher's pid file once it exists
            let pidf2 = pidf.clone();
            let sup2 = sup.clone();
            let t = std::thread::spawn(move || {
                if wait_until(20, || !read(&pidf2).trim().is_empty()) {
                    let p: i32 = read(&pidf2).trim().parse().unwrap_or(0);
                    if let Some(id) = sheepr::ident::identity(p) {
                        std::fs::write(&sup2, format!("{p} {id}\n")).unwrap();
                    }
                }
            });
            let r = trial(&d, &esc, &[sup], &launch);
            let _ = t.join();
            cleanup(&[&hold]);
            match r {
                Ok(true) => {}
                Ok(false) => panic!("{name} trial {i}: the inner escapee survived its outer"),
                Err(e) => panic!("{name} trial {i}: {e}"),
            }
        }
    }
    assert!(ran > 0, "neither launcher is installed");
    let _ = std::fs::remove_dir_all(&d);
}

/// macOS: run O with `root` (a shell script, "$FX" and "$SD" set), trace to `trace`, stderr to
/// `err`, extra env; returns O's exit status (30 s bound).
#[cfg(target_os = "macos")]
fn outer(d: &Path, root: &str, env: &[(&str, &str)]) -> (Option<i32>, String, String) {
    let (trace, err) = (d.join("trace"), d.join("err"));
    let mut c = Command::new(sheepr());
    c.args(["run", "--", "/bin/sh", "-c", root])
        .env("FX", fixture())
        .env("SD", sheepr())
        .env("SHEEPR_TEST_TRACE", &trace)
        .stdin(Stdio::null())
        .stderr(std::fs::File::create(&err).unwrap());
    for (k, v) in env {
        c.env(k, v);
    }
    let mut ch = c.spawn().unwrap();
    let code = finished(&mut ch, 30).and_then(|s| s.code());
    (code, read(&trace), read(&err))
}

/// Cell 25: a `TMPDIR` whose socket path would pass 100 bytes: the outer listens under /tmp, and
/// the inner run gets its ack (the inner inherits the same long `TMPDIR`).
#[cfg(target_os = "macos")]
#[test]
fn cell25_a_long_tmpdir_still_gets_the_ack() {
    let d = scratch("c25");
    let long = d.join("t".repeat(100 - d.as_os_str().len().min(90)).as_str()).join("u".repeat(20));
    std::fs::create_dir_all(&long).unwrap();
    assert!(long.join("sr-XXXXXXXX/s").as_os_str().len() > 100, "the cell's TMPDIR is not long enough");
    let (code, trace, _) = outer(&d, r#""$SD" run -- /usr/bin/true"#, &[("TMPDIR", long.to_str().unwrap())]);
    let registered = trace.lines().any(|l| l.starts_with("registered "));
    let refused = trace.lines().filter(|l| l.starts_with("unregistered ")).collect::<Vec<_>>();
    assert_eq!(code, Some(0));
    assert!(registered && refused.is_empty(), "no ack for the inner run: {trace}");
    let _ = std::fs::remove_dir_all(&d);
}

/// The chain holds at most 16 entries: given 15, the root sees 16 (the outer's own last);
/// given 16, the root sees the same 16, and the outer says it did not append.
#[cfg(target_os = "macos")]
#[test]
fn the_chain_holds_sixteen_entries() {
    let d = scratch("cap");
    let fake = |n: usize| (0..n).map(|i| format!("{:032x} /nonexistent/sr-{i}/s", i + 1)).collect::<Vec<_>>().join("\n");
    for (given, want) in [(15, 16), (16, 16)] {
        let out = d.join(format!("env{given}"));
        let (code, trace, _) = outer(&d, &format!(r#""$FX" print-env SHEEPR_OUTER "{}""#, out.display()), &[("SHEEPR_OUTER", &fake(given))]);
        let (seen, bad) = sheepr::regwire::parse(&read(&out));
        assert_eq!(code, Some(0));
        assert_eq!((seen.len(), bad), (want, 0), "given {given}");
        let (mine, _) = sheepr::regwire::parse(&fake(given));
        assert_eq!(seen[..given], mine[..], "the given entries are passed on unchanged");
        let full = trace.lines().any(|l| l == "chain full");
        assert_eq!(full, given == 16, "given {given}: {trace}");
    }
    let _ = std::fs::remove_dir_all(&d);
}

/// A malformed record is refused, and so is a well-formed one whose sender's process version
/// changed during the membership check (the seam); a well-formed one from a member is the
/// control: acked.
#[cfg(target_os = "macos")]
#[test]
fn a_malformed_or_changed_registration_is_refused() {
    let d = scratch("bad");
    let (bad, good) = (d.join("bad"), d.join("good"));
    let root = format!(r#""$FX" register env 0 self bad "{}"; "$FX" register env 0 self good "{}"; true"#, bad.display(), good.display());
    let (code, _, _) = outer(&d, &root, &[]);
    assert_eq!(code, Some(0));
    assert_eq!((read(&bad).as_str(), read(&good).as_str()), ("refused", "ack"));
    let changed = d.join("changed");
    let root = format!(r#""$FX" register env 0 self good "{}"; true"#, changed.display());
    let (code, _, _) = outer(&d, &root, &[("SHEEPR_TEST_REG_PIDVERSION_CHANGE", "1")]);
    assert_eq!(code, Some(0));
    assert_eq!(read(&changed), "refused");
    let _ = std::fs::remove_dir_all(&d);
}

/// 20 clients that connect and send nothing do not stall the outer: `--timeout 1s` still ends
/// the job on time (exit 124 within 2.5 s).
#[cfg(target_os = "macos")]
#[test]
fn silent_clients_do_not_stall_the_timeout() {
    let d = scratch("silent");
    let ready = d.join("ready");
    let _g = Recs(vec![ready.clone()]);
    let t0 = Instant::now();
    let mut c = Command::new(sheepr())
        .args(["run", "--timeout", "1s", "--grace", "0", "--"])
        .args([fixture(), "silent", "env", "20", ready.to_str().unwrap()])
        .stdin(Stdio::null())
        .spawn()
        .unwrap();
    let st = finished(&mut c, 30);
    let took = t0.elapsed();
    assert_eq!(read(&ready), "20", "the clients did not all connect");
    assert_eq!(st.and_then(|s| s.code()), Some(124));
    assert!(took < Duration::from_millis(2500), "the timeout fired after {took:?}");
    let _ = std::fs::remove_dir_all(&d);
}

/// The spoof: a member registers claiming a decoy's pid (a process outside the job that is
/// responsible for itself, with a kid responsible to it, as a terminal app is). The outer takes the
/// sender from the kernel and refuses the claim: the decoy's kid gets no signal.
#[cfg(target_os = "macos")]
#[test]
fn a_registration_for_another_process_is_refused() {
    let d = scratch("spoof");
    let dr = d.join("decoy");
    let head = format!("{}.head", dr.display());
    let _g = Recs(vec![dr.clone(), PathBuf::from(&head)]);
    let mut decoy = Command::new(fixture()).args(["disclaim-exec", dr.to_str().unwrap(), fixture(), "sigcount", &head]).spawn().unwrap();
    let kidr = PathBuf::from(format!("{}.kid", dr.display()));
    assert!(wait_until(10, || !records(&dr).is_empty() && !records(&kidr).is_empty()), "the decoy did not start");
    let (dp, kid) = (records(&dr)[0], records(&kidr)[0]);
    let out = d.join("out");
    let (code, _, _) = outer(&d, &format!(r#""$FX" register env 0 {} good "{}"; true"#, dp.0, out.display()), &[]);
    let (alive, sigs) = (common::alive(kid), read(&PathBuf::from(format!("{}.kid.sig", dr.display()))).lines().count());
    common::send_child(&mut decoy, libc::SIGKILL);
    let _ = decoy.wait();
    common::send(kid.0, kid.1, libc::SIGKILL);
    // the harm first: the decoy's kid got no signal (then the answer, then the run)
    assert!(alive && sigs == 0, "the decoy's kid got {sigs} signal(s), alive {alive}");
    assert_eq!(read(&out), "refused");
    assert_eq!(code, Some(0));
    let _ = std::fs::remove_dir_all(&d);
}

/// A process outside the job that has the socket path and nonce (read from the root's env)
/// registers: it is not a member, so it is refused. It is responsible for itself with a kid
/// responsible to it; the kid survives the outer's end.
#[cfg(target_os = "macos")]
#[test]
fn a_non_member_with_the_nonce_is_refused() {
    let d = scratch("stolen");
    let chain = d.join("chain");
    let m0 = marker();
    let _m = Marked(m0.clone());
    let mut o = Outer(
        Command::new(sheepr())
            .args(["run", "--", fixture(), "print-env", "SHEEPR_OUTER", chain.to_str().unwrap(), &m0])
            .stdin(Stdio::null())
            .spawn()
            .unwrap(),
    );
    assert!(wait_until(10, || !read(&chain).is_empty()), "the root did not write its chain");
    let out = d.join("out");
    let or = d.join("outsider");
    let _g = Recs(vec![or.clone()]);
    let kidr = PathBuf::from(format!("{}.kid", or.display()));
    let mut outsider = Command::new(fixture())
        .args(["disclaim-exec", or.to_str().unwrap(), fixture(), "register", chain.to_str().unwrap(), "0", "self", "good", out.to_str().unwrap(), &marker()])
        .spawn()
        .unwrap();
    assert!(wait_until(10, || !read(&out).is_empty() && !records(&kidr).is_empty()), "the outsider did not answer");
    let kid = records(&kidr)[0];
    common::send_child(&mut o.0, libc::SIGTERM);
    let ended = finished(&mut o.0, 20).is_some();
    let (alive, sigs) = (common::alive(kid), read(&PathBuf::from(format!("{}.kid.sig", or.display()))).lines().count());
    common::send_child(&mut outsider, libc::SIGKILL);
    let _ = outsider.wait();
    common::send(kid.0, kid.1, libc::SIGKILL);
    // the harm first: the outsider's kid got no signal (then the answer, then the run)
    assert!(alive && sigs == 0, "the outsider's kid got {sigs} signal(s), alive {alive}");
    assert_eq!(read(&out), "refused");
    assert!(ended);
    let _ = std::fs::remove_dir_all(&d);
}

/// The wall control for registration (PHASE2.md §0.1): an untagged member registers (the root
/// itself, after `env -i`); the accepted registration turns the latch on, so the job's kill
/// withholds every signal to it (a `withheld` line names it). No state dir: no other phase-2
/// source runs.
#[cfg(target_os = "macos")]
#[test]
fn registration_turns_the_latch_on() {
    let d = scratch("latch");
    let (chain, out, sink) = (d.join("chain"), d.join("out"), d.join("sink"));
    let m = marker();
    let _m = Marked(m.clone());
    let root = format!(r#""$FX" print-env SHEEPR_OUTER "{}"; exec /usr/bin/env -i "$FX" register "{}" 0 self good "{}" {m}"#, chain.display(), chain.display(), out.display());
    let mut c = Command::new(sheepr());
    c.args(["run", "--", "/bin/sh", "-c", &root]).env("FX", fixture()).env("SHEEPR_TEST_DEADLINE_MS", "500").env_remove("SHEEPR_TEST_STATE").stdin(Stdio::null());
    common::cell_sink(&mut c, &sink);
    let mut o = Outer(c.spawn().unwrap());
    assert!(wait_until(10, || !read(&out).is_empty()), "the registrant did not answer");
    // the registrant is the root; after its answer it runs `/bin/sleep <marker>` (the same pid)
    // (the marker is also in O's and the shell's argv: only the sleep itself counts)
    let sleeping = |w: &[&str]| w.get(1) == Some(&"/bin/sleep");
    assert!(wait_until(10, || common::scan(&m, sleeping).is_ok_and(|v| v.len() == 1)), "the registrant is not sleeping");
    let reg = common::scan(&m, sleeping).unwrap_or_default();
    common::send_child(&mut o.0, libc::SIGTERM);
    let _ = finished(&mut o.0, 20);
    let lines = read(&sink);
    let alive: Vec<(i32, u64)> = reg.iter().copied().filter(|&p| common::alive(p)).collect();
    for p in &reg {
        common::send(p.0, p.1, libc::SIGKILL);
    }
    assert_eq!(read(&out), "ack");
    assert_eq!(reg.len(), 1, "{reg:?}");
    assert_eq!(alive.len(), 1, "the untagged registrant was signalled");
    assert!(lines.lines().any(|l| l.starts_with("withheld ") && l.split_whitespace().nth(1) == Some(&reg[0].0.to_string())), "{lines:?}");
    let _ = std::fs::remove_dir_all(&d);
}

/// An inner run that starts after the process responsible for it has died (its outer sheepr was
/// SIGKILLed first) still disclaims, so its escapee is its member and dies with its job.
#[cfg(target_os = "macos")]
#[test]
fn an_inner_run_whose_responsible_process_died_still_disclaims() {
    let d = scratch("orphaned");
    let (go, rec, itrace) = (d.join("go"), d.join("esc"), d.join("itrace"));
    let _g = Recs(vec![rec.clone()]);
    let mut o = Outer(
        Command::new(sheepr())
            .args(["run", "--", fixture(), "after", go.to_str().unwrap(), sheepr(), "run", "--", fixture(), "escapee-and-wait", rec.to_str().unwrap()])
            .env("SHEEPR_TEST_TRACE", &itrace)
            // the outer's root shows in `ps` from its spawn, before the outer resumes it: a pause
            // there (debug seam) keeps that window open, so the cell holds the wait below
            .env("SHEEPR_TEST_SLEEP_AFTER_SPAWN_MS", "1500")
            .stdin(Stdio::null())
            .spawn()
            .unwrap(),
    );
    // the outer's root is waiting for GO (the only process whose argv starts `sr-fixture after`),
    // and runs: it shows from its spawn, suspended until the outer resumes it, and a SIGKILL
    // before that would leave it suspended for ever (holding the test's output: cargo would hang);
    // end the outer, then let the inner start
    let go_word = go.to_str().unwrap().to_string();
    let waiting = |w: &[&str]| w.get(2) == Some(&"after");
    assert!(wait_until(10, || common::scan(&go_word, waiting).is_ok_and(|v| v.len() == 1 && !common::stopped(v[0]))), "the outer's root is not waiting");
    common::send_child(&mut o.0, libc::SIGKILL);
    let _ = o.0.wait();
    std::fs::write(&go, b"").unwrap();
    assert!(wait_until(15, || records(&rec).len() == 1 && !records(&PathBuf::from(format!("{}.root", rec.display()))).is_empty()), "the inner job did not start; trace:\n{}", read(&itrace));
    let g = records(&rec)[0];
    let root = records(&PathBuf::from(format!("{}.root", rec.display())))[0];
    // the inner job ends when its root is ended (SIGKILL: `sigcount` counts TERM and keeps
    // running): the inner kills its members then
    common::send(root.0, root.1, libc::SIGKILL);
    let gone = wait_until(10, || !common::alive(g));
    cleanup(&[&rec]);
    assert!(gone, "the inner job's escapee survived its job; trace:\n{}", read(&itrace));
    let _ = std::fs::remove_dir_all(&d);
}


/// A burst: 40 members register with one outer at once (the outer is stopped while they
/// connect, so all 40 wait in its listen backlog, and none is answered before it runs again);
/// every one gets its ack: past its cap of pending connections the outer leaves the rest in the
/// backlog, never refusing a member for that (the cap's value is not checked here).
#[cfg(target_os = "macos")]
#[test]
fn a_burst_of_registrations_is_served() {
    let d = scratch("burst");
    let (go, trace, started) = (d.join("go"), d.join("trace"), d.join("started"));
    // (the wait for GO is bounded: 10 s)
    let root = format!(
        r#": >"{}"; n=0; until [ -e "{}" ] || [ $n -ge 500 ]; do sleep 0.02; n=$((n+1)); done; for i in $(seq 1 40); do "$FX" register env 0 self good "{}/out$i" & done; wait"#,
        started.display(),
        go.display(),
        d.display()
    );
    let mut o = Outer(
        Command::new(sheepr())
            .args(["run", "--", "/bin/sh", "-c", &root])
            .env("FX", fixture())
            .env("SHEEPR_TEST_TRACE", &trace)
            .env("SR_REG_TIMEOUT_MS", "15000")
            // the outer listens before it starts and resumes the root: a pause there (debug seam)
            // keeps the window open, so the cell holds the wait below (issue #11)
            .env("SHEEPR_TEST_SLEEP_AFTER_SPAWN_MS", "500")
            .stdin(Stdio::null())
            .spawn()
            .unwrap(),
    );
    assert!(wait_until(10, || read(&trace).lines().any(|l| l.starts_with("listening "))), "the outer is not listening");
    // stopped before it resumed its root, the outer would hold the root too, and no client would
    // start (issue #11: "0 of 40", every client refused once the cell ended the outer)
    assert!(wait_until(10, || started.exists()), "the root did not start");
    let sup = (o.0.id() as i32, sheepr::ident::identity(o.0.id() as i32).unwrap_or(0));
    let stopped = common::send(sup.0, sup.1, libc::SIGSTOP);
    std::fs::write(&go, b"").unwrap();
    // all 40 have connected and sent (queued in the stopped outer's backlog) before it runs
    // again; the clients wait 15 s for their answer (SR_REG_TIMEOUT_MS), so a slow start under
    // load (up to this wait's 10 s) still leaves them time
    let conn = || (1..=40).filter(|i| d.join(format!("out{i}.conn")).exists()).count();
    let queued = wait_until(10, || conn() == 40);
    let n = conn();
    // nobody was answered while the outer was stopped: all 40 waited in its backlog
    let answered = (1..=40).filter(|i| d.join(format!("out{i}")).exists()).count();
    common::send(sup.0, sup.1, libc::SIGCONT);
    assert!(stopped, "control: the outer was stopped");
    assert!(queued, "only {n} of 40 clients connected before the outer ran again");
    assert_eq!(answered, 0, "clients were answered while the outer was stopped");
    let code = finished(&mut o.0, 30).and_then(|s| s.code());
    let trace = read(&trace);
    let answers: Vec<String> = (1..=40).map(|i| read(&d.join(format!("out{i}")))).collect();
    let acked = answers.iter().filter(|a| a.as_str() == "ack").count();
    assert_eq!(code, Some(0));
    assert_eq!(acked, 40, "answers {answers:?}; trace:\n{trace}");
    let _ = std::fs::remove_dir_all(&d);
}

/// Silent clients past the cap do not lock registration out: 40 connect and send nothing (each
/// is dropped after the outer's read deadline; neither the cap nor the deadline is checked here);
/// a member that registers 0.5 s later gets its ack.
#[cfg(target_os = "macos")]
#[test]
fn silent_clients_past_the_cap_do_not_lock_registration_out() {
    let d = scratch("capdeadline");
    let (ready, out) = (d.join("ready"), d.join("out"));
    let _g = Recs(vec![ready.clone()]);
    let root = format!(
        r#""$FX" silent env 40 "{r}" & until [ -s "{r}" ]; do sleep 0.05; done; sleep 0.5; "$FX" register env 0 self good "{o}"; true"#,
        r = ready.display(),
        o = out.display()
    );
    let (code, trace, _) = outer(&d, &root, &[("SHEEPR_TEST_DEADLINE_MS", "500")]);
    assert_eq!(read(&ready), "40", "the silent clients did not all connect");
    assert_eq!(read(&out), "ack", "trace:\n{trace}");
    assert_eq!(code, Some(0));
    let _ = std::fs::remove_dir_all(&d);
}

/// Each supervisor of the chain holds the jobs nested in it, not only the outermost: O -> A -> B
/// (fast double forks); B's supervisor is SIGKILLed, then A ends while O still runs: B's escapee
/// is gone at A's end.
#[test]
fn a_middle_run_holds_the_jobs_nested_in_it() {
    let d = scratch("holds");
    let (esc, a, b) = (d.join("esc"), d.join("a"), d.join("b"));
    let _g = Recs(vec![esc.clone(), a.clone(), b.clone()]);
    let launch = [
        fx(), "dfork-exec".into(), a.display().to_string(), sd(), "run".into(), "--".into(),
        fx(), "dfork-exec".into(), b.display().to_string(), sd(), "run".into(), "--".into(),
        fx(), "escapee-and-wait".into(), esc.display().to_string(),
    ];
    let mut o = Outer(Command::new(sheepr()).args(["run", "--"]).args(&launch).stdin(Stdio::null()).spawn().unwrap());
    assert!(wait_until(20, || !records(&esc).is_empty() && !records(&a).is_empty() && !records(&b).is_empty()), "the job did not start");
    let (g, pa, pb) = (records(&esc)[0], records(&a)[0], records(&b)[0]);
    std::thread::sleep(Duration::from_millis(800)); // past the loss window (see `trial`)
    common::send(pb.0, pb.1, libc::SIGKILL);
    assert!(wait_until(5, || !common::alive(pb)));
    common::send(pa.0, pa.1, libc::SIGTERM);
    let a_ended = wait_until(20, || !common::alive(pa));
    let gone = wait_until(5, || !common::alive(g));
    let o_running = o.0.try_wait().ok().flatten().is_none();
    common::send_child(&mut o.0, libc::SIGTERM);
    let _ = finished(&mut o.0, 20);
    assert!(a_ended && o_running, "A ended {a_ended}, O still running {o_running}");
    assert!(gone, "B's escapee survived A's end");
    let _ = std::fs::remove_dir_all(&d);
}

/// The outer guard ends a cell's outer with TERM (and CONT) first, so an outer that has not yet
/// resumed its root (a cell that fails early) kills that root itself; a SIGKILL alone left it
/// suspended for ever (two roots of the burst cell were left so on the operator's Mac). A debug
/// seam holds the outer after the spawn; the cell stops the outer there (as the burst cell does)
/// and drops the guard: the CONT lets the outer act on the TERM, so it ends well within the
/// guard's 5 s before a SIGKILL, and the root is gone.
#[cfg(target_os = "macos")]
#[test]
fn the_outer_guard_leaves_no_root_behind() {
    let d = scratch("guard");
    let mark = d.join("root").display().to_string();
    let o = Outer(
        Command::new(sheepr())
            .args(["run", "--", fixture(), "sigcount", &mark])
            .env("SHEEPR_TEST_SLEEP_AFTER_SPAWN_MS", "2000")
            .stdin(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let roots = || common::scan(&mark, |w| w.get(2) == Some(&"sigcount")).unwrap_or_default();
    let spawned = wait_until(10, || roots().len() == 1);
    let suspended = roots().first().is_some_and(|&r| common::stopped(r));
    let sup = (o.0.id() as i32, sheepr::ident::identity(o.0.id() as i32).unwrap_or(0));
    let stopped = common::send(sup.0, sup.1, libc::SIGSTOP);
    let t = Instant::now();
    drop(o);
    let took = t.elapsed();
    let gone = wait_until(5, || roots().is_empty());
    let left = roots();
    for p in &left {
        common::send(p.0, p.1, libc::SIGKILL);
    }
    assert!(spawned && suspended && stopped, "control: the outer had spawned its root, not yet resumed it, and was stopped");
    assert!(took < Duration::from_secs(3), "the guard took {took:?}: the outer did not act on its TERM");
    assert!(gone, "the guard left the root {left:?}");
    let _ = std::fs::remove_dir_all(&d);
}
