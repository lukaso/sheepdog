//! Phase 1, step S1 (PHASE1.md): the event loop. Readiness only, never a fixed sleep before
//! acting on a child (on macOS the first launch of a fresh binary is delayed by the security
//! scan). Debug seams: SHEEPDOG_TEST_SLEEP_BEFORE_WAIT_MS widens the loop's wait window,
//! SHEEPDOG_TEST_SLEEP_AFTER_REGISTER_MS widens the window between registering for events and
//! the first consumption pass, SHEEPDOG_TEST_READY_FILE is created when a window opens.

//!
//! Every signal goes through the doors in `common` (a child not yet reaped, or an
//! identity-checked pid found by its marker); nothing is ever sent to pid 1 or less.

mod common;

use common::{kill_marked, send_child};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn sheepdog() -> &'static str {
    env!("CARGO_BIN_EXE_sheepdog")
}

fn tmp(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("sd-loop-{}-{tag}", std::process::id()))
}

/// Wait for `child` with a bound; kill it and return None if it does not end.
fn wait_bounded(child: &mut std::process::Child, limit: Duration) -> Option<std::process::ExitStatus> {
    let start = Instant::now();
    loop {
        if let Some(st) = child.try_wait().unwrap() {
            return Some(st);
        }
        if start.elapsed() > limit {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Spawn with a seam window; return once sheepdog signals it is inside the window.
fn in_window(seam: &str, ms: &str, tag: &str, root: &[&str], pre_block_term: bool) -> std::process::Child {
    let ready = tmp(&format!("ready-{tag}"));
    let _ = std::fs::remove_file(&ready);
    let mut c = Command::new(sheepdog());
    c.args(["run", "--"]).args(root).env(seam, ms).env("SHEEPDOG_TEST_READY_FILE", &ready);
    c.stdout(Stdio::null()).stderr(Stdio::null());
    if pre_block_term {
        unsafe {
            c.pre_exec(|| {
                let mut s: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut s);
                libc::sigaddset(&mut s, libc::SIGTERM);
                libc::sigprocmask(libc::SIG_BLOCK, &s, std::ptr::null_mut());
                Ok(())
            });
        }
    }
    let mut child = c.spawn().unwrap();
    let start = Instant::now();
    while !ready.exists() {
        if start.elapsed() > Duration::from_secs(10) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("sheepdog never reached the {seam} window");
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let _ = std::fs::remove_file(&ready);
    child
}

/// Is a `/bin/sleep <marker>` process running, per `ps -Ao args=` output? The program must
/// be `/bin/sleep`: sheepdog's own argv also contains the marker (`run -- /bin/sleep <marker>`),
/// and matching it would "see" the root before it exists.
fn root_running(ps: &str, marker: &str) -> bool {
    ps.lines().any(|l| {
        let w: Vec<&str> = l.split_whitespace().collect();
        w.len() >= 2 && w[0] == "/bin/sleep" && w[1] == marker
    })
}

/// P3-F1: once the root exits, sheepdog exits within 100 ms, in every run (NOTE_EXIT can
/// arrive before the root is reapable; the loop must then reap it with a blocking wait,
/// not sleep through a timeout).
#[test]
fn s1_exit_follows_the_root_within_100ms_every_time() {
    let n: usize = std::env::var("SD_LATENCY_N").ok().and_then(|v| v.parse().ok()).unwrap_or(1000);
    let _ = Command::new(sheepdog()).args(["run", "--", "true"]).status(); // warm-up (first-launch scan)
    let mut worst = Duration::ZERO;
    for _ in 0..n {
        let t = Instant::now();
        let st = Command::new(sheepdog()).args(["run", "--", "true"]).status().unwrap();
        let took = t.elapsed();
        assert_eq!(st.code(), Some(0));
        worst = worst.max(took);
    }
    assert!(worst < Duration::from_millis(100), "the slowest of {n} runs took {worst:?}");
}

/// P3-F2: with a TERM already pending when sheepdog starts (the caller blocked TERM and it
/// arrived), the root never runs, and sheepdog dies of SIGTERM.
#[test]
fn s1_a_term_pending_at_start_means_the_root_never_runs() {
    let started = tmp("f2-started");
    let _ = std::fs::remove_file(&started);
    // control: without a pending TERM the same root does run (the root resolves on every image)
    let ctl = tmp("f2-control");
    let _ = std::fs::remove_file(&ctl);
    Command::new(sheepdog()).args(["run", "--", "touch"]).arg(&ctl).status().unwrap();
    assert!(ctl.exists(), "control: the root did not run without a TERM");
    let _ = std::fs::remove_file(&ctl);
    let mut c = unsafe {
        Command::new(sheepdog())
            .args(["run", "--", "touch"])
            .arg(&started)
            .env("SHEEPDOG_TEST_SLEEP_BEFORE_SPAWN_MS", "300")
            .env("SHEEPDOG_TEST_READY_FILE", tmp("f2-ready"))
            .pre_exec(|| {
                // the caller blocks TERM; sheepdog inherits the mask with TERM pending
                let mut s: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut s);
                libc::sigaddset(&mut s, libc::SIGTERM);
                libc::sigprocmask(libc::SIG_BLOCK, &s, std::ptr::null_mut());
                libc::raise(libc::SIGTERM);
                Ok(())
            })
            .spawn()
            .unwrap()
    };
    let st = wait_bounded(&mut c, Duration::from_secs(5));
    let ran = started.exists();
    let _ = std::fs::remove_file(&started);
    let _ = std::fs::remove_file(tmp("f2-ready"));
    assert!(!ran, "the root ran although a TERM was pending before it existed");
    assert_eq!(st.and_then(|s| s.signal()), Some(libc::SIGTERM), "sheepdog must die of SIGTERM");
}

/// P3-F3 (macOS): a TERM during the self re-exec ends sheepdog before any root exists. The
/// root's FIRST action records that it started (`/usr/bin/touch`), so a root that ran even
/// for an instant is seen (a `sh -c touch` root started too slowly for that).
#[cfg(target_os = "macos")]
#[test]
fn s1_a_term_during_the_reexec_means_the_root_never_runs() {
    let started = tmp("f3-started");
    let _ = std::fs::remove_file(&started);
    let root = ["/usr/bin/touch", started.to_str().unwrap()];
    let mut c = in_window("SHEEPDOG_TEST_SLEEP_BEFORE_REEXEC_MS", "300", "f3", &root, false);
    assert!(send_child(&mut c, libc::SIGTERM), "sheepdog had already ended");
    let st = wait_bounded(&mut c, Duration::from_secs(5));
    let ran = started.exists();
    let _ = std::fs::remove_file(&started);
    assert!(!ran, "the root ran although TERM arrived before it existed");
    assert_eq!(st.and_then(|s| s.signal()), Some(libc::SIGTERM));
}

/// Exited but not reaped.
fn is_zombie(pid: i32) -> bool {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|s| s.rsplit_once(')').and_then(|(_, r)| r.trim_start().chars().next()))
            == Some('Z')
    }
    #[cfg(target_os = "macos")]
    {
        // proc_pidinfo fails for a zombie; ps reads the process table (sysctl)
        let out = Command::new("ps").args(["-o", "stat=", "-p", &pid.to_string()]).output().expect("ps");
        String::from_utf8_lossy(&out.stdout).trim_start().starts_with('Z')
    }
}

