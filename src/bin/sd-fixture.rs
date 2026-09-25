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

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let usage = || -> ! {
        eprintln!("usage: sd-fixture escape|escape-fast M R | escape-nomarker R | breed M R N");
        std::process::exit(2)
    };
    let mode = a.get(1).map(String::as_str).unwrap_or_else(|| usage());
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
