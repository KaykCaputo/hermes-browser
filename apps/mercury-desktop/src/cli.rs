use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct LaunchOptions {
    pub urls: Vec<String>,
    pub screenshot: Option<ScreenshotOptions>,
    pub window_size: Option<(u32, u32)>,
    pub background: bool,
    pub gpu_canvas: bool,
    pub profile_dir: Option<PathBuf>,
    pub experimental_web_features: bool,
    pub exit_request: Option<ExitRequest>,
}

/// Diagnostics: a quit the browser requests of itself, so that quit paths can be exercised
/// without anyone at the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ExitRequest {
    /// Counted from just before the event loop starts.
    pub after: Duration,
    pub by: ExitBy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExitBy {
    /// The window's close request, as the title bar's close button sends it.
    CloseRequest,
    /// `slint::quit_event_loop()`, as the quit shortcut calls it.
    Quit,
    /// macOS: `-[NSApplication terminate:]`, as a Quit menu item sends it.
    Terminate,
    /// macOS: a quit Apple event to this process whose reason is `kAEReallyLogOut`, as an
    /// application gets when the user logs out.
    LogOut,
}

impl Default for LaunchOptions {
    fn default() -> Self {
        Self {
            urls: Vec::new(),
            screenshot: None,
            window_size: None,
            background: false,
            gpu_canvas: true,
            profile_dir: None,
            experimental_web_features: true,
            exit_request: None,
        }
    }
}

