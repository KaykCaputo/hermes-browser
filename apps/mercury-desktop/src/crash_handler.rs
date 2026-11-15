// Adapted from Servo's servoshell, https://github.com/servo/servo, tag `v0.8.0` (commit
// `4e136d8ef31b7c4748fbfac606ca463f3efd7537`), files `ports/servoshell/crash_handler.rs` and
// `ports/servoshell/backtrace.rs` (the latter as the `backtrace` module below, without its
// OpenHarmony-only function). Licensed under the Mozilla Public License 2.0.
//
// Limitations inherited from servoshell; this is a diagnostic aid, not a crash reporter:
// - The handler allocates and symbolises a backtrace inside the signal handler. That is not
//   async-signal-safe: if the fault happened while the allocator's lock was held, the handler
//   can deadlock and the process hangs instead of dying.
// - It is installed without an alternate signal stack: `signal-hook-registry` registers its
//   dispatcher with `SA_RESTART | SA_SIGINFO` but not `SA_ONSTACK`, so on a stack overflow the
//   handler runs on the exhausted stack, faults again, and the process dies without Rust's
//   "has overflowed its stack" message.
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(crate) fn install() {}

/// Prints the signal and a backtrace when the process crashes with SIGSEGV, SIGILL, SIGIOT or
/// SIGBUS, then lets the signal terminate the process as it would have.
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(crate) fn install() {
    use std::io::Write;
    use std::sync::atomic;
    use std::thread;

    use libc::siginfo_t;

    fn handler(siginfo: &siginfo_t) {
        // Only print crash message and backtrace the first time, to avoid
        // infinite recursion if the printing causes another signal.
        static BEEN_HERE_BEFORE: atomic::AtomicBool = atomic::AtomicBool::new(false);
        if !BEEN_HERE_BEFORE.swap(true, atomic::Ordering::SeqCst) {
            // stderr is unbuffered, so we won’t lose output if we crash later
            // in this handler, and the std::io::stderr() call never allocates.
            // std::io::stdout() allocates the first time it’s called, which in
            // practice will often segfault (see below).
            let stderr = std::io::stderr();
            let mut stderr = stderr.lock();
            let _ = write!(&mut stderr, "Caught signal {}", siginfo.si_signo);
            if let Some(name) = thread::current().name() {
                let _ = write!(&mut stderr, " in thread \"{name}\"");
            }
            let _ = writeln!(&mut stderr);

            // This call always allocates, which in practice will segfault if
            // we’re handling a non-main-thread (e.g. layout) segfault. Strictly
            // speaking in POSIX terms, this is also undefined behaviour.
            let _ = backtrace::print(&mut stderr);
        }

        // Outside the BEEN_HERE_BEFORE check, we must only call functions we
        // know to be “async-signal-safe”, which includes sigaction(), raise(),
        // and _exit(), but generally doesn’t include anything that allocates.
        // https://pubs.opengroup.org/onlinepubs/9699919799/functions/V2_chap02.html#tag_15_04_03_03
        raise_signal_or_exit_with_error(siginfo.si_signo);
    }

    // SAFETY: the handler only allocates the first time it runs, as servoshell's does; after
    // that it restores the default action and re-raises, which is async-signal-safe.
    unsafe {
        signal_hook_registry::register_unchecked(libc::SIGSEGV, handler)
            .expect("Could not register SIGSEGV handler");
        signal_hook_registry::register_unchecked(libc::SIGILL, handler)
            .expect("Could not register SIGILL handler");
        signal_hook_registry::register_unchecked(libc::SIGIOT, handler)
            .expect("Could not register SIGIOT handler");
        signal_hook_registry::register_unchecked(libc::SIGBUS, handler)
            .expect("Could not register SIGBUS handler");
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(crate) fn raise_signal_or_exit_with_error(_signal: i32) {
    std::process::exit(1);
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(crate) fn raise_signal_or_exit_with_error(signal: i32) {
    // SAFETY: `sigaction` with a zeroed action set to `SIG_DFL` and `raise` are
    // async-signal-safe and only affect this process.
    unsafe {
        // Reset the signal to the default action, and reraise the signal.
        // Unlike libc::_exit(sig), which terminates the process normally,
        // this terminates abnormally just like an uncaught signal, allowing
        // mach (or your shell) to distinguish it from an ordinary exit, and
        // allows your kernel to make a core dump if configured to do so.
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = libc::SIG_DFL;
        libc::sigaction(signal, &action, std::ptr::null_mut());
        libc::raise(signal);
    }
}

/// Similar to `println!("{:?}", Backtrace::new())`, but doesn’t allocate.
///
/// Seems to fix some deadlocks: <https://github.com/servo/servo/issues/24881>
///
/// FIXME: if/when a future version of the `backtrace` crate has
/// <https://github.com/rust-lang/backtrace-rs/pull/265>, use that instead.
pub(crate) mod backtrace {
    use std::fmt::{self, Write};

    use backtrace::{BytesOrWideString, PrintFmt};

    #[inline(never)]
    pub(crate) fn print(w: &mut dyn std::io::Write) -> Result<(), std::io::Error> {
        write!(
            w,
            "{:?}",
            Print {
                print_fn_address: (print as *const ()).addr(),
            }
        )
    }

    struct Print {
        /// Function pointer without provenance - do not cast back and dereference.
        print_fn_address: usize,
    }

    impl fmt::Debug for Print {
        fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
            // Safety: we’re in a signal handler that is about to call `libc::_exit`.
            // Potential data races from using `*_unsynchronized` functions are perhaps
            // less bad than potential deadlocks?
            unsafe {
                let mut print_fn_frame = 0;
                let mut frame_count = 0;
                backtrace::trace_unsynchronized(|frame| {
                    let found = frame.symbol_address().addr() == self.print_fn_address;
                    if found {
                        print_fn_frame = frame_count;
                    }
                    frame_count += 1;
                    !found
                });

                let mode = PrintFmt::Short;
                let mut p = print_path;
                let mut f = backtrace::BacktraceFmt::new(fmt, mode, &mut p);
                f.add_context()?;
                let mut result = Ok(());
                let mut frame_count = 0;
                backtrace::trace_unsynchronized(|frame| {
                    let skip = frame_count < print_fn_frame;
                    frame_count += 1;
                    if skip {
                        return true;
                    }

                    let mut frame_fmt = f.frame();
                    let mut any_symbol = false;
                    backtrace::resolve_frame_unsynchronized(frame, |symbol| {
                        any_symbol = true;
                        if let Err(e) = frame_fmt.symbol(frame, symbol) {
                            result = Err(e)
                        }
                    });
                    if !any_symbol && let Err(e) = frame_fmt.print_raw(frame.ip(), None, None, None)
                    {
                        result = Err(e)
                    }
                    result.is_ok()
                });
                result?;
                f.finish()
            }
        }
    }

    fn print_path(fmt: &mut fmt::Formatter<'_>, path: BytesOrWideString<'_>) -> fmt::Result {
        match path {
            BytesOrWideString::Bytes(mut bytes) => loop {
                match std::str::from_utf8(bytes) {
                    Ok(s) => {
                        fmt.write_str(s)?;
                        break;
                    }
                    Err(err) => {
                        fmt.write_char(std::char::REPLACEMENT_CHARACTER)?;
                        match err.error_len() {
                            Some(len) => bytes = &bytes[err.valid_up_to() + len..],
                            None => break,
                        }
                    }
                }
            },
            BytesOrWideString::Wide(wide) => {
                for c in std::char::decode_utf16(wide.iter().cloned()) {
                    fmt.write_char(c.unwrap_or(std::char::REPLACEMENT_CHARACTER))?
                }
            }
        }
        Ok(())
    }
}