/// P3-F4: TERM and the root's exit in ONE wake: TERM wins, on both OSes (sheepdog dies of
/// SIGTERM, not with the root's code). The seam holds the loop while both happen.
#[test]
fn s1_term_and_root_exit_in_one_wake_term_wins() {
    // the root is still alive when the window opens (it exits after 100 ms), so the loop is
    // inside the window, then the root exits and TERM arrives: both wait for the next wake
    let done = tmp("f4-root-done");
    let _ = std::fs::remove_file(&done);
    let script = format!("sleep 0.1; echo $$ > '{}'; exit 7", done.display());
    let mut c = in_window("SHEEPDOG_TEST_SLEEP_BEFORE_WAIT_MS", "1500", "f4", &["sh", "-c", &script], false);
    // the root writes its pid right before it exits: wait until it is a zombie (exited, not yet
    // reaped, since the loop is held in the window), so TERM really coincides
    let t = Instant::now();
    let root = loop {
        if let Some(p) = std::fs::read_to_string(&done).ok().and_then(|s| s.trim().parse::<i32>().ok()) {
            break p;
        }
        assert!(t.elapsed() < Duration::from_secs(1), "the root never reached its exit (the window would close first)");
        std::thread::sleep(Duration::from_millis(2));
    };
    while !is_zombie(root) {
        assert!(t.elapsed() < Duration::from_secs(1), "the root never became a zombie");
        std::thread::sleep(Duration::from_millis(2));
    }
    let _ = std::fs::remove_file(&done);
    assert!(send_child(&mut c, libc::SIGTERM), "sheepdog had already ended");
    let st = wait_bounded(&mut c, Duration::from_secs(5)).expect("sheepdog did not end");
    assert_eq!(st.signal(), Some(libc::SIGTERM), "the root's exit won over TERM: {st:?}");
}