impl LaunchOptions {
    /// Screenshot and background runs are automated: they never use the user's own profile.
    pub(crate) fn is_automated(&self) -> bool {
        self.screenshot.is_some() || self.background
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ScreenshotOptions {
    pub path: PathBuf,
    pub delay: Duration,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CliOutcome {
    Run(LaunchOptions),
    PrintAndExit { text: String, code: i32 },
}

/// `--exit-by logout` sends the browser a log-out quit event; only builds with the
/// `quit-diagnostics` feature (never released ones) have it.
const LOG_OUT_DIAGNOSTIC: bool = cfg!(all(target_os = "macos", feature = "quit-diagnostics"));

const EXIT_BY_VALUES: &str = if LOG_OUT_DIAGNOSTIC {
    "close-request|quit|terminate|logout"
} else if cfg!(target_os = "macos") {
    "close-request|quit|terminate"
} else {
    "close-request|quit"
};

const DEFAULT_SCREENSHOT_DELAY: Duration = Duration::from_millis(1000);

const USAGE: &str = "\
Usage: mercury-browser [OPTIONS] [URL...]

Opens each URL in its own tab; the first one is shown.

Options:
  --screenshot <file.png>          Save a PNG of the window once the page has loaded, then exit
  --screenshot-delay <milliseconds>
                                   Wait this long after the page has loaded (default 1000)
  --window-size <W>x<H>            Set the window's logical size, for example 1280x800
  --background                     Start without taking focus (for automated runs)
  --disable-gpu-canvas             Draw 2D canvases on the CPU
  --disable-experimental-web-features
                                   Leave off Servo's experimental web-platform features
                                   (servoshell's list minus the async clipboard and
                                   WebGPU, which are always off)
                                   Both --disable options turn the feature off for this
                                   run whatever the setting says; they cannot turn on a
                                   feature the settings turned off
  --profile-dir <directory>        Keep cookies and site data in this directory
                                   (screenshot and background runs otherwise use a temporary one)
  --help                           Print this help and exit
  --version                        Print the version and exit

Diagnostics:
  --exit-after <milliseconds>      Request a quit from inside the browser this long after
                                   start-up (a screenshot run then exits 1: no screenshot)
  --exit-by close-request|quit|terminate|logout
                                   How --exit-after quits: through the window's close
                                   request, as the close button does; as the quit
                                   shortcut does (default quit); on macOS only, through
                                   the system's terminate request, as a Quit menu item
                                   sends it (terminate); in diagnostic builds only, the
                                   quit Apple event of a log-out, sent to this process
                                   only (logout)

Exit status:
  0  Success
  1  Failure, or a screenshot could not be saved
  2  Invalid command line
  3  The screenshot's page did not finish loading
  4  The profile is in use by another Mercury Browser
  On SIGTERM, SIGINT or SIGHUP the browser quits in order and then ends by that same
  signal (a shell reports 128 + its number: 143, 130, 129); a second one ends it at once.";

/// `--profile-dir` and `--screenshot` need paths that are valid UTF-8; anything else would be
/// silently altered by the conversion to `String`. Returns the usage error to print, if any.
pub(crate) fn reject_non_utf8_paths(arguments: &[std::ffi::OsString]) -> Option<CliOutcome> {
    let pair = arguments.windows(2).find(|pair| {
        (pair[0] == "--profile-dir" || pair[0] == "--screenshot") && pair[1].to_str().is_none()
    })?;
    Some(CliOutcome::PrintAndExit {
        text: format!(
            "mercury-browser: the {} path '{}' is not valid UTF-8; choose another path\n\n{USAGE}",
            pair[0].to_string_lossy(),
            pair[1].to_string_lossy()
        ),
        code: 2,
    })
}

/// Parses the command line, excluding the program name.
pub(crate) fn parse_arguments(arguments: impl IntoIterator<Item = String>) -> CliOutcome {
    match parse_launch_options(arguments.into_iter()) {
        Ok(outcome) => outcome,
        Err(message) => CliOutcome::PrintAndExit {
            text: format!("mercury-browser: {message}\n\n{USAGE}"),
            code: 2,
        },
    }
}

fn parse_launch_options(mut arguments: impl Iterator<Item = String>) -> Result<CliOutcome, String> {
    let mut urls = Vec::new();
    let mut screenshot_path = None;
    let mut screenshot_delay = None;
    let mut window_size = None;
    let mut background = false;
    let mut gpu_canvas = true;
    let mut profile_dir = None;
    let mut experimental_web_features = true;
    let mut exit_after = None;
    let mut exit_by = None;

    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--help" => {
                return Ok(CliOutcome::PrintAndExit {
                    text: USAGE.to_owned(),
                    code: 0,
                });
            }
            "--version" => {
                return Ok(CliOutcome::PrintAndExit {
                    text: format!("Mercury Browser {}", env!("CARGO_PKG_VERSION")),
                    code: 0,
                });
            }
            "--screenshot" => {
                let value = option_value(&mut arguments, "--screenshot", "<file.png>")?;
                screenshot_path = Some(PathBuf::from(value));
            }
            "--screenshot-delay" => {
                let value = option_value(&mut arguments, "--screenshot-delay", "<milliseconds>")?;
                let milliseconds = value.parse::<u64>().map_err(|_| {
                    format!("invalid --screenshot-delay '{value}': expected milliseconds")
                })?;
                screenshot_delay = Some(Duration::from_millis(milliseconds));
            }
            "--window-size" => {
                let value = option_value(&mut arguments, "--window-size", "<W>x<H>")?;
                window_size = Some(parse_window_size(&value).ok_or_else(|| {
                    format!(
                        "invalid --window-size '{value}': expected <W>x<H>, for example 1280x800"
                    )
                })?);
            }
            "--background" => background = true,
            "--disable-gpu-canvas" => gpu_canvas = false,
            "--disable-experimental-web-features" => experimental_web_features = false,
            "--profile-dir" => {
                let value = option_value(&mut arguments, "--profile-dir", "<directory>")?;
                profile_dir = Some(PathBuf::from(value));
            }
            "--exit-after" => {
                let value = option_value(&mut arguments, "--exit-after", "<milliseconds>")?;
                let milliseconds = value.parse::<u64>().map_err(|_| {
                    format!("invalid --exit-after '{value}': expected milliseconds")
                })?;
                exit_after = Some(Duration::from_millis(milliseconds));
            }
            "--exit-by" => {
                let value = option_value(&mut arguments, "--exit-by", EXIT_BY_VALUES)?;
                exit_by = Some(match value.as_str() {
                    "close-request" => ExitBy::CloseRequest,
                    "quit" => ExitBy::Quit,
                    "terminate" if cfg!(target_os = "macos") => ExitBy::Terminate,
                    "logout" if LOG_OUT_DIAGNOSTIC => ExitBy::LogOut,
                    _ => {
                        return Err(format!(
                            "invalid --exit-by '{value}': expected {EXIT_BY_VALUES}"
                        ));
                    }
                });
            }
            _ if argument.starts_with("-psn_") => {}
            _ if argument.starts_with('-') => {
                return Err(format!("unknown option '{argument}'"));
            }
            _ => urls.push(argument),
        }
    }

