use std::error::Error;
use std::fmt;

const CONTENT_PROCESS_ARGUMENT: &str = "--content-process";

/// Runs Servo's unprivileged content-process entry point when this executable
/// was spawned by the multiprocess constellation.
///
/// This must run before the desktop UI, graphics backend, media backend, or any
/// worker threads are initialized. The content process starts only the parent watchdog
/// thread (see `exit_when_the_browser_process_is_gone`) before handing over to Servo; the
/// operating-system sandbox is off (`mercury_options`), so no sandbox is entered here.
pub fn run_content_process_if_requested() -> Result<bool, ContentProcessArgumentError> {
    // `args_os`: `std::env::args` panics on any argument that is not valid UTF-8. The token
    // Servo passes is ASCII, so a lossy conversion of the other arguments is harmless here.
    let arguments = std::env::args_os().map(|argument| argument.to_string_lossy().into_owned());
    let Some(token) = content_process_token(arguments)? else {
        return Ok(false);
    };

    exit_when_the_browser_process_is_gone();
    servo::run_content_process(token);
    Ok(true)
}

/// Ends this content process once the browser process that started it has gone.
///
/// A content process waits on IPC from the browser process. If the browser dies without
/// shutting Servo down (for example `kill -9`, or a crash), nothing tells the content process
/// to stop, and it would keep running as an orphan with its memory and open pages. On Unix
/// an orphan is adopted by another process, so its parent id changes: checking once a second
/// is cheap and catches every way the browser can end. The process ends with `_exit`, which
/// skips destructors and `atexit` handlers that could otherwise run while Servo's script
/// threads are still executing.
///
/// Sandbox constraint: this thread is started before Servo would enter its sandbox, and on
/// Linux a seccomp filter applies only to the thread that installs it, so this thread would
/// stay unconfined. Before the sandbox is enabled on Linux, replace the thread (for example
/// with `prctl(PR_SET_PDEATHSIG)`).
#[cfg(unix)]
fn exit_when_the_browser_process_is_gone() {
    use std::time::Duration;

    let browser_process_id = std::os::unix::process::parent_id();
    let spawn_result = std::thread::Builder::new()
        .name("ParentWatchdog".to_owned())
        .spawn(move || {
            loop {
                if parent_changed(browser_process_id, std::os::unix::process::parent_id()) {
                    // SAFETY: `_exit` ends the process immediately; it is async-signal-safe and
                    // touches no Rust state.
                    unsafe { libc::_exit(0) };
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        });
    if let Err(error) = spawn_result {
        log::warn!("could not watch the browser process: {error}");
    }
}

#[cfg(not(unix))]
fn exit_when_the_browser_process_is_gone() {}

/// Whether the browser process that started this content process is gone, given the parent id
/// recorded at start-up and the current one.
#[cfg_attr(not(unix), allow(dead_code))]
fn parent_changed(recorded: u32, current: u32) -> bool {
    // A recorded parent of 1 (init or launchd) means the browser had already exited and this
    // process was adopted before it could record its real parent.
    recorded == 1 || current != recorded
}

fn content_process_token(
    arguments: impl IntoIterator<Item = String>,
) -> Result<Option<String>, ContentProcessArgumentError> {
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        if argument != CONTENT_PROCESS_ARGUMENT {
            continue;
        }

        return arguments
            .next()
            .filter(|token| !token.is_empty())
            .map(Some)
            .ok_or(ContentProcessArgumentError);
    }

    Ok(None)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContentProcessArgumentError;

impl fmt::Display for ContentProcessArgumentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{CONTENT_PROCESS_ARGUMENT} requires the IPC bootstrap token supplied by Servo"
        )
    }
}

impl Error for ContentProcessArgumentError {}

#[cfg(test)]
mod tests {
    use super::{ContentProcessArgumentError, content_process_token, parent_changed};

    fn arguments(values: &[&str]) -> impl Iterator<Item = String> {
        values.iter().map(|value| (*value).to_owned())
    }

    #[test]
    fn the_same_parent_means_the_browser_is_still_there() {
        assert!(!parent_changed(4242, 4242));
    }

    #[test]
    fn a_new_parent_means_the_browser_is_gone() {
        assert!(parent_changed(4242, 1));
        assert!(parent_changed(4242, 777));
    }

    #[test]
    fn a_recorded_parent_of_one_means_the_browser_was_already_gone() {
        assert!(parent_changed(1, 1));
    }

    #[test]
    fn regular_browser_invocation_has_no_content_process_token() {
        assert_eq!(
            content_process_token(arguments(&["mercury-browser"])),
            Ok(None)
        );
    }

    #[test]
    fn extracts_the_servo_content_process_token() {
        assert_eq!(
            content_process_token(arguments(&[
                "mercury-browser",
                "--unrelated",
                "--content-process",
                "ipc-token",
            ])),
            Ok(Some("ipc-token".to_owned()))
        );
    }

    #[test]
    fn rejects_a_missing_content_process_token() {
        assert_eq!(
            content_process_token(arguments(&["mercury-browser", "--content-process"])),
            Err(ContentProcessArgumentError)
        );
        assert_eq!(
            content_process_token(arguments(&["mercury-browser", "--content-process", "",])),
            Err(ContentProcessArgumentError)
        );
    }
}
