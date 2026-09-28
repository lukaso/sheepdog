//! Test fixture, not a product binary. Every mode writes `<pid> <identity>` lines to
//! <record-file> for the processes it creates, so the checker can find them by identity
//! (sheepdog::ident) and not only by argv. Every escapee runs `/bin/sleep <marker>`: the marker
//! is a number such as `29.0123456`, unique to one test iteration, so a leak ends by itself
//! after at most 29 s.
//!
//! Modes (PLAN.md §6):
//! - `escape M R`: cell 3, shape "root waits". The root forks C; C calls setsid, forks G,
//!   passes G's pid up a pipe and exits at once; the root records G and exits. The root is the
//!   recorder because sheepdog never kills it before it exits (a C or G recorder can be killed
//!   before it writes: on Debian 26% were), so creation is counted exactly.
//! - `escape-fast M R`: cell 3, shape "root exits at once" (does not wait for G). C records G
//!   and can be killed before it does, so the creation count is a lower bound.
//! - `escape-nomarker R`: as `escape`, but G runs `/bin/sleep 29` with no unique marker. The
//!   checker's control: only the identity check can find this G.
//! - `breed M R N`: cell 7 (lite). As `escape`, but G keeps forking: N children, one every
//!   200 µs, each recorded by G and running `/bin/sleep M`. The root exits as soon as G exists,
//!   so the breeding happens while sheepdog is killing. A kill without freeze-and-repeat misses
//!   children created between its scan and its signal.
//! - `chain M R N [DELAY_US]`: cell 7 (chain). As `escape`, but G runs a chain of N generations:
//!   each generation waits DELAY_US (default 500 µs; 0 = as fast as fork allows), forks its successor and exits, so the tree keeps moving; the last
//!   generation runs `/bin/sleep M`. A killer that does not freeze stays one generation behind
//!   until the chain completes, and then the last generation survives.
//! - `exec-chld-ignored PROG ARGS...`: set SIGCHLD to ignored, then exec PROG. Shells cannot
//!   do this (`trap '' CHLD` leaves SIGCHLD handled), so the SIGCHLD test needs it.
//! - `print-mask`: print the numbers of the blocked signals, one per line.
//! - `test-env TESTBIN ARGS...`: the cargo runner's body (`scripts/test-env`, PHASE2.md §0.4).
//!   Runs TESTBIN with the test environment: a tag (`SHEEPDOG_LEG_TAG` if set, else 128 random
//!   bits) in `SHEEPDOG_TEST_TAG`, a canary `SHEEPDOG_TEST_STATE` without the sentinel, a
//!   withheld sink in `SHEEPDOG_TEST_SINK`, and first on PATH a directory whose `sheepdog` is a
//!   symlink to the debug binary next to TESTBIN's `deps` directory; every other inherited
//!   `SHEEPDOG_*` and `XDG_STATE_HOME` removed. It forwards TERM and INT, waits, passes the exit
//!   status on, and fails the run if anything was written to the canary or the sink.
//! - `term-logger M R [stop]`: as `escape`, but G does not exec: it catches TERM, appends
//!   `TERM <pid>` to `<R>.term` and exits 0. With `stop`, G first stops itself (SIGSTOP), so it
//!   can only see a TERM if someone continues it. G's argv carries the marker M.
//! - `redisclaim M R` (macOS): the root forks C and waits for it. C lives 700 ms (so scans
//!   see it as a member), then re-execs itself with the responsibility disclaim (C becomes
//!   responsible for itself), starts a stray GG (fork G; G: setsid, fork GG, exit; GG waits
//!   50 ms, so it execs after it was reparented, then runs `/bin/sleep M`), lives 700 ms more
//!   and exits. Only R growing from members (C) and sticky membership (GG after C is gone)
//!   keep GG a member (PLAN.md §3.2, cell 24).
//! - `puniq-only M R` (macOS): the root forks D; D re-execs itself with the disclaim
//!   (responsible for itself) and then runs `/bin/sleep M`; the root lives 700 ms, then exits.
//!   D's only fact is its original parent's uniqueid (the root, a member).
//! - `counter M R`: phase-1 S4 (cell 22). The root and one escapee (its own session) count
//!   every INT and HUP they get as lines `INT <pid>` / `HUP <pid>` in `<R>.sig` and keep
//!   running. The root records the escapee, then itself: two lines in R mean "ready".
//! - `shell REPORT [bg] [null-stdin] [tostop] PROG ARGS...`: the pty harness's job-control shell (PHASE1.md
//!   §2). Run as a session leader with the pty as its controlling terminal. It starts PROG in a
//!   new process group, makes that group the foreground (unless `bg`), and waits with
//!   WUNTRACED. It appends `started <pgid> <identity>`, `stopped <sig>`, `exited <code>` or
//!   `signaled <sig>` to REPORT. After a stop it waits for `REPORT.fg` to exist, then gives
//!   the terminal back to the job, continues it (`fg`) and appends `continued`. A HUP (the terminal closed) is
//!   sent on to the job's group, as bash does, and logged as `hup`.
//! - `nosession PIDFILE PROG ARGS...`: start PROG with no controlling terminal and not as a
//!   session leader (as liveapp and CI run it): setsid, then PROG in a new process group of
//!   that session. `<pid> <identity>` of PROG (its pid is its pgid) goes to PIDFILE; exits as
//!   PROG did.
//! - `ticker M R [slow-tstp F | regroup-tstp | read | write]`: phase-1 S5 (cells 21, 21′). The
//!   root starts an escapee (its own session) that appends a line to `<R>.tick` every 20 ms and
//!   breeds: every 200 ms it forks a child that ticks too (up to 4). Every process is recorded
//!   in R; the root also names itself in `<R>.root` once its handler is set ("ready"). Then:
//!   `slow-tstp F`: on TSTP it takes 300 ms, creates F, then stops itself (a pager restoring the
//!   terminal); `regroup-tstp`: on TSTP it sets TSTP to default and sends it to its whole group
//!   (`kill(0, SIGTSTP)`) after 100 ms; `fork-on-tstp`: 200 ms after a TSTP it forks a ticking
//!   child (recorded) and keeps running; `read`: reads its stdin (TTIN in the background); `write`: writes a
//!   line to stdout every 50 ms (TTOU in the background with `tostop`); otherwise it waits.
//! - `int-exit CODE M READY`: exit CODE on INT (a root that handles ctrl-C itself); creates
//!   READY once the handler is installed; else waits.
//! - `deep M R N`: phase-1 S6 (cell 27). A live chain of N processes, this one first: each
//!   records itself, forks the next and waits for it; the last runs `/bin/sleep M`.
//! - `setsid-kid M R`: S6 (cell 28(a)). This process forks C and waits; C starts a new session,
//!   forks G (`/bin/sleep M`) and waits. All three are recorded.
//! - `fork-on-term M R`: S6 (macOS `puniq`). This process forks C and waits; C waits, and on TERM
//!   forks G (a new session, TERM ignored, recorded; it keeps this image and waits, 60 s at most)
//!   and exits at once.
//! - `linger-on M R GO`: S6. Waits for the file GO, then forks C (a new session); C forks G
//!   (`/bin/sleep M`, recorded), lives 300 ms and exits. This process then waits (60 s at most).
//! - `escape-exec R PROG ARGS...`: S7 (cells 8, 9). As `escape`, but G records itself and then
//!   execs PROG ARGS (`/bin/bash`, `env -i`, ...); the root records G too and exits (with
//!   SD_EXEC_READY=<file>, only once that file is non-empty: the route reached its program).
//! - `storm DONE SECS`: S7 (cell 7, zombies). C (a new session) forks every 5 ms for SECS s; each
//!   child forks an orphan that exits 20 ms later. Then this process creates DONE and waits.
//! - `spawnp PROG ARGS...`: run PROG with posix_spawnp in this environment (PATH may be unset)
//!   and exit with its code, or 127 (ENOENT) / 126 when the spawn fails: phase 1's search, for a
//!   cell that compares the root shim's search with it.
//! - `swarm N R [STEP_US]`: PHASE2.md §0.1 (the stress cell). The root forks N children, each a fixture
//!   process (its environment readable on macOS), records each, and exits. Even children exit
//!   on TERM after a short delay (i*100 µs), so some members are exiting while the kill runs;
//!   odd children ignore TERM and die only of SIGKILL. SIGALRM ends each after 60 s at most.
//! - `alloc MBPS MAX_MB PROGRESS`: PHASE2.md P3 (cell 13). Allocates MBPS MB per second in 20 ms
//!   steps, writing a varied byte into every page (so resident memory really grows and no
//!   compressor flattens it), and writes the MB reached so far to PROGRESS; at MAX_MB it stops
//!   growing and waits (60 s at most).
//! - `alloc-on-term MB PROGRESS`: P3. Ignores nothing but catches TERM: on TERM it allocates MB at
//!   once (pages touched), writes MB to PROGRESS, and keeps running (60 s at most).
//! - `forker N INTERVAL_MS R`: P3 (`--max-procs`). Forks N children, one every INTERVAL_MS, each
//!   recording itself and waiting (60 s at most); then waits itself.
//! - `escapee-and-wait R [GO]`: P4 (sweep). (With GO: first waits for that file.) Starts a setsid escapee G (C: setsid, fork G, exit) that
//!   records itself in R and becomes `sigcount R.g` (the same pid); once R has G, this process
//!   becomes `sigcount R.root`. Every process is this fixture, so its environment can be read.
//! - `spawn-on GO R`: P4 (the closure). Records itself in R, waits for the file GO, then forks a
//!   child that records itself in R and waits; both wait (60 s at most).
//! - `worker R GO`: P4 (the journal in `kill <pid>`). Records itself (W) in R, forks C; C forks E
//!   and waits for the file GO before it exits; E records itself in R and waits. W waits. Every
//!   process is this fixture (60 s at most). R's lines: W, then E, then C.
//! - `lineage-kill R GO1 GO2 RC`: P4 review (P1-B). W records itself and forks C; C forks E and
//!   records itself once E has; C exits on GO1 (E is reparented, but the journal saw E under C
//!   under W); on GO2, E runs `sheepdog kill <W>` (by name, the test PATH), waits for it, writes
//!   its code to RC (its stderr to RC.err) and waits; without GO2 it exits and runs nothing. So the kill's own parent, E, is in W's journaled subtree although
//!   W is no ancestor of it. R's lines: W, E, C.
//! - `doublefork M R`: PHASE2.md P2 (`killed[].escaped`). The root forks C; C forks G (no new
//!   session) and exits; G records itself and runs `/bin/sleep M`; the root waits for the record
//!   and exits. G escaped by reparenting only.
//! - `sigcount R`: S8 (another-user leg). Records itself, then counts every catchable signal as a
//!   line `SIG <n>` in `R.sig` and keeps running (SIGALRM ends it after 1800 s at most).
//! - `bg-then-exec M PROG ARGS...`: fork a background job (`/bin/sleep M`, stdout and stderr
//!   to /dev/null), then exec PROG in this process, with no shell in between (a shell such as
//!   dash would reset the signal mask). This is the "job & exec sheepdog" shape.
//!   `bg-apart-then-exec` is the same, but the background job moves to a group of its own.

use sheepdog::ident::identity;
use std::ffi::CString;
use std::io::Write;

fn record(path: &str, pid: i32) {
    // identity 0 when the process is already gone (a fast chain's link): the line still counts
    // the creation, and 0 matches no process, so the checker's identity scan never acts on it
    let id = identity(pid).unwrap_or(0);
    if let Ok(mut f) = std::fs::OpenOptions::new().append(true).create(true).open(path) {
        let _ = f.write_all(format!("{pid} {id}\n").as_bytes());
    }
}

