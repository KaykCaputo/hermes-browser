// Adapted from Servo's servoshell, https://github.com/servo/servo, tag `v0.8.0` (commit
// `4e136d8ef31b7c4748fbfac606ca463f3efd7537`), file `ports/servoshell/panic_hook.rs`.
// Licensed under the Mozilla Public License 2.0. Servo's options are read through
// `talaria-servo` instead of `servo::opts`.
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::io::Write;
use std::panic::PanicHookInfo;
use std::{env, thread};

use log::{error, warn};
use talaria_servo::{hard_fail_in_single_process, should_panic_hook_suppress_termination};

use crate::crash_handler::raise_signal_or_exit_with_error;

/// Prints the panic and then logs it at error level. Servo's constellation turns an error
/// logged while a thread is panicking into a crash report for the affected page, which is how
/// a content panic reaches `notify_crashed` instead of going unnoticed.
pub(crate) fn panic_hook(info: &PanicHookInfo) {
    warn!("Panic hook called.");
    let msg = match info.payload().downcast_ref::<&'static str>() {
        Some(s) => *s,
        None => match info.payload().downcast_ref::<String>() {
            Some(s) => &**s,
            None => "Box<Any>",
        },
    };
    let current_thread = thread::current();
    let name = current_thread.name().unwrap_or("<unnamed>");
    let stderr = std::io::stderr();
    let mut stderr = stderr.lock();
    if let Some(location) = info.location() {
        let _ = writeln!(
            &mut stderr,
            "{} (thread {}, at {}:{})",
            msg,
            name,
            location.file(),
            location.line()
        );
    } else {
        let _ = writeln!(&mut stderr, "{msg} (thread {name})");
    }
    if env::var("RUST_BACKTRACE").is_ok() {
        let _ = crate::crash_handler::backtrace::print(&mut stderr);
    }
    drop(stderr);

    if !should_panic_hook_suppress_termination() {
        if hard_fail_in_single_process() {
            // When we are exiting due to a hard-failure mode, we trigger a segfault so that crash
            // tests detect that we crashed. If we exit normally it just looks like a non-crash exit.
            raise_signal_or_exit_with_error(libc::SIGSEGV);
        }
        error!("{msg}");
    }
}
