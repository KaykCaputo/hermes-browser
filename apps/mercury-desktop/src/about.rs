//! The "About Mercury Browser" window. It holds the `AboutSlint` widget that the Slint
//! Royalty-free License 2.0 requires (§2(a)), and is reached from the application menu and
//! from the browser menu.

use std::cell::RefCell;

use mercury_ui::{AboutLink, AboutWindow};
use slint::ComponentHandle;

use crate::menus::{self, Document, MenuCommand};

/// The line `Info.plist` carries as `NSHumanReadableCopyright` (checked by a test).
pub(crate) const COPYRIGHT_LINE: &str =
    "Copyright © 2026 Kayk Caputo and Mercury Browser contributors. Licensed under MPL-2.0.";
/// Servo is pinned to exactly this version in the workspace `Cargo.toml` (checked by a test).
pub(crate) const ENGINE_LINE: &str = "Servo 0.8.0 web engine";

thread_local! {
    static ABOUT: RefCell<Option<AboutWindow>> = const { RefCell::new(None) };
}

pub(crate) fn version_line() -> String {
    format!("Version {}", env!("CARGO_PKG_VERSION"))
}

/// Shown only when the build recorded the GStreamer version (`MERCURY_GSTREAMER_VERSION`).
pub(crate) fn media_line(gstreamer_version: Option<&str>) -> String {
    gstreamer_version
        .filter(|version| !version.is_empty())
        .map(|version| format!("GStreamer {version} media framework"))
        .unwrap_or_default()
}

/// Shows the About window, creating it the first time.
pub(crate) fn show() {
    let existing = ABOUT.with(|about| about.borrow().as_ref().map(ComponentHandle::clone_strong));
    let about = match existing {
        Some(about) => about,
        None => match create() {
            Ok(about) => {
                ABOUT.with(|stored| stored.replace(Some(about.clone_strong())));
                about
            }
            Err(error) => {
                log::warn!("could not open the About window: {error}");
                return;
            }
        },
    };
    if let Err(error) = about.show() {
        log::warn!("could not show the About window: {error}");
    }
}

pub(crate) fn hide() {
    let about = ABOUT.with(|about| about.borrow().as_ref().map(ComponentHandle::clone_strong));
    if let Some(about) = about
        && let Err(error) = about.hide()
    {
        log::warn!("could not close the About window: {error}");
    }
}

/// Renders the open About window into a PNG through the app's own renderer (menu diagnostics).
#[cfg(all(target_os = "macos", feature = "menu-diagnostics"))]
pub(crate) fn save_snapshot(path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    let about = ABOUT
        .with(|about| about.borrow().as_ref().map(ComponentHandle::clone_strong))
        .ok_or("the About window is not open")?;
    let pixels = about.window().take_snapshot()?;
    image::save_buffer_with_format(
        path,
        pixels.as_bytes(),
        pixels.width(),
        pixels.height(),
        image::ExtendedColorType::Rgba8,
        image::ImageFormat::Png,
    )?;
    Ok(())
}

/// Drops the window before the engine and the browser window go away. Called from the end of
/// the run (`post_run.rs`), which may run inside AppKit's terminate callback: it only drops.
pub(crate) fn release() {
    ABOUT.with(|about| about.take());
}

fn create() -> Result<AboutWindow, slint::PlatformError> {
    let about = AboutWindow::new()?;
    // Its own copy of `ChromePalette`, following the appearance setting.
    crate::settings::apply_chrome_theme(&about);
    about.set_version_line(version_line().into());
    about.set_engine_line(ENGINE_LINE.into());
    about.set_media_line(media_line(option_env!("MERCURY_GSTREAMER_VERSION")).into());
    about.set_copyright_line(COPYRIGHT_LINE.into());
    about.on_open_link(|link| {
        let _slint = crate::callback_scope::InSlintCallback::enter();
        let document = match link {
            AboutLink::Licence => Document::Licence,
            AboutLink::ThirdPartyNotices => Document::ThirdPartyNotices,
        };
        menus::dispatch(MenuCommand::OpenDocument(document));
    });
    about.on_close_requested(hide);
    #[cfg(target_os = "macos")]
    menus::reinstall_when_key(about.window(), "the About window became key");
    Ok(about)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_copyright_line_is_the_app_bundles() {
        let template = include_str!("../resources/macos/Info.plist.in");
        assert!(template.contains(&format!(
            "<key>NSHumanReadableCopyright</key>\n\t<string>{COPYRIGHT_LINE}</string>"
        )));
    }

    #[test]
    fn the_engine_line_names_the_pinned_servo() {
        let workspace = include_str!("../../../Cargo.toml");
        assert!(workspace.contains("servo = { version = \"=0.8.0\""));
        assert!(ENGINE_LINE.contains("Servo 0.8.0"));
    }

    #[test]
    fn the_version_is_the_workspace_version() {
        let workspace = include_str!("../../../Cargo.toml");
        let package = workspace
            .split("[workspace.package]")
            .nth(1)
            .and_then(|section| section.split("\n[").next())
            .expect("the workspace has a [workspace.package] section");
        let version = package
            .lines()
            .find_map(|line| line.strip_prefix("version = "))
            .expect("[workspace.package] has a version");
        assert_eq!(version, format!("\"{}\"", env!("CARGO_PKG_VERSION")));
        assert_eq!(
            version_line(),
            format!("Version {}", env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn the_media_line_appears_only_when_the_version_is_known() {
        assert_eq!(media_line(None), "");
        assert_eq!(media_line(Some("")), "");
        assert_eq!(
            media_line(Some("1.28.9")),
            "GStreamer 1.28.9 media framework"
        );
    }
}