/// macOS consumption (plan review round 2): a TERM that arrives between the registration
/// for events and the first consumption pass is seen, and the supervisor ends; a second TERM
/// does not hang anything.
#[test]
fn s1_a_term_right_after_registration_ends_the_job() {
    let root = format!("23.{}111001", std::process::id());
    let mut c = in_window("SHEEPDOG_TEST_SLEEP_AFTER_REGISTER_MS", "300", "reg", &["/bin/sleep", &root], false);
    assert!(send_child(&mut c, libc::SIGTERM), "sheepdog had already ended");
    std::thread::sleep(Duration::from_millis(20));
    // the second TERM: sent only if sheepdog has not ended (and been reaped) yet
    send_child(&mut c, libc::SIGTERM);
    let st = wait_bounded(&mut c, Duration::from_secs(5));
    kill_marked(&[&root]);
    assert_eq!(st.and_then(|s| s.signal()), Some(libc::SIGTERM), "the supervisor did not end on TERM");
}

/// Linux: the loop's signalfd must not leak into the root (SFD_CLOEXEC).
#[cfg(target_os = "linux")]
#[test]
fn s1_the_root_holds_no_signalfd() {
    let out = Command::new(sheepdog())
        .args(["run", "--", "sh", "-c", "ls -l /proc/$$/fd"])
        .output()
        .unwrap();
    let fds = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "the root failed: {fds}");
    assert!(!fds.contains("signalfd"), "the root inherited a signalfd:\n{fds}");
}

/// P2-2 (S1 review): the loop wakes on TERM itself, not on its 250 ms tick. With a live root
/// and sheepdog blocked in its wait, TERM ends the job well under the tick, every time.
#[test]
fn s1_term_wakes_the_loop_at_once() {
    let _ = Command::new(sheepdog()).args(["run", "--", "true"]).status(); // warm-up (first-launch scan)
    let mut worst = Duration::ZERO;
    for i in 0..10 {
        let root = format!("23.{}112{:03}", std::process::id(), i);
        let mut c = Command::new(sheepdog()).args(["run", "--", "/bin/sleep", &root]).spawn().unwrap();
        // readiness: the root exists, so sheepdog is in (or about to enter) its wait
        let t = Instant::now();
        loop {
            let out = Command::new("ps").args(["-Ao", "args="]).output().unwrap();
            if root_running(&String::from_utf8_lossy(&out.stdout), &root) {
                break;
            }
            assert!(t.elapsed() < Duration::from_secs(10), "the root never started");
            std::thread::sleep(Duration::from_millis(5));
        }
        std::thread::sleep(Duration::from_millis(30)); // let sheepdog block in its wait
        let k = Instant::now();
        assert!(send_child(&mut c, libc::SIGTERM), "sheepdog had already ended");
        let st = wait_bounded(&mut c, Duration::from_secs(5)).expect("sheepdog did not end");
        worst = worst.max(k.elapsed());
        kill_marked(&[&root]);
        assert_eq!(st.signal(), Some(libc::SIGTERM));
    }
    assert!(worst < Duration::from_millis(100), "TERM took {worst:?} to end the job (the 250 ms tick, not a wake-up?)");
}

