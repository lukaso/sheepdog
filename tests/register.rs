//! Phase-2 P5: registration of nested runs (PLAN.md §3.2, PHASE2.md decision 13 and D5).
//!
//! Cell 15: an inner `sheepdog run` started through a fast double fork (and through a Node and a
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
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

static SEQ: AtomicUsize = AtomicUsize::new(0);

fn sheepdog() -> &'static str {
    common::test_env();
    env!("CARGO_BIN_EXE_sheepdog")
}
fn fixture() -> &'static str {
    common::test_env();
    env!("CARGO_BIN_EXE_sd-fixture")
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("sd-reg-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A marker unique to one iteration (`/bin/sleep <marker>` ends by itself within 29 s).
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
        for suffix in ["", ".g", ".root"] {
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
    let mut o = Command::new(sheepdog()).args(["run", "--"]).args(launch).env("SHEEPDOG_TEST_TRACE", &trace).stdin(Stdio::null()).spawn().unwrap();
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

/// The outer run of a cell, SIGKILLed when the cell ends (a panic too): a live outer holds the
/// test's output pipe, and cargo would wait for it.
struct Outer(Child);
impl Drop for Outer {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            common::send_child(&mut self.0, libc::SIGKILL);
            let _ = self.0.wait();
        }
    }
}