/// Exec this fixture as `sigcount DIR/NAME` (with an empty environment when `bare`).
unsafe fn sigcount_as(dir: &str, name: &str, bare: bool) -> ! {
    let me = CString::new(std::env::current_exe().unwrap().as_os_str().as_encoded_bytes()).unwrap();
    let args = [me.clone(), CString::new("sigcount").unwrap(), CString::new(format!("{dir}/{name}")).unwrap()];
    let argv = [args[0].as_ptr(), args[1].as_ptr(), args[2].as_ptr(), std::ptr::null()];
    if bare {
        let envp: [*const libc::c_char; 1] = [std::ptr::null()];
        libc::execve(me.as_ptr(), argv.as_ptr(), envp.as_ptr());
    } else {
        libc::execv(me.as_ptr(), argv.as_ptr());
    }
    libc::_exit(127)
}

/// Wait (at most 10 s) until DIR/NAME has a line.
fn wait_record(dir: &str, name: &str) {
    let p = format!("{dir}/{name}");
    let mut n = 0;
    while std::fs::read_to_string(&p).map_or(true, |t| t.is_empty()) && n < 1000 {
        unsafe { libc::usleep(10_000) };
        n += 1;
    }
}

/// An orphan named NAME: fork C; C forks G (G: setsid first when `apart`, then `body`); C waits
/// for G's record, then exits; this process reaps C.
unsafe fn orphan(dir: &str, name: &str, apart: bool, body: impl FnOnce()) {
    match libc::fork() {
        0 => {
            if libc::fork() == 0 {
                if apart {
                    libc::setsid();
                }
                body();
                libc::_exit(127);
            }
            wait_record(dir, name);
            libc::_exit(0);
        }
        c if c > 0 => {
            libc::waitpid(c, std::ptr::null_mut(), 0);
        }
        _ => libc::_exit(1),
    }
}

/// The `suspect-tree` mode's body (see its comment in `main`).
unsafe fn suspect_tree(dir: &str, untagged: bool) -> ! {
    orphan(dir, "early", false, || sigcount_as(dir, "early", false));
    libc::usleep(30_000);
    if libc::fork() == 0 {
        libc::usleep(30_000);
        orphan(dir, "g", false, || {
            if libc::fork() == 0 {
                // with SD_SUP (a sheepdog): gc is a supervisor whose job's root is `sigcount gc`
                if let Ok(sd) = std::env::var("SD_SUP") {
                    let me = std::env::current_exe().unwrap();
                    let args: Vec<CString> = [sd.as_str(), "run", "--", me.to_str().unwrap(), "sigcount", &format!("{dir}/gc")].iter().map(|s| CString::new(*s).unwrap()).collect();
                    let mut p: Vec<*const libc::c_char> = args.iter().map(|c| c.as_ptr()).collect();
                    p.push(std::ptr::null());
                    libc::execv(args[0].as_ptr(), p.as_ptr());
                    libc::_exit(127);
                }
                sigcount_as(dir, "gc", untagged);
            }
            wait_record(dir, "gc");
            sigcount_as(dir, "g", untagged)
        });
        orphan(dir, "g2", true, || sigcount_as(dir, "g2", false));
        #[cfg(target_os = "macos")]
        orphan(dir, "g3", true, || disclaim_reexec_args(&["sigcount", &format!("{dir}/g3")]));
        sigcount_as(dir, "t", false);
    }
    wait_record(dir, "t");
    libc::usleep(30_000);
    if libc::fork() == 0 {
        sigcount_as(dir, "n", false);
    }
    wait_record(dir, "n");
    sigcount_as(dir, "leader", false)
}

/// Send one registration record over a fresh connection to `path`; the outer's answer: `ack`,
/// `refused` (closed without a byte), `none` (nothing in 2 s) or `err <why>`.
fn reg_client(path: &std::path::Path, rec: &[u8], conn: &str) -> String {
    use std::io::Read;
    let mut s = match std::os::unix::net::UnixStream::connect(path) {
        Ok(s) => s,
        Err(e) => return format!("err {e}"),
    };
    if let Err(e) = s.write_all(rec) {
        return format!("err {e}");
    }
    // after the record: no file-system call inside the outer's per-connection deadline
    let _ = std::fs::write(conn, b"");
    // the answer's timeout: SD_REG_TIMEOUT_MS, default 2000 (a cell that holds the outer back
    // on purpose gives its clients longer)
    // (at least 1 ms: a zero timeout is refused by the socket and would mean no timeout at all)
    let ms: u64 = std::env::var("SD_REG_TIMEOUT_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(2000).max(1);
    let _ = s.set_read_timeout(Some(std::time::Duration::from_millis(ms)));
    let mut b = [0u8; 1];
    match s.read(&mut b) {
        Ok(1) if b[0] == b'1' => "ack".into(),
        Ok(0) => "refused".into(),
        Ok(_) => "err an unexpected answer".into(),
        Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => "none".into(),
        Err(e) => format!("err {e}"),
    }
}

unsafe fn exec_sleep(arg: &CString) -> ! {
    let prog = CString::new("/bin/sleep").unwrap();
    let argv = [prog.as_ptr(), arg.as_ptr(), std::ptr::null()];
    libc::execv(prog.as_ptr(), argv.as_ptr());
    libc::_exit(127)
}

/// Fork C; C: setsid, fork G (G runs `g_body`), then send G's pid up a pipe (if `via_pipe`) or
/// record it itself, and exit. Returns G's pid in the root, or None in fast mode.
unsafe fn spawn_escapee(rec: &str, via_pipe: bool, g_body: impl FnOnce()) -> Option<i32> {
    let mut fds = [0 as libc::c_int; 2];
    if libc::pipe(fds.as_mut_ptr()) != 0 {
        std::process::exit(1);
    }
    match libc::fork() {
        0 => {
            libc::close(fds[0]);
            libc::setsid();
            match libc::fork() {
                0 => {
                    libc::close(fds[1]);
                    g_body();
                    libc::_exit(0)
                }
                g => {
                    if via_pipe {
                        let b = g.to_ne_bytes();
                        libc::write(fds[1], b.as_ptr() as *const libc::c_void, b.len());
                    } else {
                        record(rec, g);
                    }
                    libc::_exit(0)
                }
            }
        }
        -1 => std::process::exit(1),
        _ => {
            libc::close(fds[1]);
            if !via_pipe {
                return None;
            }
            let mut b = [0u8; 4];
            (libc::read(fds[0], b.as_mut_ptr() as *mut libc::c_void, 4) == 4)
                .then(|| i32::from_ne_bytes(b))
        }
    }
}

/// Is this process responsible for itself (the disclaim took effect)?
#[cfg(target_os = "macos")]
fn self_responsible() -> bool {
    type RespUniq = unsafe extern "C" fn(libc::pid_t) -> u64;
    unsafe {
        let name = CString::new("responsibility_get_uniqueid_responsible_for_pid").unwrap();
        let f = libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr());
        if f.is_null() {
            return false;
        }
        let f: RespUniq = std::mem::transmute(f);
        let me = libc::getpid();
        identity(me).map_or(false, |u| f(me) == u)
    }
}

/// Stopped by a signal (`T`).
fn is_stopped(pid: i32) -> bool {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|s| s.rsplit_once(')').and_then(|(_, r)| r.trim_start().chars().next()))
            == Some('T')
    }
    #[cfg(target_os = "macos")]
    unsafe {
        let mut b: libc::proc_bsdinfo = std::mem::zeroed();
        let n = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let r = libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, &mut b as *mut _ as *mut libc::c_void, n);
        r == n && b.pbi_status == 4 // SSTOP
    }
}

static mut TERM_FD: libc::c_int = -1;
static mut DECOY_FD: libc::c_int = -1;

/// decoy: log the signal number and keep running.
extern "C" fn log_signal(sig: libc::c_int) {
    unsafe {
        let line = format!("{sig}\n");
        libc::write(DECOY_FD, line.as_ptr() as *const libc::c_void, line.len());
    }
}

/// term-logger: record the TERM, take 300 ms to shut down, record the clean exit. A SIGKILL
/// during those 300 ms (the grace not honoured) leaves no EXIT line.
extern "C" fn on_term(_: libc::c_int) {
    unsafe {
        let pid = libc::getpid();
        let line = format!("TERM {pid}\n");
        libc::write(TERM_FD, line.as_ptr() as *const libc::c_void, line.len());
        libc::usleep(300_000);
        let line = format!("EXIT {pid}\n");
        libc::write(TERM_FD, line.as_ptr() as *const libc::c_void, line.len());
        libc::_exit(0);
    }
}

/// term-counter: record every TERM and keep running (only SIGKILL ends it).
extern "C" fn count_term(_: libc::c_int) {
    unsafe {
        let pid = libc::getpid();
        let line = format!("TERM {pid}\n");
        libc::write(TERM_FD, line.as_ptr() as *const libc::c_void, line.len());
    }
}

static mut SIG_FD: libc::c_int = -1;
static mut INT_EXIT: libc::c_int = 0;
static HUP_SEEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// counter: record an INT or HUP and keep running.
extern "C" fn count_sig(sig: libc::c_int) {
    unsafe {
        let name = if sig == libc::SIGINT { "INT" } else { "HUP" };
        let line = format!("{name} {}\n", libc::getpid());
        libc::write(SIG_FD, line.as_ptr() as *const libc::c_void, line.len());
    }
}

extern "C" fn int_exit(_: libc::c_int) {
    unsafe { libc::_exit(INT_EXIT) }
}

extern "C" fn shell_hup(_: libc::c_int) {
    HUP_SEEN.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// Install `h` for `sig` with no SA_RESTART, so a blocking waitpid returns EINTR.
unsafe fn on(sig: libc::c_int, h: usize, restart: bool) {
    let mut sa: libc::sigaction = std::mem::zeroed();
    sa.sa_sigaction = h;
    sa.sa_flags = if restart { libc::SA_RESTART } else { 0 };
    libc::sigemptyset(&mut sa.sa_mask);
    libc::sigaction(sig, &sa, std::ptr::null_mut());
}

fn unblock_all() {
    unsafe {
        let mut none: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut none);
        libc::sigprocmask(libc::SIG_SETMASK, &none, std::ptr::null_mut());
    }
}

fn append(path: &str, line: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new().append(true).create(true).open(path) {
        let _ = f.write_all(format!("{line}\n").as_bytes());
    }
}

/// The job-control shell of the pty harness (see the module doc).
fn shell(report: &str, bg: bool, null_stdin: bool, prog: &[String]) -> ! {
    use std::os::unix::process::CommandExt;
    unsafe {
        for s in [libc::SIGINT, libc::SIGQUIT, libc::SIGTSTP, libc::SIGTTIN, libc::SIGTTOU] {
            libc::signal(s, libc::SIG_IGN);
        }
        on(libc::SIGHUP, shell_hup as *const () as usize, false);
        let mut cmd = std::process::Command::new(&prog[0]);
        cmd.args(&prog[1..]);
        cmd.pre_exec(move || {
            libc::setpgid(0, 0);
            if !bg {
                libc::tcsetpgrp(0, libc::getpid());
            }
            for s in [libc::SIGINT, libc::SIGQUIT, libc::SIGTSTP, libc::SIGTTIN, libc::SIGTTOU, libc::SIGHUP] {
                libc::signal(s, libc::SIG_DFL);
            }
            unblock_all();
            if null_stdin {
                let n = libc::open(b"/dev/null\0".as_ptr() as *const libc::c_char, libc::O_RDONLY);
                libc::dup2(n, 0);
            }
            Ok(())
        });
        let child = match cmd.spawn() {
            Ok(c) => c.id() as i32,
            Err(e) => {
                append(report, &format!("spawn-failed {e}"));
                libc::_exit(1)
            }
        };
        // the job's group is signalled below; a pid that cannot be ours must never become a
        // group signal (kill(-1) is every process this user owns)
        if child <= 1 {
            append(report, "bad-pid");
            libc::_exit(1);
        }
        libc::setpgid(child, child);
        if !bg {
            libc::tcsetpgrp(0, child);
        }
        // the identity is read before this shell can reap the child, so it is the child's
        append(report, &format!("started {child} {}", identity(child).unwrap_or(0)));
        let fg = format!("{report}.fg");
        loop {
            // poll, so a HUP that lands between two waits is seen within 5 ms (a blocking wait
            // would miss it until the job changes state)
            if HUP_SEEN.swap(false, std::sync::atomic::Ordering::SeqCst) {
                libc::kill(-child, libc::SIGHUP);
                append(report, "hup");
            }
            let mut st = 0;
            let r = libc::waitpid(child, &mut st, libc::WUNTRACED | libc::WNOHANG);
            if r == 0 || (r < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR)) {
                libc::usleep(5000);
                continue;
            }
            if r < 0 {
                append(report, "wait-failed");
                libc::_exit(1);
            }
            if libc::WIFSTOPPED(st) {
                append(report, &format!("stopped {}", libc::WSTOPSIG(st)));
                libc::tcsetpgrp(0, libc::getpgrp());
                let mut n = 0;
                while !std::path::Path::new(&fg).exists() && n < 30_000 {
                    libc::usleep(1000);
                    n += 1;
                }
                let _ = std::fs::remove_file(&fg);
                libc::tcsetpgrp(0, child);
                libc::kill(-child, libc::SIGCONT);
                append(report, "continued");
                continue;
            }
            if libc::WIFSIGNALED(st) {
                append(report, &format!("signaled {}", libc::WTERMSIG(st)));
            } else {
                append(report, &format!("exited {}", libc::WEXITSTATUS(st)));
            }
            libc::_exit(0);
        }
    }
}

