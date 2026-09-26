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
//! - `shell REPORT [bg] [null-stdin] PROG ARGS...`: the pty harness's job-control shell (PHASE1.md
//!   §2). Run as a session leader with the pty as its controlling terminal. It starts PROG in a
//!   new process group, makes that group the foreground (unless `bg`), and waits with
//!   WUNTRACED. It appends `started <pgid>`, `stopped <sig>`, `exited <code>` or
//!   `signaled <sig>` to REPORT. After a stop it waits for `REPORT.fg` to exist, then gives
//!   the terminal back to the job, continues it (`fg`) and appends `continued`. A HUP (the terminal closed) is
//!   sent on to the job's group, as bash does, and logged as `hup`.
//! - `nosession PIDFILE PROG ARGS...`: start PROG with no controlling terminal and not as a
//!   session leader (as liveapp and CI run it): setsid, then PROG in a new process group of
//!   that session. PROG's pid (= its pgid) goes to PIDFILE; exits as PROG did.
//! - `int-exit CODE M`: exit CODE on INT (a root that handles ctrl-C itself); else sleep.
//! - `bg-then-exec M PROG ARGS...`: fork a background job (`/bin/sleep M`, stdout and stderr
//!   to /dev/null), then exec PROG in this process, with no shell in between (a shell such as
//!   dash would reset the signal mask). This is the "job & exec sheepdog" shape.

use sheepdog::ident::identity;
use std::ffi::CString;
use std::io::Write;

fn record(path: &str, pid: i32) {
    let id = identity(pid).unwrap_or(0);
    if let Ok(mut f) = std::fs::OpenOptions::new().append(true).create(true).open(path) {
        let _ = f.write_all(format!("{pid} {id}\n").as_bytes());
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
        append(report, &format!("started {child}"));
        let fg = format!("{report}.fg");
        loop {
            let mut st = 0;
            let r = libc::waitpid(child, &mut st, libc::WUNTRACED);
            if r < 0 {
                if HUP_SEEN.swap(false, std::sync::atomic::Ordering::SeqCst) {
                    libc::kill(-child, libc::SIGHUP);
                    append(report, "hup");
                    continue;
                }
                if std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                    continue;
                }
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

/// Re-exec this fixture as `mode M R` with the responsibility disclaim (macOS), so the new
/// image is responsible for itself.
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

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let usage = || -> ! {
        eprintln!("usage: sd-fixture escape|escape-fast M R | escape-nomarker R | breed M R N");
        std::process::exit(2)
    };
    let mode = a.get(1).map(String::as_str).unwrap_or_else(|| usage());
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
    if mode == "shell" && a.len() >= 4 {
        let mut i = 3;
        let (mut bg, mut null_stdin) = (false, false);
        while i < a.len() && (a[i] == "bg" || a[i] == "null-stdin") {
            bg |= a[i] == "bg";
            null_stdin |= a[i] == "null-stdin";
            i += 1;
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
            let _ = std::fs::write(&a[2], format!("{}\n", c.id()));
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
    if mode == "int-exit" && a.len() == 4 {
        unsafe {
            INT_EXIT = a[2].parse().unwrap_or(1);
            on(libc::SIGINT, int_exit as *const () as usize, true);
            unblock_all();
            // no exec (it would reset the handler); the marker stays in this argv
            loop {
                libc::pause();
            }
        }
    }
    if mode == "bg-then-exec" && a.len() >= 4 {
        use std::os::unix::process::CommandExt;
        let m = CString::new(a[2].as_str()).unwrap();
        unsafe {
            if libc::fork() == 0 {
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
    unsafe {
        match (mode, a.len()) {
            ("escape", 4) | ("escape-fast", 4) => {
                let m = CString::new(a[2].as_str()).unwrap();
                let via_pipe = mode == "escape";
                if let Some(g) = spawn_escapee(&a[3], via_pipe, || { exec_sleep(&m); }) {
                    record(&a[3], g);
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
