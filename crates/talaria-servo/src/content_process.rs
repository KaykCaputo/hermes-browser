use std::error::Error;
use std::fmt;

const CONTENT_PROCESS_ARGUMENT: &str = "--content-process";

/// Environment variable in which the browser records its own process id for its content
/// processes, which inherit it.
const BROWSER_PROCESS_ID_VARIABLE: &str = "MERCURY_BROWSER_PROCESS_ID";

/// Records this browser's process id for the content processes Servo will start, so that they
/// can tell when it has gone (`exit_when_the_browser_process_is_gone`).
///
/// Call it in `main` after `run_content_process_if_requested` has returned `false` (a content
/// process must keep its browser's value) and before any other thread exists.
pub fn record_browser_process_id() {
    // SAFETY: called on the main thread before any other thread has been started, so nothing
    // can read the environment concurrently.
    unsafe {
        std::env::set_var(BROWSER_PROCESS_ID_VARIABLE, std::process::id().to_string());
    }
}

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

    ignore_group_quit_signals();
    exit_when_the_browser_process_is_gone();
    servo::run_content_process(token);
    Ok(true)
}

/// A terminal's Ctrl-C (`SIGINT`) and hang-up (`SIGHUP`) go to the whole process group, content
/// processes included, and so does a group-wide `SIGTERM` (`kill -TERM -<pgid>`, systemd's
/// `KillMode=control-group`). Ended by them, a content process would die while the browser is
/// shutting Servo down in order, which then waits for it until the shutdown watchdog fires. The
/// browser handles these signals and ends its content processes itself (closing their IPC); if
/// it dies instead, `exit_when_the_browser_process_is_gone` ends them within a second.
#[cfg(unix)]
fn ignore_group_quit_signals() {
    for signal in [libc::SIGINT, libc::SIGHUP, libc::SIGTERM] {
        // SAFETY: setting a signal's disposition to "ignore" affects only this process, before
        // any thread has started; it is inherited by what this process executes, which is
        // what is wanted for these two signals.
        unsafe {
            libc::signal(signal, libc::SIG_IGN);
        }
    }
}

#[cfg(not(unix))]
fn ignore_group_quit_signals() {}

/// Ends this content process once the browser process that started it has gone.
///
/// A content process waits on IPC from the browser process. If the browser dies without
/// shutting Servo down (for example `kill -9`, or a crash), nothing tells the content process
/// to stop, and it would keep running as an orphan with its memory and open pages. On Unix
/// an orphan is adopted by another process, so its parent id changes: checking once a second
/// is cheap and catches every way the browser can end. The browser's id comes from
/// `MERCURY_BROWSER_PROCESS_ID` (see `record_browser_process_id`), not from a guess: the
/// browser may itself be process 1, as a container's entry process. If the variable is missing
/// or unparsable (a browser that did not record it), the parent at start-up is watched instead
/// and a warning is logged; a healthy child keeps running. The process ends with `_exit`, which
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

    let recorded = std::env::var(BROWSER_PROCESS_ID_VARIABLE).ok();
    let parent_at_start = std::os::unix::process::parent_id();
    let browser_process_id = expected_browser_process_id(recorded.as_deref(), parent_at_start);
    if recorded
        .as_deref()
        .and_then(|value| value.parse::<u32>().ok())
        .is_none()
    {
        log::warn!(
            "{BROWSER_PROCESS_ID_VARIABLE} is missing or invalid; watching the parent at start-up \
             ({parent_at_start}) instead"
        );
    }
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

/// Whether the browser process is gone: the current parent is no longer the expected browser
/// process. A browser that is itself process 1 stays "present" as long as it is the parent.
#[cfg_attr(not(unix), allow(dead_code))]
fn parent_changed(expected: u32, current: u32) -> bool {
    current != expected
}

/// The browser process this content process belongs to: the id recorded in the environment,
/// or the parent at start-up when there is no usable record.
#[cfg_attr(not(unix), allow(dead_code))]
fn expected_browser_process_id(recorded: Option<&str>, parent_at_start: u32) -> u32 {
    recorded
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(parent_at_start)
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
    use super::{
        ContentProcessArgumentError, content_process_token, expected_browser_process_id,
        parent_changed,
    };

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
    fn a_browser_running_as_process_one_is_still_there() {
        assert!(!parent_changed(1, 1));
    }

    #[test]
    fn the_recorded_browser_process_id_is_the_expected_parent() {
        assert_eq!(expected_browser_process_id(Some("4242"), 1), 4242);
        assert_eq!(expected_browser_process_id(Some("1"), 1), 1);
    }

    #[test]
    fn without_a_usable_record_the_parent_at_start_is_expected() {
        assert_eq!(expected_browser_process_id(None, 4242), 4242);
        assert_eq!(expected_browser_process_id(Some(""), 4242), 4242);
        assert_eq!(
            expected_browser_process_id(Some("not a number"), 4242),
            4242
        );
        assert_eq!(expected_browser_process_id(Some("-5"), 4242), 4242);
    }

    #[test]
    fn a_child_whose_browser_died_before_it_started_is_detected() {
        // The browser (4242) recorded itself, then died; the child was adopted by process 1.
        let expected = expected_browser_process_id(Some("4242"), 1);

        assert!(parent_changed(expected, 1));
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
