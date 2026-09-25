use std::os::raw::c_int;
extern "C" { fn execv(p:*const i8, a:*const *const i8)->c_int; }
fn main(){ let p=b"/bin/sh\0"; let a=[b"sh\0".as_ptr() as *const i8, b"-c\0".as_ptr() as *const i8, b"trap -p; yes | head -c1 >/dev/null; echo yes_rc_seen; ps -o pid,ignored -p $$ | tail -1\0".as_ptr() as *const i8, std::ptr::null()]; unsafe{ execv(p.as_ptr() as *const i8, a.as_ptr()); } }