static mut TSTP_FILE: [u8; 512] = [0; 512];

/// ticker slow-tstp: 300 ms of "restoring the terminal", then the marker, then a real stop.
extern "C" fn slow_tstp(_: libc::c_int) {
    unsafe {
        libc::usleep(300_000);
        let f = libc::open(std::ptr::addr_of!(TSTP_FILE) as *const libc::c_char, libc::O_WRONLY | libc::O_CREAT, 0o644);
        if f >= 0 {
            libc::close(f);
        }
        libc::signal(libc::SIGTSTP, libc::SIG_DFL);
        let mut one: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut one);
        libc::sigaddset(&mut one, libc::SIGTSTP);
        libc::sigprocmask(libc::SIG_UNBLOCK, &one, std::ptr::null_mut());
        libc::raise(libc::SIGTSTP);
        // continued: handle the next ctrl-Z the same way
        libc::signal(libc::SIGTSTP, slow_tstp as *const () as usize);
    }
}

static mut DECOY_LOG: [u8; 512] = [0; 512];

/// decoy: append `SIG <n>` to its log (open, write, close: async-signal-safe).
extern "C" fn decoy_sig(s: libc::c_int) {
    unsafe {
        let f = libc::open(std::ptr::addr_of!(DECOY_LOG) as *const libc::c_char, libc::O_WRONLY | libc::O_CREAT | libc::O_APPEND, 0o644);
        if f >= 0 {
            let mut line = *b"SIG 00\n";
            line[4] = b'0' + (s / 10) as u8;
            line[5] = b'0' + (s % 10) as u8;
            libc::write(f, line.as_ptr() as *const libc::c_void, line.len());
            libc::close(f);
        }
    }
}

static mut TERM_REC: [u8; 512] = [0; 512];

/// fork-on-term: on TERM, fork G (a new session, TERM ignored, recorded, then waiting, 60 s at
/// most), then exit: G's parent is gone before anything could see G as its child. G keeps this
/// image (argv with the marker): on macOS an exec after the reparenting would reset its `puniq`
/// to 1 (PLAN.md §2). It allocates in the handler (`record`, in the forked child only): safe
/// because the main loop sits in `pause()`.
extern "C" fn fork_on_term(_: libc::c_int) {
    unsafe {
        if libc::fork() == 0 {
            libc::setsid();
            libc::signal(libc::SIGTERM, libc::SIG_IGN);
            let rec = std::ffi::CStr::from_ptr(std::ptr::addr_of!(TERM_REC) as *const libc::c_char);
            record(&rec.to_string_lossy(), libc::getpid());
            libc::alarm(60);
            loop {
                libc::pause();
            }
        }
        libc::_exit(0);
    }
}

static mut FORK_TICK: [u8; 512] = [0; 512];
static mut FORK_REC: [u8; 512] = [0; 512];

/// ticker fork-on-tstp: 200 ms after a TSTP, fork a ticking child (recorded), and keep running
/// (the root does not stop by itself, so sheepdog SIGSTOPs it after its wait). It allocates
/// inside the handler (`format!`, `record`): safe only because the root's main loop sits in
/// `pause()` and allocates nothing (a fixture, not a pattern).
extern "C" fn fork_on_tstp(_: libc::c_int) {
    unsafe {
        libc::usleep(200_000);
        match libc::fork() {
            0 => {
                libc::alarm(60);
                let fd = libc::open(std::ptr::addr_of!(FORK_TICK) as *const libc::c_char, libc::O_WRONLY | libc::O_CREAT | libc::O_APPEND, 0o644);
                let line = format!("{}\n", libc::getpid());
                loop {
                    libc::write(fd, line.as_ptr() as *const libc::c_void, line.len());
                    libc::usleep(20_000);
                }
            }
            c if c > 0 => {
                let rec = std::ffi::CStr::from_ptr(std::ptr::addr_of!(FORK_REC) as *const libc::c_char).to_string_lossy().into_owned();
                record(&rec, c);
            }
            _ => {}
        }
    }
}

/// ticker regroup-tstp: stop the whole group, as some programs do. TSTP is blocked while its own
/// handler runs, so it is set to its default and unblocked first: the root then stops inside
/// the handler (without that, its own TSTP would stay pending and re-run the handler forever).
extern "C" fn regroup_tstp(_: libc::c_int) {
    unsafe {
        libc::signal(libc::SIGTSTP, libc::SIG_DFL);
        let mut one: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut one);
        libc::sigaddset(&mut one, libc::SIGTSTP);
        libc::sigprocmask(libc::SIG_UNBLOCK, &one, std::ptr::null_mut());
        // first some work (restoring the terminal): the second TSTP then reaches sheepdog while
        // it is stopping the job, not together with the terminal's
        libc::usleep(100_000);
        libc::kill(0, libc::SIGTSTP);
        libc::signal(libc::SIGTSTP, regroup_tstp as *const () as usize);
    }
}

/// One ticking process: a line `<pid>` in `tick` every 20 ms, forever.
unsafe fn tick_forever(tick: &CString) -> ! {
    libc::alarm(60); // alarms are not inherited by fork: a bounded life for each ticker
    let fd = libc::open(tick.as_ptr(), libc::O_WRONLY | libc::O_CREAT | libc::O_APPEND, 0o644);
    let line = format!("{}\n", libc::getpid());
    loop {
        libc::write(fd, line.as_ptr() as *const libc::c_void, line.len());
        libc::usleep(20_000);
    }
}

/// Is this process's parent a sheepdog (its executable's file name)?
fn parent_is_sheepdog() -> bool {
    let ppid = unsafe { libc::getppid() };
    // under a binary translator (Rosetta, qemu-user) the exe link names the translator: argv[0]
    #[cfg(target_os = "linux")]
    let path = {
        let exe = std::fs::read_link(format!("/proc/{ppid}/exe")).map(|p| p.display().to_string()).unwrap_or_default();
        let name = exe.rsplit('/').next().unwrap_or("");
        if name == "rosetta" || name.starts_with("qemu-") {
            std::fs::read(format!("/proc/{ppid}/cmdline")).ok().and_then(|b| b.split(|&c| c == 0).next().map(|a| String::from_utf8_lossy(a).into_owned())).unwrap_or_default()
        } else {
            exe
        }
    };
    #[cfg(target_os = "macos")]
    let path = {
        let mut buf = vec![0u8; 4096];
        let n = unsafe { libc::proc_pidpath(ppid, buf.as_mut_ptr() as *mut libc::c_void, buf.len() as u32) };
        String::from_utf8_lossy(&buf[..n.max(0) as usize]).into_owned()
    };
    path.rsplit('/').next() == Some("sheepdog")
}

/// Re-exec this fixture as `mode M R` with the responsibility disclaim (macOS), so the new
/// image is responsible for itself.
/// Re-exec this fixture with the responsibility disclaim, with these arguments.
#[cfg(target_os = "macos")]
unsafe fn disclaim_reexec_args(rest: &[&str]) -> ! {
    type Disclaim = unsafe extern "C" fn(*mut libc::posix_spawnattr_t, libc::c_int) -> libc::c_int;
    extern "C" {
        fn _NSGetEnviron() -> *mut *const *const libc::c_char;
    }
    let name = CString::new("responsibility_spawnattrs_setdisclaim").unwrap();
    let f = libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr());
    let mut path = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let n = libc::proc_pidpath(libc::getpid(), path.as_mut_ptr() as *mut libc::c_void, path.len() as u32);
    path.truncate(n.max(0) as usize);
    let path = CString::new(path).unwrap();
    let mut args: Vec<CString> = vec![path.clone()];
    args.extend(rest.iter().map(|s| CString::new(*s).unwrap()));
    let mut ptrs: Vec<*mut libc::c_char> = args.iter().map(|c| c.as_ptr() as *mut libc::c_char).collect();
    ptrs.push(std::ptr::null_mut());
    let mut attr: libc::posix_spawnattr_t = std::mem::zeroed();
    libc::posix_spawnattr_init(&mut attr);
    if f.is_null() {
        libc::_exit(3);
    }
    let d: Disclaim = std::mem::transmute(f);
    d(&mut attr, 1);
    libc::posix_spawnattr_setflags(&mut attr, libc::POSIX_SPAWN_SETEXEC as i16);
    libc::posix_spawn(std::ptr::null_mut(), path.as_ptr(), std::ptr::null(), &attr, ptrs.as_ptr(), *_NSGetEnviron() as *const *mut libc::c_char);
    libc::_exit(3)
}

#[cfg(target_os = "macos")]
unsafe fn disclaim_reexec(mode: &str, m: &str, r: &str) -> ! {
    type Disclaim = unsafe extern "C" fn(*mut libc::posix_spawnattr_t, libc::c_int) -> libc::c_int;
    extern "C" {
        fn _NSGetEnviron() -> *mut *const *const libc::c_char;
    }
    let name = CString::new("responsibility_spawnattrs_setdisclaim").unwrap();
    let f = libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr());
    let mut path = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let n = libc::proc_pidpath(libc::getpid(), path.as_mut_ptr() as *mut libc::c_void, path.len() as u32);
    path.truncate(n.max(0) as usize);
    let path = CString::new(path).unwrap();
    let args: Vec<CString> = [path.to_str().unwrap(), mode, m, r].iter().map(|s| CString::new(*s).unwrap()).collect();
    let mut ptrs: Vec<*mut libc::c_char> = args.iter().map(|c| c.as_ptr() as *mut libc::c_char).collect();
    ptrs.push(std::ptr::null_mut());
    let mut attr: libc::posix_spawnattr_t = std::mem::zeroed();
    libc::posix_spawnattr_init(&mut attr);
    if f.is_null() {
        libc::_exit(3); // no disclaim: the cell would pass for another reason
    }
    let d: Disclaim = std::mem::transmute(f);
    d(&mut attr, 1);
    libc::posix_spawnattr_setflags(&mut attr, libc::POSIX_SPAWN_SETEXEC as i16);
    libc::posix_spawn(std::ptr::null_mut(), path.as_ptr(), std::ptr::null(), &attr, ptrs.as_ptr(), *_NSGetEnviron() as *const *mut libc::c_char);
    libc::_exit(3) // SETEXEC returns only on failure
}

/// `alloc-on-term`: how much to allocate when TERM comes, and whether it came.
static ALLOC_ON_TERM_MB: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static ALLOC_ON_TERM_GOT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