/// P3-b (S1 review, macOS): a kqueue registration that fails with anything but ESRCH falls
/// back to polling; TERM still ends the job (debug seam forces EINVAL).
#[cfg(target_os = "macos")]
#[test]
fn s1_a_failed_kqueue_registration_falls_back_to_polling() {
    let root = format!("23.{}113001", std::process::id());
    let logf = tmp("kq-einval-log");
    let _ = std::fs::remove_file(&logf);
    let mut c = Command::new(sheepdog())
        .args(["run", "--", "/bin/sleep", &root])
        .env("SHEEPDOG_TEST_KQ_EINVAL", "1")
        .env("SHEEPDOG_TEST_SIGNAL_LOG", &logf)
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let t = Instant::now();
    loop {
        let out = Command::new("ps").args(["-Ao", "args="]).output().unwrap();
        if root_running(&String::from_utf8_lossy(&out.stdout), &root) {
            break;
        }
        assert!(t.elapsed() < Duration::from_secs(10), "the root never started");
        std::thread::sleep(Duration::from_millis(5));
    }
    std::thread::sleep(Duration::from_millis(50)); // the root exists: let sheepdog settle into its wait
    assert!(send_child(&mut c, libc::SIGTERM), "sheepdog had already ended");
    let st = wait_bounded(&mut c, Duration::from_secs(3));
    // KILL, not TERM: a root left stopped would keep TERM pending and the stderr pipe open
    kill_marked(&[&root]);
    let log = std::fs::read_to_string(&logf).unwrap_or_default();
    let _ = std::fs::remove_file(&logf);
    assert!(log.lines().any(|l| l == "polling exit"), "the seam did not make the wait poll: {log:?}");
    assert_eq!(st.and_then(|s| s.signal()), Some(libc::SIGTERM), "TERM was ignored after a failed registration");
}

/// P2-1 (S1 review, Linux): a TERM pending when sheepdog starts must end it before any root
/// runs also in the relay shape (sheepdog starts with a child it did not create, so it forks
/// a relay; a pending signal is not inherited across fork). Debug seam
/// SHEEPDOG_TEST_SLEEP_RELAY_BEFORE_FORWARD_MS delays the relay's forwarding, which makes the
/// race deterministic.
#[cfg(target_os = "linux")]
#[test]
fn s1_a_term_pending_at_start_means_no_root_in_the_relay_shape() {
    let started = tmp("relay-f2-started");
    let _ = std::fs::remove_file(&started);
    let job = format!("22.{}114001", std::process::id());
    let job_c = std::ffi::CString::new(job.clone()).unwrap();
    let mut c = unsafe {
        Command::new(sheepdog())
            .args(["run", "--", "touch"])
            .arg(&started)
            .env("SHEEPDOG_TEST_SLEEP_RELAY_BEFORE_FORWARD_MS", "300")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .pre_exec(move || {
                // a child that sheepdog did not create (the relay shape), then a pending TERM
                if libc::fork() == 0 {
                    let prog = b"/bin/sleep\0";
                    let argv = [prog.as_ptr() as *const libc::c_char, job_c.as_ptr(), std::ptr::null()];
                    libc::execv(argv[0], argv.as_ptr());
                    libc::_exit(127);
                }
                let mut s: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut s);
                libc::sigaddset(&mut s, libc::SIGTERM);
                libc::sigprocmask(libc::SIG_BLOCK, &s, std::ptr::null_mut());
                libc::raise(libc::SIGTERM);
                Ok(())
            })
            .spawn()
            .unwrap()
    };
    let st = wait_bounded(&mut c, Duration::from_secs(5));
    std::thread::sleep(Duration::from_millis(100));
    let ran = started.exists();
    let _ = std::fs::remove_file(&started);
    kill_marked(&[&job]);
    assert!(!ran, "the root ran although a TERM was pending before sheepdog started");
    assert_eq!(st.and_then(|s| s.signal()), Some(libc::SIGTERM));
}