    let screenshot = match (screenshot_path, screenshot_delay) {
        (Some(path), delay) => Some(ScreenshotOptions {
            path,
            delay: delay.unwrap_or(DEFAULT_SCREENSHOT_DELAY),
        }),
        (None, Some(_)) => return Err("--screenshot-delay requires --screenshot".to_owned()),
        (None, None) => None,
    };
    let exit_request = match (exit_after, exit_by) {
        (Some(after), by) => Some(ExitRequest {
            after,
            by: by.unwrap_or(ExitBy::Quit),
        }),
        (None, Some(_)) => return Err("--exit-by requires --exit-after".to_owned()),
        (None, None) => None,
    };

    Ok(CliOutcome::Run(LaunchOptions {
        urls,
        screenshot,
        window_size,
        background,
        gpu_canvas,
        profile_dir,
        experimental_web_features,
        exit_request,
    }))
}

fn option_value(
    arguments: &mut impl Iterator<Item = String>,
    option: &str,
    placeholder: &str,
) -> Result<String, String> {
    arguments
        .next()
        .filter(|value| !value.starts_with("--"))
        .ok_or_else(|| format!("{option} requires a value {placeholder}"))
}

fn parse_window_size(value: &str) -> Option<(u32, u32)> {
    let (width, height) = value.split_once('x')?;
    let width = parse_dimension(width)?;
    let height = parse_dimension(height)?;
    Some((width, height))
}

