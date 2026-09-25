//! Test fixture, not a product binary.
//!
//! `sd-fixture escape <marker> <record-file>`: the root forks C. C calls setsid, forks G,
//! writes G's pid into a pipe and exits at once. The root reads the pid, appends it to
//! <record-file> and exits at once. G execs `/bin/sleep <marker>`.
//!
//! The root records, not C or G, because sheepdog never kills the root before it exits: a C
//! or G recorder can be killed before it writes (on Debian 26% were), which would make a
//! created escapee look uncreated. C still exits before the root, so G is already orphaned
//! when the root exits: that is the race under test (PLAN.md §6 cell 3).
//!
//! The marker is a number such as `29.0123456`, so G's argv is unique to one test iteration
//! and G ends by itself after at most 29 s.

use std::ffi::CString;
use std::io::Write;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 || args[1] != "escape" {
        eprintln!("usage: sd-fixture escape <marker> <record-file>");
        std::process::exit(2);
    }
    let marker = CString::new(args[2].as_str()).unwrap();
    let record = args[3].clone();
    unsafe {
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
                        let prog = CString::new("/bin/sleep").unwrap();
                        let argv = [prog.as_ptr(), marker.as_ptr(), std::ptr::null()];
                        libc::execv(prog.as_ptr(), argv.as_ptr());
                        libc::_exit(127);
                    }
                    g => {
                        let b = g.to_ne_bytes();
                        libc::write(fds[1], b.as_ptr() as *const libc::c_void, b.len());
                        libc::_exit(0)
                    }
                }
            }
            -1 => std::process::exit(1),
            _ => {
                libc::close(fds[1]);
                let mut b = [0u8; 4];
                let n = libc::read(fds[0], b.as_mut_ptr() as *mut libc::c_void, 4);
                if n == 4 {
                    if let Ok(mut f) =
                        std::fs::OpenOptions::new().append(true).create(true).open(&record)
                    {
                        let _ = f.write_all(format!("{}\n", i32::from_ne_bytes(b)).as_bytes());
                    }
                }
                libc::_exit(0)
            }
        }
    }
}
