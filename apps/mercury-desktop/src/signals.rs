//! `SIGTERM`, `SIGINT` and `SIGHUP` start the same orderly quit as the quit shortcut.
//!
//! The handler only records the signal and writes one byte to a pipe; a thread waiting on the
//! pipe asks the event loop to quit, so the quit itself runs on the main thread. A second
//! signal while the first is being handled ends the process at once, by that signal. After an
//! orderly quit, `main` ends the process by re-raising the first signal with its default
//! action (see `end_by`).
//!
//! Installed only in the browser process: content processes return from `main` before
//! `install` is reached, and handlers do not survive the `exec` that starts them anyway.

use std::sync::atomic::{AtomicI32, Ordering};

/// The first quit signal received, or 0.
static RECEIVED: AtomicI32 = AtomicI32::new(0);
/// The pipe end the handler writes to, or -1.
static WAKE_WRITE_FD: AtomicI32 = AtomicI32::new(-1);

const QUIT_SIGNALS: [libc::c_int; 3] = [libc::SIGTERM, libc::SIGINT, libc::SIGHUP];

/// What the handler does with a quit signal, given the signal received before it (0 if none).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SignalAction {
    StartOrderlyQuit,
    EndNow,
}

pub(crate) fn action_for(previously_received: i32) -> SignalAction {
    if previously_received == 0 {
        SignalAction::StartOrderlyQuit
    } else {
        SignalAction::EndNow
    }
}

/// The exit status a shell reports for a process ended by `signal`, used where the process
/// exits rather than re-raising (the shutdown watchdog).
pub(crate) fn exit_status_for(signal: i32) -> i32 {
    128 + signal
}

/// The first quit signal received, if any.
pub(crate) fn received() -> Option<i32> {
    match RECEIVED.load(Ordering::SeqCst) {
        0 => None,
        signal => Some(signal),
    }
}

/// Installs the handlers. Call once, in the browser process only.
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(crate) fn install() {
    let mut fds = [0 as libc::c_int; 2];
    // SAFETY: `fds` has room for the two descriptors `pipe` writes.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        log::warn!(
            "could not create the signal pipe ({}); quit signals end the browser at once",
            std::io::Error::last_os_error()
        );
        return;
    }
    let [read_fd, write_fd] = fds;
    // SAFETY: both descriptors were just created and are owned here. Close-on-exec keeps them
    // out of content processes; a full pipe must never block the handler.
    unsafe {
        libc::fcntl(read_fd, libc::F_SETFD, libc::FD_CLOEXEC);
        libc::fcntl(write_fd, libc::F_SETFD, libc::FD_CLOEXEC);
        libc::fcntl(write_fd, libc::F_SETFL, libc::O_NONBLOCK);
    }
    WAKE_WRITE_FD.store(write_fd, Ordering::SeqCst);

    let spawned = std::thread::Builder::new()
        .name("quit signals".to_owned())
        .spawn(move || wait_for_signals(read_fd));
    if let Err(error) = spawned {
        log::warn!(
            "could not start the quit-signal thread ({error}); signals end the browser at once"
        );
        return;
    }

    for signal in QUIT_SIGNALS {
        // SAFETY: `handle` only performs async-signal-safe operations: atomic operations,
        // `write`, `sigaction` and `raise`.
        let registered = unsafe { signal_hook_registry::register(signal, move || handle(signal)) };
        if let Err(error) = registered {
            log::warn!("could not handle signal {signal}: {error}");
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(crate) fn install() {}

/// Runs inside the signal handler: async-signal-safe operations only.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn handle(signal: i32) {
    let previous = RECEIVED
        .compare_exchange(0, signal, Ordering::SeqCst, Ordering::SeqCst)
        .unwrap_or_else(|current| current);
    match action_for(previous) {
        SignalAction::StartOrderlyQuit => {
            let byte = 1u8;
            // SAFETY: `write` is async-signal-safe; the descriptor is non-blocking and stays
            // open for the life of the process. A failed write only loses the wake-up.
            let _ = unsafe {
                libc::write(
                    WAKE_WRITE_FD.load(Ordering::SeqCst),
                    (&raw const byte).cast(),
                    1,
                )
            };
        }
        SignalAction::EndNow => crate::crash_handler::raise_signal_or_exit_with_error(signal),
    }
}

/// Waits for the handler's byte and asks the event loop to quit. Until the event loop exists,
/// the request is retried.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn wait_for_signals(read_fd: libc::c_int) {
    loop {
        let mut byte = 0u8;
        // SAFETY: reads at most one byte into `byte`.
        let read = unsafe { libc::read(read_fd, (&raw mut byte).cast(), 1) };
        if read < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        if read <= 0 {
            return;
        }
        if let Some(signal) = received() {
            log::info!(
                "received signal {signal}; quitting in order (a second one ends the browser at once)"
            );
        }
        while slint::invoke_from_event_loop(|| {
            let _slint = crate::callback_scope::InSlintCallback::enter();
            crate::quit::request_quit();
        })
        .is_err()
        {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
}

/// Ends the process by `signal` with its default action, as if the signal had not been
/// caught, so the parent's `wait` reports it. Call last, after the shutdown.
pub(crate) fn end_by(signal: i32) -> ! {
    log::logger().flush();
    crate::crash_handler::raise_signal_or_exit_with_error(signal);
    std::process::exit(exit_status_for(signal));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_quit_signal_starts_an_orderly_quit() {
        assert_eq!(action_for(0), SignalAction::StartOrderlyQuit);
    }

    #[test]
    fn a_second_quit_signal_ends_the_browser_at_once() {
        for previous in QUIT_SIGNALS {
            assert_eq!(action_for(previous), SignalAction::EndNow);
        }
    }

    #[test]
    fn the_exit_status_for_a_signal_is_128_plus_its_number() {
        assert_eq!(exit_status_for(libc::SIGTERM), 143);
        assert_eq!(exit_status_for(libc::SIGINT), 130);
        assert_eq!(exit_status_for(libc::SIGHUP), 129);
    }
}