fn parse_dimension(value: &str) -> Option<u32> {
    if !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse::<u32>().ok().filter(|dimension| *dimension > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(arguments: &[&str]) -> CliOutcome {
        parse_arguments(arguments.iter().map(|argument| (*argument).to_owned()))
    }

    fn exit_code(outcome: &CliOutcome) -> Option<i32> {
        match outcome {
            CliOutcome::Run(_) => None,
            CliOutcome::PrintAndExit { code, .. } => Some(*code),
        }
    }

    #[test]
    fn no_arguments_run_with_no_urls() {
        assert_eq!(parse(&[]), CliOutcome::Run(LaunchOptions::default()));
    }

    #[test]
    fn urls_are_kept_in_order() {
        assert_eq!(
            parse(&["https://a.test", "b.test"]),
            CliOutcome::Run(LaunchOptions {
                urls: vec!["https://a.test".to_owned(), "b.test".to_owned()],
                ..LaunchOptions::default()
            })
        );
    }

    #[test]
    fn screenshot_options_and_window_size_are_parsed() {
        assert_eq!(
            parse(&[
                "--screenshot",
                "out.png",
                "--screenshot-delay",
                "250",
                "--window-size",
                "800x600",
                "https://a.test",
            ]),
            CliOutcome::Run(LaunchOptions {
                urls: vec!["https://a.test".to_owned()],
                screenshot: Some(ScreenshotOptions {
                    path: PathBuf::from("out.png"),
                    delay: Duration::from_millis(250),
                }),
                window_size: Some((800, 600)),
                background: false,
                gpu_canvas: true,
                profile_dir: None,
                experimental_web_features: true,
                exit_request: None,
            })
        );
    }

    #[test]
    fn screenshot_delay_defaults_to_one_second() {
        let CliOutcome::Run(options) = parse(&["--screenshot", "out.png"]) else {
            panic!("a screenshot path alone should be accepted");
        };

        assert_eq!(
            options.screenshot,
            Some(ScreenshotOptions {
                path: PathBuf::from("out.png"),
                delay: Duration::from_secs(1),
            })
        );
    }

    #[test]
    fn window_size_without_a_height_is_a_usage_error() {
        assert_eq!(exit_code(&parse(&["--window-size", "800"])), Some(2));
    }

    #[test]
    fn invalid_window_sizes_are_usage_errors() {
        for size in [
            "0x600",
            "800x0",
            "x600",
            "800x",
            "800x600x2",
            "-800x600",
            "wide",
        ] {
            assert_eq!(
                exit_code(&parse(&["--window-size", size])),
                Some(2),
                "{size}"
            );
        }
    }

    #[test]
    fn screenshot_without_a_value_is_a_usage_error() {
        assert_eq!(exit_code(&parse(&["--screenshot"])), Some(2));
    }

    #[test]
    fn invalid_screenshot_delay_is_a_usage_error() {
        assert_eq!(
            exit_code(&parse(&[
                "--screenshot",
                "out.png",
                "--screenshot-delay",
                "soon"
            ])),
            Some(2)
        );
        assert_eq!(
            exit_code(&parse(&["--screenshot", "out.png", "--screenshot-delay"])),
            Some(2)
        );
    }

    #[test]
    fn screenshot_delay_without_screenshot_is_a_usage_error() {
        assert_eq!(exit_code(&parse(&["--screenshot-delay", "250"])), Some(2));
    }

    #[test]
    fn unknown_option_is_a_usage_error_with_usage_text() {
        let outcome = parse(&["--bogus"]);

        let CliOutcome::PrintAndExit { text, code } = outcome else {
            panic!("an unknown option should not run the browser");
        };
        assert_eq!(code, 2);
        assert!(
            text.lines()
                .next()
                .is_some_and(|line| line.contains("--bogus"))
        );
        assert!(text.contains("Usage: mercury-browser [OPTIONS] [URL...]"));
    }

    #[test]
    fn version_prints_the_product_name_and_version() {
        let CliOutcome::PrintAndExit { text, code } = parse(&["--version"]) else {
            panic!("--version should not run the browser");
        };

        assert_eq!(code, 0);
        assert!(text.contains("Mercury Browser 0.2.0"));
    }

    #[test]
    fn help_prints_usage_and_succeeds() {
        let CliOutcome::PrintAndExit { text, code } = parse(&["--help"]) else {
            panic!("--help should not run the browser");
        };

        assert_eq!(code, 0);
        assert!(text.contains("Usage: mercury-browser [OPTIONS] [URL...]"));
        assert!(text.contains("--screenshot <file.png>"));
    }

    #[test]
    fn background_is_off_by_default_and_parsed() {
        let CliOutcome::Run(default_options) = parse(&["https://a.test"]) else {
            panic!("a URL alone should run the browser");
        };
        assert!(!default_options.background);

        assert_eq!(
            parse(&["--background", "https://a.test"]),
            CliOutcome::Run(LaunchOptions {
                urls: vec!["https://a.test".to_owned()],
                background: true,
                ..LaunchOptions::default()
            })
        );
    }

    #[test]
    fn help_describes_background() {
        let CliOutcome::PrintAndExit { text, .. } = parse(&["--help"]) else {
            panic!("--help should not run the browser");
        };

        assert!(text.contains("--background"));
        assert!(text.contains("Start without taking focus (for automated runs)"));
    }

    #[test]
    fn gpu_canvas_is_on_by_default_and_disable_gpu_canvas_turns_it_off() {
        let CliOutcome::Run(default_options) = parse(&["https://a.test"]) else {
            panic!("a URL alone should run the browser");
        };
        assert!(default_options.gpu_canvas);

        assert_eq!(
            parse(&["--disable-gpu-canvas", "https://a.test"]),
            CliOutcome::Run(LaunchOptions {
                urls: vec!["https://a.test".to_owned()],
                gpu_canvas: false,
                ..LaunchOptions::default()
            })
        );
    }

    #[test]
    fn help_describes_disable_gpu_canvas() {
        let CliOutcome::PrintAndExit { text, .. } = parse(&["--help"]) else {
            panic!("--help should not run the browser");
        };

        assert!(text.contains("--disable-gpu-canvas"));
        assert!(text.contains("Draw 2D canvases on the CPU"));
    }

    #[test]
    fn profile_dir_is_parsed() {
        assert_eq!(
            parse(&["--profile-dir", "/x", "https://a.test"]),
            CliOutcome::Run(LaunchOptions {
                urls: vec!["https://a.test".to_owned()],
                profile_dir: Some(PathBuf::from("/x")),
                ..LaunchOptions::default()
            })
        );
    }

    #[test]
    fn help_describes_profile_dir() {
        let CliOutcome::PrintAndExit { text, .. } = parse(&["--help"]) else {
            panic!("--help should not run the browser");
        };

        assert!(text.contains("--profile-dir <directory>"));
    }

    #[test]
    fn screenshot_and_background_runs_are_automated() {
        let automated = |arguments: &[&str]| {
            let CliOutcome::Run(options) = parse(arguments) else {
                panic!("these arguments should run the browser");
            };
            options.is_automated()
        };

        assert!(!automated(&[]));
        assert!(!automated(&["--profile-dir", "/x"]));
        assert!(automated(&["--screenshot", "out.png"]));
        assert!(automated(&["--background"]));
    }

    #[test]
    fn help_says_automated_runs_use_a_temporary_profile() {
        let CliOutcome::PrintAndExit { text, .. } = parse(&["--help"]) else {
            panic!("--help should not run the browser");
        };

        assert!(text.contains("screenshot and background runs otherwise use a temporary one"));
    }

    #[test]
    fn help_documents_the_profile_in_use_exit_status() {
        let CliOutcome::PrintAndExit { text, .. } = parse(&["--help"]) else {
            panic!("--help should not run the browser");
        };

        assert!(text.contains(&format!(
            "  {}  The profile is in use by another Mercury Browser",
            crate::profile::EXIT_PROFILE_IN_USE
        )));
    }

    #[cfg(unix)]
    #[test]
    fn path_options_that_are_not_utf8_are_usage_errors() {
        use std::ffi::{OsStr, OsString};
        use std::os::unix::ffi::OsStrExt;

        let arguments = [
            OsString::from("--profile-dir"),
            OsStr::from_bytes(b"/tmp/\xffprofile").to_owned(),
        ];

        assert_eq!(
            reject_non_utf8_paths(&arguments)
                .as_ref()
                .and_then(exit_code),
            Some(2)
        );
        assert_eq!(
            reject_non_utf8_paths(&[OsString::from("--profile-dir"), OsString::from("/x")]),
            None
        );
        assert_eq!(
            reject_non_utf8_paths(&[
                OsString::from("--screenshot"),
                OsStr::from_bytes(b"\xff.png").to_owned()
            ])
            .as_ref()
            .and_then(exit_code),
            Some(2)
        );
        assert_eq!(
            reject_non_utf8_paths(&[OsString::from("--screenshot"), OsString::from("out.png")]),
            None
        );
    }

    #[test]
    fn help_says_the_disable_options_win_over_the_settings() {
        let CliOutcome::PrintAndExit { text, .. } = parse(&["--help"]) else {
            panic!("--help should not run the browser");
        };

        assert!(text.contains("run whatever the setting says"));
    }

    #[test]
    fn profile_dir_without_a_value_is_a_usage_error() {
        assert_eq!(exit_code(&parse(&["--profile-dir"])), Some(2));
    }

    #[test]
    fn experimental_web_features_are_on_by_default_and_can_be_disabled() {
        let CliOutcome::Run(default_options) = parse(&["https://a.test"]) else {
            panic!("a URL alone should run the browser");
        };
        assert!(default_options.experimental_web_features);

        assert_eq!(
            parse(&["--disable-experimental-web-features", "https://a.test"]),
            CliOutcome::Run(LaunchOptions {
                urls: vec!["https://a.test".to_owned()],
                experimental_web_features: false,
                ..LaunchOptions::default()
            })
        );
    }

    #[test]
    fn help_describes_disable_experimental_web_features() {
        let CliOutcome::PrintAndExit { text, .. } = parse(&["--help"]) else {
            panic!("--help should not run the browser");
        };

        assert!(text.contains("--disable-experimental-web-features"));
    }

    #[test]
    fn exit_after_is_parsed_and_quits_by_default() {
        let CliOutcome::Run(options) = parse(&["--background", "--exit-after", "300"]) else {
            panic!("--exit-after with a delay should run the browser");
        };

        assert_eq!(
            options.exit_request,
            Some(ExitRequest {
                after: Duration::from_millis(300),
                by: ExitBy::Quit,
            })
        );
        assert_eq!(LaunchOptions::default().exit_request, None);
    }

    #[test]
    fn exit_by_chooses_the_close_request_or_the_quit() {
        let exit_by = |value: &str| {
            let CliOutcome::Run(options) = parse(&["--exit-by", value, "--exit-after", "0"]) else {
                panic!("--exit-by {value} should run the browser");
            };
            options.exit_request.map(|request| request.by)
        };

        assert_eq!(exit_by("close-request"), Some(ExitBy::CloseRequest));
        assert_eq!(exit_by("quit"), Some(ExitBy::Quit));
    }

    #[test]
    fn exit_by_terminate_is_macos_only_and_logout_needs_a_diagnostic_build() {
        for (value, expected, available) in [
            ("terminate", ExitBy::Terminate, cfg!(target_os = "macos")),
            ("logout", ExitBy::LogOut, LOG_OUT_DIAGNOSTIC),
        ] {
            let outcome = parse(&["--exit-after", "0", "--exit-by", value]);
            if available {
                let CliOutcome::Run(options) = outcome else {
                    panic!("--exit-by {value} should run the browser on macOS");
                };
                assert_eq!(
                    options.exit_request.map(|request| request.by),
                    Some(expected)
                );
            } else {
                assert_eq!(exit_code(&outcome), Some(2), "{value}");
            }
        }
    }

    #[test]
    fn exit_after_works_with_a_screenshot() {
        let CliOutcome::Run(options) = parse(&[
            "--screenshot",
            "out.png",
            "--exit-after",
            "50",
            "--exit-by",
            "close-request",
        ]) else {
            panic!("--exit-after should be accepted with --screenshot");
        };

        assert!(options.screenshot.is_some());
        assert_eq!(
            options.exit_request,
            Some(ExitRequest {
                after: Duration::from_millis(50),
                by: ExitBy::CloseRequest,
            })
        );
    }

    #[test]
    fn invalid_exit_requests_are_usage_errors() {
        for arguments in [
            &["--exit-after"][..],
            &["--exit-after", "soon"],
            &["--exit-after", "-5"],
            &["--exit-after", "100", "--exit-by"],
            &["--exit-after", "100", "--exit-by", "kill"],
            &["--exit-by", "quit"],
        ] {
            assert_eq!(exit_code(&parse(arguments)), Some(2), "{arguments:?}");
        }
    }

    #[test]
    fn help_documents_how_quit_signals_end_the_browser() {
        let CliOutcome::PrintAndExit { text, .. } = parse(&["--help"]) else {
            panic!("--help should not run the browser");
        };

        let exit_status = text
            .split_once("Exit status:")
            .map(|(_, rest)| rest)
            .expect("an exit status section");
        assert!(exit_status.contains("SIGTERM, SIGINT or SIGHUP"));
        assert!(exit_status.contains("ends by that same"));
    }

    #[test]
    fn help_lists_the_exit_request_under_diagnostics() {
        let CliOutcome::PrintAndExit { text, .. } = parse(&["--help"]) else {
            panic!("--help should not run the browser");
        };

        let diagnostics = text
            .split_once("Diagnostics:")
            .map(|(_, rest)| rest)
            .expect("a diagnostics heading");
        assert!(diagnostics.contains("--exit-after <milliseconds>"));
        assert!(diagnostics.contains("--exit-by close-request|quit|terminate|logout"));
    }

    #[test]
    fn macos_process_serial_number_is_ignored() {
        assert_eq!(
            parse(&["-psn_0_123", "https://a.test"]),
            CliOutcome::Run(LaunchOptions {
                urls: vec!["https://a.test".to_owned()],
                ..LaunchOptions::default()
            })
        );
    }
}
