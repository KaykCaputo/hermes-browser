// Copyright 2015 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! Sandboxing on Linux via miscellaneous kernel features.

use libc;
use std::io;

pub fn activate() -> Result<(), libc::c_int> {
    // Do not set RLIMIT_FSIZE to zero here. Modern Servo IPC creates anonymous
    // shared-memory files with memfd_create() and sizes them with ftruncate().
    // RLIMIT_FSIZE also applies to those descriptors, so a zero limit makes a
    // normal page load fail with EFBIG and terminates the content process with
    // SIGXFSZ. Filesystem writes remain prohibited by the read-only open/openat
    // seccomp rules and by the restricted mount namespace/chroot.

    // Set a restrictive `umask` so that even if files happened to get written it'd be hard to do
    // anything with them.
    unsafe {
        libc::umask(0);
    }

    // Disable core dumps and debugging via `PTRACE_ATTACH`.
    let result = unsafe {
        libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0)
    };
    if result != 0 {
        return Err(result)
    }

    // Enter a new session group. (This can fail with -EPERM if we're already the session leader,
    // which is OK.)
    unsafe {
        if libc::setsid() < 0 {
            let result = io::Error::last_os_error().raw_os_error().unwrap() as i32;
            if result != libc::EPERM {
                return Err(result)
            }
        }
    }

    // Clear out the process environment.
    let result = unsafe {
        libc::clearenv()
    };
    if result == 0 {
        Ok(())
    } else {
        Err(result)
    }
}