fn fx() -> String {
    fixture().to_string()
}
fn sd() -> String {
    sheepdog().to_string()
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
                    if let Some(id) = sheepdog::ident::identity(p) {
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
    let mut c = Command::new(sheepdog());
    c.args(["run", "--", "/bin/sh", "-c", root])
        .env("FX", fixture())
        .env("SD", sheepdog())
        .env("SHEEPDOG_TEST_TRACE", &trace)
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
    assert!(long.join("sd-XXXXXXXX/s").as_os_str().len() > 100, "the cell's TMPDIR is not long enough");
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
    let fake = |n: usize| (0..n).map(|i| format!("{:032x} /nonexistent/sd-{i}/s", i + 1)).collect::<Vec<_>>().join("\n");
    for (given, want) in [(15, 16), (16, 16)] {
        let out = d.join(format!("env{given}"));
        let (code, trace, _) = outer(&d, &format!(r#""$FX" print-env SHEEPDOG_OUTER "{}""#, out.display()), &[("SHEEPDOG_OUTER", &fake(given))]);
        let (seen, bad) = sheepdog::regwire::parse(&read(&out));
        assert_eq!(code, Some(0));
        assert_eq!((seen.len(), bad), (want, 0), "given {given}");
        let (mine, _) = sheepdog::regwire::parse(&fake(given));
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
    let (code, _, _) = outer(&d, &root, &[("SHEEPDOG_TEST_REG_PIDVERSION_CHANGE", "1")]);
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
    let m = marker();
    let t0 = Instant::now();
    let mut c = Command::new(sheepdog())
        .args(["run", "--timeout", "1s", "--"])
        .args([fixture(), "silent", "env", "20", &m, ready.to_str().unwrap()])
        .stdin(Stdio::null())
        .spawn()
        .unwrap();
    let st = finished(&mut c, 30);
    let took = t0.elapsed();
    common::kill_marked(&[&m]);
    assert_eq!(read(&ready), "20", "the clients did not all connect");
    assert_eq!(st.and_then(|s| s.code()), Some(124));
    assert!(took < Duration::from_millis(2500), "the timeout fired after {took:?}");
    let _ = std::fs::remove_dir_all(&d);
}

/// The spoof: a member registers claiming a decoy's pid (a process outside the job). The outer
/// takes the sender from the kernel, refuses the claim, and the decoy gets no signal.
#[cfg(target_os = "macos")]
#[test]
fn a_registration_for_another_process_is_refused() {
    let d = scratch("spoof");
    let dr = d.join("decoy");
    let mut decoy = Command::new(fixture()).args(["sigcount", dr.to_str().unwrap()]).spawn().unwrap();
    assert!(wait_until(10, || !records(&dr).is_empty()));
    let dp = records(&dr)[0];
    let out = d.join("out");
    let (code, _, _) = outer(&d, &format!(r#""$FX" register env 0 {} good "{}"; true"#, dp.0, out.display()), &[]);
    let (alive, sigs) = (common::alive(dp), read(&PathBuf::from(format!("{}.sig", dr.display()))).lines().count());
    common::send_child(&mut decoy, libc::SIGKILL);
    let _ = decoy.wait();
    assert_eq!(code, Some(0));
    assert_eq!(read(&out), "refused");
    assert!(alive && sigs == 0, "the decoy got {sigs} signal(s), alive {alive}");
    let _ = std::fs::remove_dir_all(&d);
}

/// A process outside the job that has the socket path and nonce (read from the root's env)
/// registers: it is not a member, so it is refused, and it survives the outer's end.
#[cfg(target_os = "macos")]
#[test]
fn a_non_member_with_the_nonce_is_refused() {
    let d = scratch("stolen");
    let chain = d.join("chain");
    let mut o = Command::new(sheepdog())
        .args(["run", "--", fixture(), "print-env", "SHEEPDOG_OUTER", chain.to_str().unwrap(), &marker()])
        .stdin(Stdio::null())
        .spawn()
        .unwrap();
    assert!(wait_until(10, || !read(&chain).is_empty()), "the root did not write its chain");
    let out = d.join("out");
    let mut outsider = Command::new(fixture()).args(["register", chain.to_str().unwrap(), "0", "self", "good", out.to_str().unwrap(), &marker()]).spawn().unwrap();
    let op = (outsider.id() as i32, sheepdog::ident::identity(outsider.id() as i32).unwrap_or(0));
    assert!(wait_until(10, || !read(&out).is_empty()));
    common::send_child(&mut o, libc::SIGTERM);
    let ended = finished(&mut o, 20).is_some();
    let alive = common::alive(op);
    common::send_child(&mut outsider, libc::SIGKILL);
    let _ = outsider.wait();
    assert!(ended);
    assert_eq!(read(&out), "refused");
    assert!(alive, "the outsider was killed with the job");
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
    let root = format!(r#""$FX" print-env SHEEPDOG_OUTER "{}"; exec /usr/bin/env -i "$FX" register "{}" 0 self good "{}" {m}"#, chain.display(), chain.display(), out.display());
    let mut c = Command::new(sheepdog());
    c.args(["run", "--", "/bin/sh", "-c", &root]).env("FX", fixture()).env("SHEEPDOG_TEST_DEADLINE_MS", "500").env_remove("SHEEPDOG_TEST_STATE").stdin(Stdio::null());
    common::cell_sink(&mut c, &sink);
    let mut o = c.spawn().unwrap();
    assert!(wait_until(10, || !read(&out).is_empty()), "the registrant did not answer");
    // the registrant is the root; after its answer it runs `/bin/sleep <marker>` (the same pid)
    // (the marker is also in O's and the shell's argv: only the sleep itself counts)
    let sleeping = |w: &[&str]| w.get(1) == Some(&"/bin/sleep");
    assert!(wait_until(10, || common::scan(&m, sleeping).is_ok_and(|v| v.len() == 1)), "the registrant is not sleeping");
    let reg = common::scan(&m, sleeping).unwrap_or_default();
    common::send_child(&mut o, libc::SIGTERM);
    let _ = finished(&mut o, 20);
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

/// An inner run that starts after the process responsible for it has died (its outer sheepdog was
/// SIGKILLed first) still disclaims, so its escapee is its member and dies with its job.
#[cfg(target_os = "macos")]
#[test]
fn an_inner_run_whose_responsible_process_died_still_disclaims() {
    let d = scratch("orphaned");
    let (go, rec, itrace) = (d.join("go"), d.join("esc"), d.join("itrace"));
    let mut o = Outer(
        Command::new(sheepdog())
            .args(["run", "--", fixture(), "after", go.to_str().unwrap(), sheepdog(), "run", "--", fixture(), "escapee-and-wait", rec.to_str().unwrap()])
            .env("SHEEPDOG_TEST_TRACE", &itrace)
            .stdin(Stdio::null())
            .spawn()
            .unwrap(),
    );
    // the outer's root is waiting for GO (the only process whose argv starts `sd-fixture after`);
    // end the outer, then let the inner start
    let go_word = go.to_str().unwrap().to_string();
    let waiting = |w: &[&str]| w.get(2) == Some(&"after");
    assert!(wait_until(10, || common::scan(&go_word, waiting).is_ok_and(|v| v.len() == 1)), "the outer's root is not waiting");
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

