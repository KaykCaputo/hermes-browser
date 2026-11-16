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

Exit status:
  0  Success
  1  Failure, or a screenshot could not be saved
  2  Invalid command line
  3  The screenshot's page did not finish loading
  4  The profile is in use by another Mercury Browser";

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

    Ok(CliOutcome::Run(LaunchOptions {
        urls,
        screenshot,
        window_size,
        background,
        gpu_canvas,
        profile_dir,
        experimental_web_features,
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
