//! The file chooser for `<input type=file>`: the system open panel, attached to the browser
//! window as a sheet. Only the files the user chose are passed on.

use std::path::PathBuf;

use mercury_ui::MainWindow;

/// Called once, on the main thread, with the chosen files (empty if the user cancelled).
pub(crate) type FilesChosen = Box<dyn FnOnce(Vec<PathBuf>)>;

/// The panel's message, naming the page that asked.
fn panel_message(multiple: bool, origin: &str) -> String {
    let what = if multiple { "files" } else { "a file" };
    format!("Choose {what} for {origin}")
}

#[cfg(target_os = "macos")]
pub(crate) fn choose_files(
    window: &MainWindow,
    multiple: bool,
    extensions: &[String],
    origin: &str,
    done: FilesChosen,
) {
    use std::cell::Cell;

    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2_app_kit::{NSModalResponse, NSModalResponseOK, NSOpenPanel, NSView, NSWindow};
    use objc2_foundation::{MainThreadMarker, NSArray, NSString};
    use slint::ComponentHandle;
    use slint::winit_030::WinitWindowAccessor;
    use slint::winit_030::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let Some(main_thread) = MainThreadMarker::new() else {
        log::error!("the file panel can only be opened on the main thread");
        done(Vec::new());
        return;
    };
    let ns_window: Option<Retained<NSWindow>> = window
        .window()
        .with_winit_window(|winit_window| {
            let handle = winit_window.window_handle().ok()?;
            let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
                return None;
            };
            // SAFETY: winit's AppKit handle points at the window's content view, which lives as
            // long as the window; it is only used here, on the main thread, to find its window.
            let view = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
            view.window()
        })
        .flatten();
    let Some(ns_window) = ns_window else {
        log::error!("the browser window has no AppKit window to attach the file panel to");
        done(Vec::new());
        return;
    };

    let panel = NSOpenPanel::openPanel(main_thread);
    panel.setCanChooseFiles(true);
    panel.setCanChooseDirectories(false);
    panel.setAllowsMultipleSelection(multiple);
    panel.setMessage(Some(&NSString::from_str(&panel_message(multiple, origin))));
    if !extensions.is_empty() {
        let types = extensions
            .iter()
            .map(|extension| NSString::from_str(extension))
            .collect::<Vec<_>>();
        // `allowedContentTypes` needs `UTType` from `objc2-uniform-type-identifiers` 0.3, a
        // package not otherwise in the build; the older file-type list still works on current
        // macOS releases.
        #[expect(deprecated)]
        panel.setAllowedFileTypes(Some(&NSArray::from_retained_slice(&types)));
    }

    let done = Cell::new(Some(done));
    let chosen_panel = panel.clone();
    let handler = RcBlock::new(move |response: NSModalResponse| {
        let paths = if response == NSModalResponseOK {
            chosen_panel
                .URLs()
                .iter()
                .filter_map(|url| url.to_file_path())
                .collect()
        } else {
            Vec::new()
        };
        if let Some(done) = done.take() {
            done(paths);
        }
    });
    // Returns at once; AppKit calls the handler on the main thread when the sheet closes.
    panel.beginSheetModalForWindow_completionHandler(&ns_window, &handler);
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn choose_files(
    _window: &MainWindow,
    multiple: bool,
    _extensions: &[String],
    origin: &str,
    done: FilesChosen,
) {
    log::warn!(
        "no file chooser on this platform yet; answering \"no files\" ({})",
        panel_message(multiple, origin)
    );
    done(Vec::new());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_panel_names_the_page_that_asked() {
        assert_eq!(
            panel_message(false, "https://upload.example"),
            "Choose a file for https://upload.example"
        );
        assert_eq!(
            panel_message(true, "https://upload.example"),
            "Choose files for https://upload.example"
        );
    }
}
