use std::error::Error;
use std::fmt;

const CONTENT_PROCESS_ARGUMENT: &str = "--content-process";

/// Runs Servo's unprivileged content-process entry point when this executable
/// was spawned by the multiprocess constellation.
///
/// This must run before the desktop UI, graphics backend, media backend, or any
/// worker threads are initialized. Sandboxed children are deliberately kept on
/// Servo's minimal bootstrap path.
pub fn run_content_process_if_requested() -> Result<bool, ContentProcessArgumentError> {
    let Some(token) = content_process_token(std::env::args())? else {
        return Ok(false);
    };

    servo::run_content_process(token);
    Ok(true)
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
    use super::{ContentProcessArgumentError, content_process_token};

    fn arguments(values: &[&str]) -> impl Iterator<Item = String> {
        values.iter().map(|value| (*value).to_owned())
    }

    #[test]
    fn regular_browser_invocation_has_no_content_process_token() {
        assert_eq!(
            content_process_token(arguments(&["hermes-browser"])),
            Ok(None)
        );
    }

    #[test]
    fn extracts_the_servo_content_process_token() {
        assert_eq!(
            content_process_token(arguments(&[
                "hermes-browser",
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
            content_process_token(arguments(&["hermes-browser", "--content-process"])),
            Err(ContentProcessArgumentError)
        );
        assert_eq!(
            content_process_token(arguments(&["hermes-browser", "--content-process", "",])),
            Err(ContentProcessArgumentError)
        );
    }
}
