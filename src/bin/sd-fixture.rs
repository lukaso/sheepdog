//! Test fixture, not a product binary.
//!
//! `sd-fixture escape <marker> <record-file>`: the root forks C and exits at once. C calls
//! setsid, forks G and exits at once. G appends its pid to <record-file>, then execs
//! `/bin/sleep <marker>`. The marker is a number such as `29.0123456`, so G's argv is unique
//! to one test iteration and G ends by itself after at most 29 s.

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
        match libc::fork() {
            0 => {
                libc::setsid();
                match libc::fork() {
                    0 => {
                        if let Ok(mut f) =
                            std::fs::OpenOptions::new().append(true).create(true).open(&record)
                        {
                            let _ = writeln!(f, "{}", libc::getpid());
                        }
                        let prog = CString::new("/bin/sleep").unwrap();
                        let argv = [prog.as_ptr(), marker.as_ptr(), std::ptr::null()];
                        libc::execv(prog.as_ptr(), argv.as_ptr());
                        libc::_exit(127);
                    }
                    _ => libc::_exit(0),
                }
            }
            -1 => std::process::exit(1),
            _ => libc::_exit(0),
        }
    }
}