extern "C" fn alloc_on_term(_: libc::c_int) {
    ALLOC_ON_TERM_GOT.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// `swarm`: how long an even child waits after TERM before it exits.
static SWARM_DELAY_US: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

extern "C" fn swarm_term(_: libc::c_int) {
    unsafe {
        libc::usleep(SWARM_DELAY_US.load(std::sync::atomic::Ordering::SeqCst));
        libc::_exit(0);
    }
}

/// The child the test-env runner waits for, for its TERM/INT forwarding handler.
static TEST_ENV_CHILD: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

extern "C" fn test_env_forward(sig: libc::c_int) {
    let c = TEST_ENV_CHILD.load(std::sync::atomic::Ordering::SeqCst);
    if c > 1 {
        unsafe { libc::kill(c, sig) }; // the child this runner spawned, never a group
    }
}

/// The cargo runner's body: see `test-env` in the header.
fn test_env(argv: &[String]) -> ! {
    use std::path::{Path, PathBuf};
    let bin = PathBuf::from(&argv[0]);
    let profile = bin.parent().and_then(Path::parent).map(Path::to_path_buf).unwrap_or_default();
    let tag = std::env::var("SHEEPDOG_LEG_TAG").ok().filter(|t| !t.is_empty()).unwrap_or_else(|| {
        let mut b = [0u8; 16];
        let ok = std::fs::File::open("/dev/urandom").and_then(|mut f| std::io::Read::read_exact(&mut f, &mut b)).is_ok();
        if !ok {
            eprintln!("sd-test-env: cannot read /dev/urandom");
            std::process::exit(125);
        }
        b.iter().map(|x| format!("{x:02x}")).collect()
    });
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("sd-testenv.{}.{nanos}", std::process::id()));
    let (bindir, canary, sink) = (dir.join("bin"), dir.join("canary"), dir.join("sink"));
    let made = std::fs::create_dir_all(&bindir)
        .and_then(|_| std::fs::create_dir(&canary))
        .and_then(|_| std::fs::write(&sink, b""))
        .and_then(|_| std::os::unix::fs::symlink(profile.join("sheepdog"), bindir.join("sheepdog")));
    if let Err(e) = made {
        eprintln!("sd-test-env: cannot make {}: {e}", dir.display());
        std::process::exit(125);
    }
    let mut cmd = std::process::Command::new(&bin);
    cmd.args(&argv[1..]);
    for (k, _) in std::env::vars_os() {
        if k.to_string_lossy().starts_with("SHEEPDOG_") {
            cmd.env_remove(&k);
        }
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut newpath = bindir.clone().into_os_string();
    newpath.push(":");
    newpath.push(path);
    cmd.env_remove("XDG_STATE_HOME")
        .env("SHEEPDOG_TEST_TAG", &tag)
        .env("SHEEPDOG_TEST_STATE", &canary)
        .env("SHEEPDOG_TEST_SINK", &sink)
        .env("PATH", newpath);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("sd-test-env: cannot run {}: {e}", bin.display());
            std::process::exit(125);
        }
    };
    TEST_ENV_CHILD.store(child.id() as i32, std::sync::atomic::Ordering::SeqCst);
    unsafe {
        libc::signal(libc::SIGTERM, test_env_forward as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, test_env_forward as *const () as libc::sighandler_t);
    }
    // the child's exit is observed first without reaping it, and only then is the forwarding
    // target cleared and the child reaped: a TERM that comes later is never sent to a pid the
    // runner has reaped (and that may be another process by then)
    let pid = child.id() as libc::id_t;
    loop {
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        if unsafe { libc::waitid(libc::P_PID, pid, &mut info, libc::WEXITED | libc::WNOWAIT) } == 0 {
            break;
        }
        if std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            break;
        }
    }
    TEST_ENV_CHILD.store(0, std::sync::atomic::Ordering::SeqCst);
    let status = child.wait();
    let mut rc = match status {
        Ok(s) => s.code().unwrap_or_else(|| 128 + std::os::unix::process::ExitStatusExt::signal(&s).unwrap_or(0)),
        Err(_) => 125,
    };
    let canary_used = std::fs::read_dir(&canary).map(|mut d| d.next().is_some()).unwrap_or(true);
    let sink_used = std::fs::metadata(&sink).map(|m| m.len() > 0).unwrap_or(true);
    if canary_used {
        eprintln!("sd-test-env: something wrote to the canary state directory {}", canary.display());
    }
    if sink_used {
        eprintln!("sd-test-env: signals were withheld; see {}", sink.display());
    }
    if canary_used || sink_used {
        if rc == 0 {
            rc = 1;
        }
    } else {
        let _ = std::fs::remove_dir_all(&dir);
    }
    std::process::exit(rc)
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let usage = || -> ! {
        eprintln!("usage: sd-fixture escape|escape-fast M R | escape-nomarker R | breed M R N");
        std::process::exit(2)
    };
    let mode = a.get(1).map(String::as_str).unwrap_or_else(|| usage());
    if mode == "test-env" && a.len() >= 3 {
        test_env(&a[2..]);
    }
    // P5 (registration). `dfork-exec R PROG ARGS...`: a fast double fork: C forks G and exits at
    // once; G (not in a new session) records itself in R and execs PROG ARGS. The root waits
    // until R has G's line (bounded, 10 s), then becomes `sigcount R.root` (this fixture, so its
    // environment can be read; `/bin/sleep` is a platform binary whose environment cannot).
    if mode == "dfork-exec" && a.len() >= 4 {
        let r = a[2].clone();
        let prog: Vec<CString> = a[3..].iter().map(|s| CString::new(s.as_str()).unwrap()).collect();
        let mut ptrs: Vec<*const libc::c_char> = prog.iter().map(|c| c.as_ptr()).collect();
        ptrs.push(std::ptr::null());
        unsafe {
            match libc::fork() {
                0 => {
                    if libc::fork() == 0 {
                        record(&r, libc::getpid());
                        libc::execvp(ptrs[0], ptrs.as_ptr());
                        libc::_exit(127);
                    }
                    libc::_exit(0);
                }
                -1 => std::process::exit(1),
                c => {
                    libc::waitpid(c, std::ptr::null_mut(), 0);
                    let mut n = 0;
                    while std::fs::read_to_string(&r).map_or(true, |t| t.is_empty()) && n < 1000 {
                        libc::usleep(10_000);
                        n += 1;
                    }
                    let me = CString::new(std::env::current_exe().unwrap().as_os_str().as_encoded_bytes()).unwrap();
                    let args = [me.clone(), CString::new("sigcount").unwrap(), CString::new(format!("{r}.root")).unwrap()];
                    let argv = [args[0].as_ptr(), args[1].as_ptr(), args[2].as_ptr(), std::ptr::null()];
                    libc::execv(me.as_ptr(), argv.as_ptr());
                    libc::_exit(127);
                }
            }
        }
    }
    // `disclaim-exec R PROG ARGS...` (macOS): re-exec itself with the responsibility disclaim
    // (as `disclaim-head`), so it is responsible for itself; start a kid, `sigcount R.kid`, which
    // is then responsible to it; record itself in R; exec PROG ARGS (same pid, still responsible
    // for itself). Exit 4 if the disclaim did not take effect.
    #[cfg(target_os = "macos")]
    if mode == "disclaim-exec" && a.len() >= 4 {
        unsafe { disclaim_reexec_args(&[&["disclaim-head"], &a[2..].iter().map(String::as_str).collect::<Vec<_>>()[..]].concat()) };
    }
    #[cfg(target_os = "macos")]
    if mode == "disclaim-head" && a.len() >= 4 {
        if !self_responsible() {
            std::process::exit(4);
        }
        let r = a[2].clone();
        let me = CString::new(std::env::current_exe().unwrap().as_os_str().as_encoded_bytes()).unwrap();
        unsafe {
            if libc::fork() == 0 {
                let args = [me.clone(), CString::new("sigcount").unwrap(), CString::new(format!("{r}.kid")).unwrap()];
                let argv = [args[0].as_ptr(), args[1].as_ptr(), args[2].as_ptr(), std::ptr::null()];
                libc::execv(me.as_ptr(), argv.as_ptr());
                libc::_exit(127);
            }
            // the kid is recorded (by itself) before the head goes on
            let kid = format!("{r}.kid");
            let mut n = 0;
            while std::fs::read_to_string(&kid).map_or(true, |t| t.is_empty()) && n < 1000 {
                libc::usleep(10_000);
                n += 1;
            }
            record(&r, libc::getpid());
            let prog: Vec<CString> = a[3..].iter().map(|s| CString::new(s.as_str()).unwrap()).collect();
            let mut ptrs: Vec<*const libc::c_char> = prog.iter().map(|c| c.as_ptr()).collect();
            ptrs.push(std::ptr::null());
            libc::execvp(ptrs[0], ptrs.as_ptr());
            libc::_exit(127);
        }
    }
    // P6 (suspects). `new-session PROG ARGS...`: setsid (a session and group the test made),
    // then exec PROG ARGS.
    if mode == "new-session" && a.len() >= 3 {
        let prog: Vec<CString> = a[2..].iter().map(|s| CString::new(s.as_str()).unwrap()).collect();
        let mut ptrs: Vec<*const libc::c_char> = prog.iter().map(|c| c.as_ptr()).collect();
        ptrs.push(std::ptr::null());
        unsafe {
            if libc::setsid() < 0 {
                libc::_exit(4); // already a group leader: the cell would test another shape
            }
            libc::execvp(ptrs[0], ptrs.as_ptr());
            libc::_exit(127);
        }
    }
    // `suspect-tree DIR [untagged]` (run as a session leader, under `new-session`): every
    // process is `sigcount DIR/<name>` (it records itself and counts its signals):
    //   early  an orphan in this session, started before the target (its parent exited);
    //   t      the target; it starts, 30 ms apart (so their start ticks differ on Linux):
    //     g    an orphan that stays in this session (its parent exited) — without its tag with
    //          `untagged`; it keeps a child, `gc` (no orphan: under a suspect);
    //     g2   an orphan in a session of its own (setsid);
    //     g3   (macOS) as g2, but responsible for itself (the disclaim);
    //   n      not an orphan: this leader's child, started after the target;
    //   leader this process, at the end.
    // Each orphan's parent waits until the orphan has recorded itself (after its exec), then
    // exits: the orphan never execs after it is reparented (macOS `puniq` stays its parent's).
    if mode == "suspect-tree" && (a.len() == 3 || a.len() == 4) {
        let dir = a[2].clone();
        let untagged = a.get(3).map(String::as_str) == Some("untagged");
        unsafe { suspect_tree(&dir, untagged) };
    }
    // P7 (strays). `stray DIR NAME [bare]`: the 2026-09-25 shape: an orphan `sigcount DIR/NAME`
    // (its parent exited after the orphan's exec; `bare`: without any environment, so untagged).
    if mode == "stray" && (a.len() == 4 || a.len() == 5) {
        let bare = a.get(4).map(String::as_str) == Some("bare");
        unsafe { orphan(&a[2], &a[3], false, || sigcount_as(&a[2], &a[3], bare)) };
        std::process::exit(0);
    }
    // `late-exec DIR NAME`: an orphan that execs only after it was reparented, so its `puniq`
    // resets to 1 (a launchd-started look, PLAN §3.0): `sigcount DIR/NAME`.
    if mode == "late-exec" && a.len() == 4 {
        unsafe {
            match libc::fork() {
                0 => {
                    if libc::fork() == 0 {
                        let mut n = 0;
                        while libc::getppid() != 1 && n < 5000 {
                            libc::usleep(1000);
                            n += 1;
                        }
                        sigcount_as(&a[2], &a[3], false);
                    }
                    libc::_exit(0);
                }
                c if c > 0 => {
                    libc::waitpid(c, std::ptr::null_mut(), 0);
                }
                _ => std::process::exit(1),
            }
        }
        wait_record(&a[2], &a[3]);
        std::process::exit(0);
    }
    // `stray-run DIR NAME PROG ARGS...`: an orphan (recorded in DIR/NAME.d) that runs PROG ARGS as
    // its child, writes the exit code to DIR/NAME.code, then becomes `sigcount DIR/NAME`.
    if mode == "stray-run" && a.len() >= 5 {
        let me = std::env::current_exe().unwrap();
        let mut args: Vec<String> = vec!["run-then-count".into(), a[2].clone(), a[3].clone()];
        args.extend(a[4..].iter().cloned());
        unsafe {
            orphan(&a[2], &format!("{}.d", a[3]), false, || {
                let c: Vec<CString> = std::iter::once(CString::new(me.as_os_str().as_encoded_bytes()).unwrap()).chain(args.iter().map(|s| CString::new(s.as_str()).unwrap())).collect();
                let mut p: Vec<*const libc::c_char> = c.iter().map(|x| x.as_ptr()).collect();
                p.push(std::ptr::null());
                libc::execv(c[0].as_ptr(), p.as_ptr());
            })
        };
        std::process::exit(0);
    }
    if mode == "run-then-count" && a.len() >= 5 {
        let first = unsafe { libc::getppid() };
        record(&format!("{}/{}.d", a[2], a[3]), std::process::id() as i32);
        // an orphan first (bounded, 10 s): its parent exits once the record above exists, and it
        // is adopted (by PID 1 or a subreaper)
        let mut n = 0;
        while unsafe { libc::getppid() } == first && n < 1000 {
            unsafe { libc::usleep(10_000) };
            n += 1;
        }
        let code = std::process::Command::new(&a[4]).args(&a[5..]).status().ok().and_then(|s| s.code()).map_or("none".to_string(), |c| c.to_string());
        let _ = std::fs::write(format!("{}/{}.code", a[2], a[3]), code);
        unsafe { sigcount_as(&a[2], &a[3], false) };
    }
    // `app DIR HELPER` (macOS; run from an executable inside an `.app` bundle): become
    // responsible for itself (as an app launched by launchd is), start a double-forked helper
    // `HELPER sigcount DIR/helper` (an executable inside the same bundle), then become
    // `sigcount DIR/app`.
    #[cfg(target_os = "macos")]
    if mode == "app" && a.len() == 4 {
        unsafe { disclaim_reexec_args(&["app-d", &a[2], &a[3]]) };
    }
    #[cfg(target_os = "macos")]
    if mode == "app-d" && a.len() == 4 {
        if !self_responsible() {
            std::process::exit(4);
        }
        let helper = CString::new(a[3].as_str()).unwrap();
        let (sc, rec) = (CString::new("sigcount").unwrap(), CString::new(format!("{}/helper", a[2])).unwrap());
        unsafe {
            orphan(&a[2], "helper", true, || {
                let argv = [helper.as_ptr(), sc.as_ptr(), rec.as_ptr(), std::ptr::null()];
                libc::execv(helper.as_ptr(), argv.as_ptr());
            });
            sigcount_as(&a[2], "app", false);
        }
    }
    // `reaper NAME READY PROG ARGS...` (Linux): a subreaper whose command name is NAME (as `tini`
    // is): it runs PROG ARGS as its child and creates the file READY once that child has ended
    // (its orphans are adopted by then), then reaps every orphan it adopts until none is left.
    #[cfg(target_os = "linux")]
    if mode == "reaper" && a.len() >= 5 {
        let name = CString::new(a[2].as_str()).unwrap();
        unsafe {
            libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0);
            libc::prctl(libc::PR_SET_NAME, name.as_ptr() as libc::c_ulong, 0, 0, 0);
            // it dies with the test that started it
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0);
        }
        if let Ok(mut ch) = std::process::Command::new(&a[4]).args(&a[5..]).spawn() {
            let _ = ch.wait();
        }
        let _ = std::fs::write(&a[3], b"");
        // no descendant left: nothing more can be adopted
        while unsafe { libc::waitpid(-1, std::ptr::null_mut(), 0) } >= 0 {}
        std::process::exit(0);
    }
    // P8. `t R PROG ARGS...` (macOS): the test-made terminal T (PHASE2 §0): re-exec itself with
    // the responsibility disclaim (as `t-run`), so it is responsible for itself; record itself in
    // R; run PROG ARGS as its child (a "tab"), wait for it and exit with its code. Exit 4 if the
    // disclaim did not take effect.
    #[cfg(target_os = "macos")]
    if mode == "t" && a.len() >= 4 {
        unsafe { disclaim_reexec_args(&[&["t-run"], &a[2..].iter().map(String::as_str).collect::<Vec<_>>()[..]].concat()) };
    }
    #[cfg(target_os = "macos")]
    if mode == "t-run" && a.len() >= 4 {
        if !self_responsible() {
            std::process::exit(4);
        }
        record(&a[2], std::process::id() as i32);
        let code = std::process::Command::new(&a[3]).args(&a[4..]).status().ok().and_then(|s| s.code()).unwrap_or(1);
        std::process::exit(code);
    }
    // `setsid-kid-fx R`: cell 2 with fixture processes only (an inherit-mode cell latches the
    // wall, and a platform binary's environment cannot be read): the root forks C (recorded in
    // R); C starts a new session, forks G (`sigcount R.g`) and waits; the root waits.
    if mode == "setsid-kid-fx" && a.len() == 3 {
        let r = a[2].clone();
        let (dir, name) = r.rsplit_once('/').map_or((".".to_string(), r.clone()), |(d, n)| (d.to_string(), n.to_string()));
        unsafe {
            match libc::fork() {
                0 => {
                    libc::setsid();
                    if libc::fork() == 0 {
                        sigcount_as(&dir, &format!("{name}.g"), false);
                    }
                    loop {
                        libc::pause();
                    }
                }
                c if c > 0 => {
                    record(&r, c);
                    loop {
                        libc::pause();
                    }
                }
                _ => std::process::exit(1),
            }
        }
    }
    // `escape-after R GO`: the root forks C (recorded in R); C waits for the file GO (bounded,
    // 20 s: the cell writes it after the supervisor's first scans saw C), then starts a new
    // session, forks G (`sigcount R.g`, which records itself after its exec) and exits only once
    // G has recorded itself (so G never execs after it is reparented: its `puniq` stays C); the
    // root waits.
    if mode == "escape-after" && a.len() == 4 {
        let (r, go) = (a[2].clone(), a[3].clone());
        unsafe {
            match libc::fork() {
                0 => {
                    let mut n = 0;
                    while !std::path::Path::new(&go).exists() && n < 2000 {
                        libc::usleep(10_000);
                        n += 1;
                    }
                    libc::setsid();
                    if libc::fork() == 0 {
                        let (dir, name) = r.rsplit_once('/').unwrap_or((".", &r));
                        sigcount_as(dir, &format!("{name}.g"), false);
                    }
                    wait_record(&r.rsplit_once('/').map_or(".".to_string(), |x| x.0.to_string()), &format!("{}.g", r.rsplit_once('/').map_or(r.as_str(), |x| x.1)));
                    libc::_exit(0);
                }
                c if c > 0 => {
                    record(&r, c);
                    loop {
                        libc::pause();
                    }
                }
                _ => std::process::exit(1),
            }
        }
    }
    // `after GO PROG ARGS...`: wait until the file GO exists (bounded, 60 s), then exec PROG ARGS.
    if mode == "after" && a.len() >= 4 {
        let mut n = 0;
        while !std::path::Path::new(&a[2]).exists() && n < 6000 {
            unsafe { libc::usleep(10_000) };
            n += 1;
        }
        let prog: Vec<CString> = a[3..].iter().map(|s| CString::new(s.as_str()).unwrap()).collect();
        let mut ptrs: Vec<*const libc::c_char> = prog.iter().map(|c| c.as_ptr()).collect();
        ptrs.push(std::ptr::null());
        unsafe {
            libc::execvp(ptrs[0], ptrs.as_ptr());
            libc::_exit(127);
        }
    }
    // `print-env VAR OUT [M]`: write VAR's value (empty if unset) to OUT.tmp, rename it to OUT,
    // then run `/bin/sleep M` if M is given.
    if mode == "print-env" && (a.len() == 4 || a.len() == 5) {
        let v = std::env::var(&a[2]).unwrap_or_default();
        let tmp = format!("{}.tmp", a[3]);
        let _ = std::fs::write(&tmp, v);
        let _ = std::fs::rename(&tmp, &a[3]);
        if let Some(m) = a.get(4) {
            unsafe { exec_sleep(&CString::new(m.as_str()).unwrap()) };
        }
        std::process::exit(0);
    }
    // `register SRC IDX CLAIM KIND OUT [M]`: register with entry IDX of a registration chain
    // (SRC: `env` for SHEEPDOG_OUTER, else a file holding the chain's text), claiming pid CLAIM
    // (`self` for its own), sending a well-formed record (`good`) or one with a bad magic
    // (`bad`). Creates OUT.conn once connected and sent. Writes `ack`, `refused`, `none` (no answer in
    // 2 s, or SD_REG_TIMEOUT_MS) or `err <why>` to OUT (via a rename), then runs `/bin/sleep M` if M is given, else exits 0 on `ack` and 1 otherwise.
    if mode == "register" && (a.len() == 7 || a.len() == 8) {
        let text = if a[2] == "env" { std::env::var(sheepdog::regwire::VAR).unwrap_or_default() } else { std::fs::read_to_string(&a[2]).unwrap_or_default() };
        let (chain, _) = sheepdog::regwire::parse(&text);
        let idx: usize = a[3].parse().unwrap_or(usize::MAX);
        let claim = if a[4] == "self" { std::process::id() as i32 } else { a[4].parse().unwrap_or(0) };
        let res = match chain.get(idx) {
            None => "err no such entry".to_string(),
            Some(e) => {
                let mut rec = sheepdog::regwire::record(&e.nonce, claim);
                if a[5] == "bad" {
                    rec[0] = b'X';
                }
                reg_client(&e.path, &rec, &format!("{}.conn", a[6]))
            }
        };
        let tmp = format!("{}.tmp", a[6]);
        let _ = std::fs::write(&tmp, &res);
        let _ = std::fs::rename(&tmp, &a[6]);
        if let Some(m) = a.get(7) {
            unsafe { exec_sleep(&CString::new(m.as_str()).unwrap()) };
        }
        std::process::exit(if res == "ack" { 0 } else { 1 });
    }
    // `silent SRC N READY`: open N connections to entry 0 of the chain and send nothing; write
    // the count to READY; then become `sigcount READY.hold`, which keeps them open.
    if mode == "silent" && a.len() == 5 {
        let text = if a[2] == "env" { std::env::var(sheepdog::regwire::VAR).unwrap_or_default() } else { std::fs::read_to_string(&a[2]).unwrap_or_default() };
        let (chain, _) = sheepdog::regwire::parse(&text);
        let n: usize = a[3].parse().unwrap_or(0);
        let mut held = Vec::new();
        if let Some(e) = chain.first() {
            for _ in 0..n {
                if let Ok(s) = std::os::unix::net::UnixStream::connect(&e.path) {
                    held.push(s);
                }
            }
        }
        let _ = std::fs::write(&a[4], format!("{}", held.len()));
        // the connections survive the exec (UnixStream is CLOEXEC: clear it)
        for s in &held {
            use std::os::fd::AsRawFd;
            unsafe { libc::fcntl(s.as_raw_fd(), libc::F_SETFD, 0) };
        }
        std::mem::forget(held);
        let me = CString::new(std::env::current_exe().unwrap().as_os_str().as_encoded_bytes()).unwrap();
        let args = [me.clone(), CString::new("sigcount").unwrap(), CString::new(format!("{}.hold", a[4])).unwrap()];
        let argv = [args[0].as_ptr(), args[1].as_ptr(), args[2].as_ptr(), std::ptr::null()];
        unsafe {
            libc::execv(me.as_ptr(), argv.as_ptr());
            libc::_exit(127);
        }
    }
    #[cfg(target_os = "macos")]
    if mode == "redisclaim-c" && a.len() == 4 {
        // C after its disclaim re-exec: start GG, live SD_C_LIFE_MS (default 700 ms), exit.
        // Exit 4 if the disclaim did not take effect (the cell would pass for another reason).
        if !self_responsible() {
            std::process::exit(4);
        }
        let m = CString::new(a[2].as_str()).unwrap();
        let life: u32 = std::env::var("SD_C_LIFE_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(700);
        unsafe {
            if libc::fork() == 0 {
                libc::setsid();
                let g = libc::getpid();
                if libc::fork() == 0 {
                    // exec only after G has exited (then `puniq` resets to 1), bounded
                    let mut n = 0;
                    while libc::getppid() == g && n < 2000 {
                        libc::usleep(1000);
                        n += 1;
                    }
                    record(&a[3], libc::getpid());
                    exec_sleep(&m);
                }
                libc::_exit(0);
            }
            libc::usleep(life * 1000);
            libc::_exit(0);
        }
    }
    #[cfg(target_os = "macos")]
    if mode == "puniq-d" && a.len() == 4 {
        // D after its disclaim re-exec: record itself only if responsible for itself
        if !self_responsible() {
            std::process::exit(4);
        }
        record(&a[3], unsafe { libc::getpid() });
        let m = CString::new(a[2].as_str()).unwrap();
        unsafe { exec_sleep(&m) }
    }
    // `dspawn [wait] PROG ARGS...` (macOS): posix_spawn PROG with the responsibility disclaim
    // (PROG is responsible for itself); with `wait`, wait for it and exit with its code, else
    // exit at once. Exit 3 if the disclaim or the spawn is not available.
    #[cfg(target_os = "macos")]
    if mode == "dspawn" && a.len() >= 3 {
        let wait = a[2] == "wait";
        let rest = &a[if wait { 3 } else { 2 }..];
        type Disclaim = unsafe extern "C" fn(*mut libc::posix_spawnattr_t, libc::c_int) -> libc::c_int;
        extern "C" {
            fn _NSGetEnviron() -> *mut *const *const libc::c_char;
        }
        unsafe {
            let name = CString::new("responsibility_spawnattrs_setdisclaim").unwrap();
            let f = libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr());
            if f.is_null() || rest.is_empty() {
                libc::_exit(3);
            }
            let args: Vec<CString> = rest.iter().map(|s| CString::new(s.as_str()).unwrap()).collect();
            let mut ptrs: Vec<*mut libc::c_char> = args.iter().map(|c| c.as_ptr() as *mut libc::c_char).collect();
            ptrs.push(std::ptr::null_mut());
            let mut attr: libc::posix_spawnattr_t = std::mem::zeroed();
            libc::posix_spawnattr_init(&mut attr);
            let d: Disclaim = std::mem::transmute(f);
            d(&mut attr, 1);
            let mut pid = 0;
            let rc = libc::posix_spawn(&mut pid, args[0].as_ptr(), std::ptr::null(), &attr, ptrs.as_ptr(), *_NSGetEnviron() as *const *mut libc::c_char);
            if rc != 0 {
                libc::_exit(3);
            }
            // the disclaim must have taken effect, or a cell would pass for another reason
            type RespUniq = unsafe extern "C" fn(libc::pid_t) -> u64;
            let rn = CString::new("responsibility_get_uniqueid_responsible_for_pid").unwrap();
            let rf = libc::dlsym(libc::RTLD_DEFAULT, rn.as_ptr());
            if rf.is_null() || identity(pid).map_or(true, |u| (std::mem::transmute::<_, RespUniq>(rf))(pid) != u) {
                libc::kill(pid, libc::SIGKILL);
                libc::_exit(4);
            }
            if wait {
                let mut st = 0;
                libc::waitpid(pid, &mut st, 0);
                libc::_exit(if libc::WIFEXITED(st) { libc::WEXITSTATUS(st) } else { 128 + libc::WTERMSIG(st) });
            }
            libc::_exit(0);
        }
    }
    // escape-late M R: C forks G (records itself, becomes `/bin/sleep M`), C lives 600 ms and
    // exits; the root waits for C. So G is a descendant for over two scan ticks, then is
    // reparented away (to init without a subreaper): only tracking while running catches it.
    if mode == "escape-late" && a.len() == 4 {
        let m = CString::new(a[2].as_str()).unwrap();
        unsafe {
            let c = libc::fork();
            if c == 0 {
                if libc::fork() == 0 {
                    record(&a[3], libc::getpid());
                    exec_sleep(&m);
                }
                libc::usleep(600_000);
                libc::_exit(0);
            }
            let mut st = 0;
            libc::waitpid(c, &mut st, 0);
            libc::_exit(0);
        }
    }
    if (mode == "redisclaim" || mode == "puniq-only") && a.len() == 4 {
        #[cfg(target_os = "macos")]
        unsafe {
            let next = if mode == "redisclaim" { "redisclaim-c" } else { "puniq-d" };
            let c = libc::fork();
            if c == 0 {
                if mode == "redisclaim" {
                    libc::usleep(700_000); // be seen as a member first
                }
                disclaim_reexec(next, &a[2], &a[3]);
            }
            if mode == "redisclaim" {
                let mut st = 0;
                libc::waitpid(c, &mut st, 0);
                if !libc::WIFEXITED(st) || libc::WEXITSTATUS(st) != 0 {
                    libc::_exit(3); // C failed (no disclaim, or not responsible for itself)
                }
                // outlive C, so GG has become responsible for itself (the change is not
                // immediate) before the kill starts, not during it
                libc::usleep(400_000);
            } else {
                libc::usleep(700_000);
            }
            libc::_exit(0);
        }
        #[cfg(not(target_os = "macos"))]
        {
            eprintln!("sd-fixture: {mode} is macOS-only");
            std::process::exit(2);
        }
    }
    if (mode == "term-logger" || mode == "term-counter") && (a.len() == 4 || (a.len() == 5 && a[4] == "stop")) {
        let term_file = CString::new(format!("{}.term", a[3])).unwrap();
        let stop = a.len() == 5;
        unsafe {
            let fd = libc::open(term_file.as_ptr(), libc::O_WRONLY | libc::O_CREAT | libc::O_APPEND, 0o644);
            TERM_FD = fd;
            // installed before the fork, so the escapee has it from its first instruction (a TERM
            // that came before a handler installed in the escapee would kill it unrecorded)
            let h = if mode == "term-logger" { on_term as *const () as usize } else { count_term as *const () as usize };
            libc::signal(libc::SIGTERM, h);
            if let Some(g) = spawn_escapee(&a[3], true, || {
                let mut none: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut none);
                libc::sigprocmask(libc::SIG_SETMASK, &none, std::ptr::null_mut());
                if stop {
                    libc::raise(libc::SIGSTOP);
                }
                loop {
                    libc::pause();
                }
            }) {
                record(&a[3], g);
                // the root's exit starts the kill: in stop mode the escapee must be stopped by
                // then, or its TERM would come before the stop and the CONT would not be tested
                let mut waited = 0;
                while stop && !is_stopped(g) {
                    if waited > 2000 {
                        libc::_exit(3); // the escapee never stopped: fail loudly, never hang
                    }
                    libc::usleep(1000);
                    waited += 1;
                }
            }
            libc::_exit(0);
        }
    }
    // decoy F: a process that is NOT in any job; it logs every catchable signal it receives to
    // F.log (one number per line), then writes its pid to F and waits. A SIGSTOP shows as state
    // T, a SIGKILL as its death.
    if mode == "decoy" && a.len() == 3 {
        let log = CString::new(format!("{}.log", a[2])).unwrap();
        unsafe {
            DECOY_FD = libc::open(log.as_ptr(), libc::O_WRONLY | libc::O_CREAT | libc::O_APPEND, 0o644);
            for s in [libc::SIGTERM, libc::SIGCONT, libc::SIGHUP, libc::SIGINT, libc::SIGQUIT, libc::SIGUSR1, libc::SIGUSR2, libc::SIGALRM] {
                libc::signal(s, log_signal as *const () as usize);
            }
            let mut none: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut none);
            libc::sigprocmask(libc::SIG_SETMASK, &none, std::ptr::null_mut());
        }
        let _ = std::fs::write(&a[2], format!("{}\n", std::process::id()));
        loop {
            unsafe { libc::pause() };
        }
    }
    if mode == "print-pending" {
        // the pending signals of this process, one number per line
        unsafe {
            let mut p: libc::sigset_t = std::mem::zeroed();
            libc::sigpending(&mut p);
            for sig in 1..32 {
                if libc::sigismember(&p, sig) == 1 {
                    println!("{sig}");
                }
            }
        }
        std::process::exit(0);
    }
    if mode == "print-mask" {
        // the blocked signals of this process, one number per line (for cell 23's mask check)
        unsafe {
            let mut m: libc::sigset_t = std::mem::zeroed();
            libc::sigprocmask(libc::SIG_BLOCK, std::ptr::null(), &mut m);
            for sig in 1..32 {
                if libc::sigismember(&m, sig) == 1 {
                    println!("{sig}");
                }
            }
        }
        std::process::exit(0);
    }
    if mode == "counter" && a.len() == 4 {
        let sig_file = CString::new(format!("{}.sig", a[3])).unwrap();
        unsafe {
            SIG_FD = libc::open(sig_file.as_ptr(), libc::O_WRONLY | libc::O_CREAT | libc::O_APPEND, 0o644);
            // installed before the fork, so the escapee counts from its first instruction
            on(libc::SIGINT, count_sig as *const () as usize, true);
            on(libc::SIGHUP, count_sig as *const () as usize, true);
            let g = spawn_escapee(&a[3], true, || {
                unblock_all();
                loop {
                    libc::pause();
                }
            });
            match g {
                Some(g) => record(&a[3], g),
                None => libc::_exit(3),
            }
            record(&a[3], libc::getpid());
            unblock_all();
            loop {
                libc::pause();
            }
        }
    }
    if mode == "ticker" && (a.len() == 4 || a.len() == 5 || a.len() == 6) {
        // regroup-tstp sends TSTP to its whole group: only when its parent (sheepdog) leads that
        // group, never in a group it did not make (a test runner's: that would stop the runner)
        if a.get(4).map(String::as_str) == Some("regroup-tstp") && !(unsafe { libc::getpgrp() == libc::getppid() } && parent_is_sheepdog()) {
            eprintln!("sd-fixture: regroup-tstp refused: the parent does not lead this process group");
            std::process::exit(5);
        }
        let tick = CString::new(format!("{}.tick", a[3])).unwrap();
        let rec = a[3].clone();
        unsafe {
            unblock_all();
            // a bounded life: every ticker process ends after 60 s while running (each sets its
            // own alarm: fork clears it); a stopped one is ended by the test's SIGKILL
            libc::signal(libc::SIGALRM, libc::SIG_DFL);
            libc::alarm(60);
            let g = spawn_escapee(&rec, true, || {
                libc::alarm(60);
                // breed: up to 4 ticking children, one every 200 ms, each recorded
                let tick2 = tick.clone();
                let rec2 = rec.clone();
                match libc::fork() {
                    0 => tick_forever(&tick2),
                    c if c > 0 => record(&rec2, c),
                    _ => {}
                }
                for _ in 0..4 {
                    libc::usleep(200_000);
                    match libc::fork() {
                        0 => tick_forever(&tick2),
                        c if c > 0 => record(&rec2, c),
                        _ => {}
                    }
                }
                loop {
                    libc::pause();
                }
            });
            match g {
                Some(g) => record(&a[3], g),
                None => libc::_exit(3),
            }
            match a.get(4).map(String::as_str) {
                Some("slow-tstp") if a.len() == 6 => {
                    let b = a[5].as_bytes();
                    // NUL-terminated by the zeroed static (paths are far below 512 bytes)
                    std::ptr::copy_nonoverlapping(b.as_ptr(), std::ptr::addr_of_mut!(TSTP_FILE) as *mut u8, b.len().min(511));
                    on(libc::SIGTSTP, slow_tstp as *const () as usize, true);
                }
                Some("regroup-tstp") => on(libc::SIGTSTP, regroup_tstp as *const () as usize, true),
                Some("fork-on-tstp") => {
                    let t = format!("{}.tick", a[3]);
                    std::ptr::copy_nonoverlapping(t.as_ptr(), std::ptr::addr_of_mut!(FORK_TICK) as *mut u8, t.len().min(511));
                    std::ptr::copy_nonoverlapping(a[3].as_ptr(), std::ptr::addr_of_mut!(FORK_REC) as *mut u8, a[3].len().min(511));
                    on(libc::SIGTSTP, fork_on_tstp as *const () as usize, true);
                }
                _ => {}
            }
            record(&a[3], libc::getpid());
            // ready: the root names itself in <R>.root (the escapee's children keep recording)
            record(&format!("{}.root", a[3]), libc::getpid());
            match a.get(4).map(String::as_str) {
                Some("read") => {
                    let mut b = [0u8; 64];
                    loop {
                        libc::read(0, b.as_mut_ptr() as *mut libc::c_void, b.len());
                    }
                }
                Some("write") => loop {
                    libc::write(1, b"x\n".as_ptr() as *const libc::c_void, 2);
                    libc::usleep(50_000);
                },
                _ => loop {
                    libc::pause();
                },
            }
        }
    }
    if mode == "shell" && a.len() >= 4 {
        let mut i = 3;
        let (mut bg, mut null_stdin, mut tostop) = (false, false, false);
        while i < a.len() && (a[i] == "bg" || a[i] == "null-stdin" || a[i] == "tostop") {
            bg |= a[i] == "bg";
            null_stdin |= a[i] == "null-stdin";
            tostop |= a[i] == "tostop";
            i += 1;
        }
        if tostop {
            // a background job that writes to the terminal gets TTOU
            unsafe {
                let mut t: libc::termios = std::mem::zeroed();
                libc::tcgetattr(0, &mut t);
                t.c_lflag |= libc::TOSTOP;
                libc::tcsetattr(0, libc::TCSANOW, &t);
            }
        }
        if i >= a.len() {
            usage();
        }
        shell(&a[2], bg, null_stdin, &a[i..]);
    }
    if mode == "nosession" && a.len() >= 4 {
        use std::os::unix::process::CommandExt;
        unsafe {
            libc::setsid();
            let mut cmd = std::process::Command::new(&a[3]);
            cmd.args(&a[4..]).process_group(0);
            let mut c = cmd.spawn().unwrap_or_else(|e| {
                eprintln!("sd-fixture: spawn failed: {e}");
                std::process::exit(127)
            });
            // "<pid> <identity>", read before this process can reap it
            let _ = std::fs::write(&a[2], format!("{} {}\n", c.id(), identity(c.id() as i32).unwrap_or(0)));
            let st = c.wait().map(|s| std::os::unix::process::ExitStatusExt::into_raw(s)).unwrap_or(0);
            if libc::WIFSIGNALED(st) {
                let sig = libc::WTERMSIG(st);
                libc::signal(sig, libc::SIG_DFL);
                unblock_all();
                libc::raise(sig);
            }
            libc::_exit(libc::WEXITSTATUS(st));
        }
    }
    if mode == "int-exit" && a.len() == 5 {
        unsafe {
            INT_EXIT = a[2].parse().unwrap_or(1);
            on(libc::SIGINT, int_exit as *const () as usize, true);
            unblock_all();
            // ready only once the handler is in place: an earlier INT would kill it by default
            let _ = std::fs::write(&a[4], "");
            // no exec (it would reset the handler); the marker stays in this argv
            loop {
                libc::pause();
            }
        }
    }
    if (mode == "bg-then-exec" || mode == "bg-apart-then-exec") && a.len() >= 4 {
        use std::os::unix::process::CommandExt;
        let m = CString::new(a[2].as_str()).unwrap();
        unsafe {
            if libc::fork() == 0 {
                if mode == "bg-apart-then-exec" {
                    libc::setpgid(0, 0); // the background job leaves the caller's group
                }
                let null = libc::open(b"/dev/null\0".as_ptr() as *const libc::c_char, libc::O_RDWR);
                libc::dup2(null, 1);
                libc::dup2(null, 2);
                exec_sleep(&m);
            }
        }
        let e = std::process::Command::new(&a[3]).args(&a[4..]).exec();
        eprintln!("sd-fixture: exec failed: {e}");
        std::process::exit(127);
    }
    if mode == "exec-chld-ignored" && a.len() >= 3 {
        use std::os::unix::process::CommandExt;
        unsafe { libc::signal(libc::SIGCHLD, libc::SIG_IGN) };
        let e = std::process::Command::new(&a[2]).args(&a[3..]).exec();
        eprintln!("sd-fixture: exec failed: {e}");
        std::process::exit(127);
    }
    if mode == "alloc" && a.len() == 5 {
        let mbps: usize = a[2].parse().unwrap_or_else(|_| usage());
        let max: usize = a[3].parse().unwrap_or_else(|_| usage());
        let progress = a[4].clone();
        unsafe { libc::alarm(60) };
        let mut held: Vec<Vec<u8>> = Vec::new();
        let step = (mbps * 1024 * 1024 / 50).max(4096);
        let mut total = 0usize;
        let mut seed: u8 = 1;
        while total < max * 1024 * 1024 {
            let mut v = vec![0u8; step];
            for i in (0..step).step_by(4096) {
                seed = seed.wrapping_mul(31).wrapping_add(7);
                v[i] = seed;
            }
            total += step;
            held.push(v);
            let _ = std::fs::write(&progress, format!("{}\n", total / (1024 * 1024)));
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        loop {
            unsafe { libc::pause() };
        }
    }
    if mode == "alloc-on-term" && a.len() == 4 {
        let mb: usize = a[2].parse().unwrap_or_else(|_| usage());
        ALLOC_ON_TERM_MB.store(mb, std::sync::atomic::Ordering::SeqCst);
        let p = a[3].as_bytes();
        unsafe {
            std::ptr::copy_nonoverlapping(p.as_ptr(), std::ptr::addr_of_mut!(TERM_REC) as *mut u8, p.len().min(511));
            libc::alarm(60);
            on(libc::SIGTERM, alloc_on_term as *const () as usize, true);
            unblock_all();
        }
        let _ = std::fs::write(&a[3], "0\n");
        loop {
            unsafe { libc::pause() };
            if ALLOC_ON_TERM_GOT.load(std::sync::atomic::Ordering::SeqCst) {
                let n = mb * 1024 * 1024;
                let mut v = vec![0u8; n];
                let mut seed: u8 = 3;
                for i in (0..n).step_by(4096) {
                    seed = seed.wrapping_mul(31).wrapping_add(7);
                    v[i] = seed;
                }
                let _ = std::fs::write(&a[3], format!("{mb}\n"));
                std::mem::forget(v);
                ALLOC_ON_TERM_GOT.store(false, std::sync::atomic::Ordering::SeqCst);
            }
        }
    }
    if mode == "forker" && a.len() == 5 {
        let n: u32 = a[2].parse().unwrap_or_else(|_| usage());
        let every: u32 = a[3].parse().unwrap_or_else(|_| usage());
        unsafe {
            libc::alarm(60);
            for _ in 0..n {
                match libc::fork() {
                    0 => {
                        record(&a[4], libc::getpid());
                        libc::alarm(60);
                        loop {
                            libc::pause();
                        }
                    }
                    -1 => std::process::exit(3),
                    _ => {
                        libc::usleep(every * 1000);
                    }
                }
            }
            loop {
                libc::pause();
            }
        }
    }
    if mode == "escapee-and-wait" && (a.len() == 3 || a.len() == 4) {
        let me = std::env::current_exe().unwrap();
        let exe = CString::new(me.as_os_str().as_encoded_bytes()).unwrap();
        let r = a[2].clone();
        if let Some(go) = a.get(3) {
            let mut n = 0;
            while !std::path::Path::new(go).exists() && n < 6000 {
                unsafe { libc::usleep(10_000) };
                n += 1;
            }
        }
        unsafe {
            match libc::fork() {
                0 => {
                    libc::setsid();
                    if libc::fork() == 0 {
                        record(&r, libc::getpid());
                        let args = [exe.clone(), CString::new("sigcount").unwrap(), CString::new(format!("{r}.g")).unwrap()];
                        let mut p: Vec<*const libc::c_char> = args.iter().map(|c| c.as_ptr()).collect();
                        p.push(std::ptr::null());
                        libc::execv(exe.as_ptr(), p.as_ptr());
                        libc::_exit(127);
                    }
                    libc::_exit(0);
                }
                -1 => std::process::exit(1),
                c => {
                    let mut st = 0;
                    libc::waitpid(c, &mut st, 0);
                }
            }
            let mut n = 0;
            while std::fs::metadata(&r).map_or(true, |m| m.len() == 0) && n < 1000 {
                libc::usleep(10_000);
                n += 1;
            }
            let args = [exe.clone(), CString::new("sigcount").unwrap(), CString::new(format!("{r}.root")).unwrap()];
            let mut p: Vec<*const libc::c_char> = args.iter().map(|c| c.as_ptr()).collect();
            p.push(std::ptr::null());
            libc::execv(exe.as_ptr(), p.as_ptr());
            std::process::exit(127);
        }
    }
    if mode == "spawn-on" && a.len() == 4 {
        unsafe {
            libc::alarm(60);
            record(&a[3], libc::getpid());
            let mut n = 0;
            while !std::path::Path::new(&a[2]).exists() && n < 6000 {
                libc::usleep(10_000);
                n += 1;
            }
            if libc::fork() == 0 {
                record(&a[3], libc::getpid());
                libc::alarm(60);
                loop {
                    libc::pause();
                }
            }
            loop {
                libc::pause();
            }
        }
    }
    if mode == "lineage-kill" && a.len() == 6 {
        let (r, go1, go2, rc) = (a[2].clone(), a[3].clone(), a[4].clone(), a[5].clone());
        let wait_file = |p: &str| {
            let mut n = 0;
            while !std::path::Path::new(p).exists() && n < 6000 {
                unsafe { libc::usleep(10_000) };
                n += 1;
            }
        };
        unsafe {
            libc::alarm(60);
            let w = libc::getpid();
            record(&r, w);
            if libc::fork() == 0 {
                if libc::fork() == 0 {
                    record(&r, libc::getpid()); // E
                    wait_file(&go2);
                    if !std::path::Path::new(&go2).exists() {
                        libc::_exit(3); // the test gave up: never run the kill late
                    }
                    let err = std::fs::File::create(format!("{rc}.err")).ok();
                    let mut cmd = std::process::Command::new("sheepdog");
                    cmd.args(["kill", &w.to_string()]);
                    if let Some(e) = err {
                        cmd.stderr(e);
                    }
                    let st = cmd.status();
                    let code = st.ok().and_then(|s| s.code()).map_or("none".to_string(), |c| c.to_string());
                    let _ = std::fs::write(&rc, format!("{code}\n"));
                    libc::alarm(60);
                    loop {
                        libc::pause();
                    }
                }
                let mut n = 0;
                while std::fs::read_to_string(&r).map_or(0, |t| t.lines().count()) < 2 && n < 2000 {
                    libc::usleep(5_000);
                    n += 1;
                }
                record(&r, libc::getpid()); // C
                wait_file(&go1);
                libc::_exit(0);
            }
            loop {
                libc::pause();
            }
        }
    }
    if mode == "worker" && a.len() == 4 {
        let (r, go) = (a[2].clone(), a[3].clone());
        unsafe {
            libc::alarm(60);
            record(&r, libc::getpid());
            if libc::fork() == 0 {
                // C
                if libc::fork() == 0 {
                    record(&r, libc::getpid()); // E
                    libc::alarm(60);
                    loop {
                        libc::pause();
                    }
                }
                let mut n = 0;
                while std::fs::metadata(&r).map_or(0, |m| m.len()) == 0 || std::fs::read_to_string(&r).map_or(0, |t| t.lines().count()) < 2 {
                    libc::usleep(5_000);
                    n += 1;
                    if n > 2000 {
                        break;
                    }
                }
                record(&r, libc::getpid()); // C, last
                let mut n = 0;
                while !std::path::Path::new(&go).exists() && n < 6000 {
                    libc::usleep(10_000);
                    n += 1;
                }
                libc::_exit(0);
            }
            loop {
                libc::pause();
            }
        }
    }
    if mode == "doublefork" && a.len() == 4 {
        let m = CString::new(a[2].as_str()).unwrap();
        unsafe {
            match libc::fork() {
                0 => {
                    if libc::fork() == 0 {
                        // wait until C is gone, so G is reparented before it records itself
                        let c = libc::getppid();
                        let mut n = 0;
                        while libc::getppid() == c && n < 2000 {
                            libc::usleep(1000);
                            n += 1;
                        }
                        record(&a[3], libc::getpid());
                        exec_sleep(&m);
                    }
                    libc::_exit(0);
                }
                -1 => std::process::exit(1),
                c => {
                    let mut st = 0;
                    libc::waitpid(c, &mut st, 0);
                }
            }
            let mut n = 0;
            while std::fs::metadata(&a[3]).map_or(true, |m| m.len() == 0) && n < 1000 {
                libc::usleep(10_000);
                n += 1;
            }
        }
        std::process::exit(0);
    }
    if mode == "spawnp" && a.len() >= 3 {
        let argv: Vec<CString> = a[2..].iter().map(|s| CString::new(s.as_str()).unwrap()).collect();
        let mut ptrs: Vec<*mut libc::c_char> = argv.iter().map(|c| c.as_ptr() as *mut libc::c_char).collect();
        ptrs.push(std::ptr::null_mut());
        extern "C" {
            static environ: *const *mut libc::c_char;
        }
        let mut pid = 0;
        let rc = unsafe { libc::posix_spawnp(&mut pid, ptrs[0], std::ptr::null(), std::ptr::null(), ptrs.as_ptr(), environ) };
        if rc != 0 {
            std::process::exit(if rc == libc::ENOENT { 127 } else { 126 });
        }
        let mut st = 0;
        unsafe { libc::waitpid(pid, &mut st, 0) };
        std::process::exit(if libc::WIFEXITED(st) { libc::WEXITSTATUS(st) } else { 128 + libc::WTERMSIG(st) });
    }
    if mode == "swarm" && (a.len() == 4 || a.len() == 5) {
        let n: u32 = a[2].parse().unwrap_or_else(|_| usage());
        // with STEP_US, child i also exits by itself i*STEP_US after it starts: members that
        // are exiting while the kill freezes them
        let step: u32 = a.get(4).map(|s| s.parse().unwrap_or_else(|_| usage())).unwrap_or(0);
        let r = a[3].clone();
        unsafe {
            for i in 0..n {
                match libc::fork() {
                    0 => {
                        libc::alarm(60);
                        if step > 0 {
                            libc::usleep(i * step);
                            libc::_exit(0);
                        }
                        if i % 2 == 0 {
                            SWARM_DELAY_US.store(i * 100, std::sync::atomic::Ordering::SeqCst);
                            on(libc::SIGTERM, swarm_term as *const () as usize, true);
                        } else {
                            libc::signal(libc::SIGTERM, libc::SIG_IGN);
                        }
                        unblock_all();
                        loop {
                            libc::pause();
                        }
                    }
                    c if c > 0 => record(&r, c),
                    _ => std::process::exit(3),
                }
            }
        }
        std::process::exit(0);
    }
    if mode == "sigcount" && a.len() == 3 {
        // S8 (another-user leg): record itself, then count every catchable signal (a line
        // `SIG <n>` in R.sig) and keep running; SIGALRM keeps its default action and ends it
        // after 1800 s at most
        let r = a[2].clone();
        let sig = format!("{r}.sig");
        let b = sig.as_bytes();
        unsafe {
            std::ptr::copy_nonoverlapping(b.as_ptr(), std::ptr::addr_of_mut!(DECOY_LOG) as *mut u8, b.len().min(511));
            for s in 1..32 {
                if s != libc::SIGKILL && s != libc::SIGSTOP && s != libc::SIGALRM {
                    on(s, decoy_sig as *const () as usize, true);
                }
            }
            unblock_all();
            record(&r, libc::getpid());
            libc::alarm(1800);
            loop {
                libc::pause();
            }
        }
    }
    if mode == "storm" && a.len() == 4 {
        // cell 7 (zombies): C (a new session) forks G every 5 ms for SECS seconds; each G forks
        // GG and exits at once, so GG is an orphan (adopted by the subreaper on Linux) and exits
        // 20 ms later. C reaps its own G's. Then this process creates DONE and waits (60 s at
        // most): the supervisor is still running when the test counts its zombies.
        let secs: u64 = a[3].parse().unwrap_or_else(|_| usage());
        let done = a[2].clone();
        unsafe {
            libc::alarm(60);
            match libc::fork() {
                0 => {
                    libc::setsid();
                    let end = std::time::Instant::now() + std::time::Duration::from_secs(secs);
                    while std::time::Instant::now() < end {
                        match libc::fork() {
                            0 => {
                                if libc::fork() == 0 {
                                    libc::usleep(20_000);
                                    libc::_exit(0);
                                }
                                libc::_exit(0);
                            }
                            _ => {}
                        }
                        let mut st = 0;
                        while libc::waitpid(-1, &mut st, libc::WNOHANG) > 0 {}
                        libc::usleep(5_000);
                    }
                    libc::_exit(0);
                }
                -1 => std::process::exit(1),
                c => {
                    let mut st = 0;
                    libc::waitpid(c, &mut st, 0);
                }
            }
            let _ = std::fs::File::create(&done);
            loop {
                libc::pause();
            }
        }
    }
    if mode == "escape-exec" && a.len() >= 4 {
        // as `escape`, but G execs PROG ARGS (after recording itself); the root records G
        let argv: Vec<CString> = a[3..].iter().map(|x| CString::new(x.as_str()).unwrap()).collect();
        let mut ptrs: Vec<*const libc::c_char> = argv.iter().map(|c| c.as_ptr()).collect();
        ptrs.push(std::ptr::null());
        let rec = a[2].clone();
        unsafe {
            if let Some(g) = spawn_escapee(&rec, true, || {
                record(&rec, libc::getpid());
                libc::execvp(ptrs[0], ptrs.as_ptr());
                libc::_exit(127);
            }) {
                record(&a[2], g);
            }
            // SD_EXEC_READY=<file>: exit only once that file is non-empty (the route has reached
            // its final program), 10 s at most
            if let Ok(f) = std::env::var("SD_EXEC_READY") {
                let mut n = 0;
                while std::fs::metadata(&f).map_or(true, |m| m.len() == 0) && n < 1000 {
                    libc::usleep(10_000);
                    n += 1;
                }
            }
        }
        std::process::exit(0);
    }
    unsafe {
        match (mode, a.len()) {
            ("escape", 4) | ("escape-fast", 4) => {
                let m = CString::new(a[2].as_str()).unwrap();
                let via_pipe = mode == "escape";
                if let Some(g) = spawn_escapee(&a[3], via_pipe, || { exec_sleep(&m); }) {
                    record(&a[3], g);
                }
            }
            ("deep", 5) => {
                // a live chain of N processes (this one first): each records itself, forks the
                // next and waits for it; the last runs `/bin/sleep M`
                let m = CString::new(a[2].as_str()).unwrap();
                let n: u32 = a[4].parse().unwrap_or_else(|_| usage());
                for _ in 1..n {
                    record(&a[3], libc::getpid());
                    match libc::fork() {
                        0 => continue,
                        -1 => libc::_exit(1),
                        c => {
                            let mut st = 0;
                            libc::waitpid(c, &mut st, 0);
                            libc::_exit(0);
                        }
                    }
                }
                record(&a[3], libc::getpid());
                exec_sleep(&m);
            }
            ("setsid-kid", 4) => {
                // this process forks C and waits; C starts a new session, forks G (`/bin/sleep M`)
                // and waits: G is a grandchild in another session whose parent lives
                record(&a[3], libc::getpid());
                match libc::fork() {
                    0 => {
                        libc::setsid();
                        match libc::fork() {
                            0 => {
                                record(&a[3], libc::getpid());
                                exec_sleep(&CString::new(a[2].as_str()).unwrap());
                            }
                            -1 => libc::_exit(1),
                            g => {
                                record(&a[3], libc::getpid());
                                let mut st = 0;
                                libc::waitpid(g, &mut st, 0);
                                libc::_exit(0);
                            }
                        }
                    }
                    -1 => std::process::exit(1),
                    c => {
                        let mut st = 0;
                        libc::waitpid(c, &mut st, 0);
                    }
                }
            }
            ("fork-on-term", 4) => {
                // this process forks C and waits; C waits too, and on TERM forks an escapee and
                // exits (a member that daemonizes when it is told to stop). This process keeps
                // TERM at its default. C records itself once its handler is set.
                record(&a[3], libc::getpid());
                let r = a[3].as_bytes();
                std::ptr::copy_nonoverlapping(r.as_ptr(), std::ptr::addr_of_mut!(TERM_REC) as *mut u8, r.len().min(511));
                match libc::fork() {
                    0 => {
                        on(libc::SIGTERM, fork_on_term as *const () as usize, false);
                        unblock_all();
                        record(&a[3], libc::getpid());
                        libc::alarm(60);
                        loop {
                            libc::pause();
                        }
                    }
                    -1 => std::process::exit(1),
                    c => {
                        let mut st = 0;
                        libc::waitpid(c, &mut st, 0);
                    }
                }
            }
            ("linger-on", 5) => {
                // wait (30 s at most) for the file GO, then fork C: a new session, C forks G
                // (recorded, `/bin/sleep M`), lives 300 ms and exits; this process then waits
                record(&a[3], libc::getpid());
                libc::alarm(60);
                let mut n = 0;
                while !std::path::Path::new(&a[4]).exists() && n < 3000 {
                    libc::usleep(10_000);
                    n += 1;
                }
                match libc::fork() {
                    0 => {
                        libc::setsid();
                        if libc::fork() == 0 {
                            record(&a[3], libc::getpid());
                            exec_sleep(&CString::new(a[2].as_str()).unwrap());
                        }
                        libc::usleep(300_000);
                        libc::_exit(0);
                    }
                    -1 => std::process::exit(1),
                    c => {
                        let mut st = 0;
                        libc::waitpid(c, &mut st, 0);
                    }
                }
                loop {
                    libc::pause();
                }
            }
            ("escape-nomarker", 3) => {
                let m = CString::new("29").unwrap();
                if let Some(g) = spawn_escapee(&a[2], true, || { exec_sleep(&m); }) {
                    record(&a[2], g);
                }
            }
            ("chain", 5) | ("chain", 6) => {
                let m = CString::new(a[2].as_str()).unwrap();
                let n: usize = a[4].parse().unwrap_or_else(|_| usage());
                let delay: u32 = a.get(5).map(|d| d.parse().unwrap_or_else(|_| usage())).unwrap_or(500);
                let g = spawn_escapee(&a[3], true, || {
                    for _ in 0..n {
                        if delay > 0 {
                            libc::usleep(delay);
                        }
                        match libc::fork() {
                            0 => continue,
                            -1 => {}
                            _ => libc::_exit(0),
                        }
                    }
                    exec_sleep(&m);
                });
                if let Some(g) = g {
                    record(&a[3], g);
                }
            }
            ("breed", 5) => {
                let m = CString::new(a[2].as_str()).unwrap();
                let rec = a[3].clone();
                let n: usize = a[4].parse().unwrap_or_else(|_| usage());
                let g = spawn_escapee(&rec, true, || {
                    for _ in 0..n {
                        match libc::fork() {
                            0 => exec_sleep(&m),
                            c if c > 0 => record(&rec, c),
                            _ => {}
                        }
                        libc::usleep(200);
                    }
                    exec_sleep(&m);
                });
                if let Some(g) = g {
                    record(&a[3], g);
                }
            }
            _ => usage(),
        }
        libc::_exit(0)
    }
}
