use slint::winit_030::winit::window::Window;
use talaria_servo::ScreenMetrics;

/// The screen and window geometry pages should see, read from the native window and the
/// monitor it is on. `None` when no monitor is known.
///
/// On Wayland the compositor does not reveal window positions, so `outer_position` fails and
/// the window is reported at (0, 0); sizes are still correct.
pub(crate) fn screen_metrics(window: &Window) -> Option<ScreenMetrics> {
    let monitor = window
        .current_monitor()
        .or_else(|| window.primary_monitor())?;
    let screen_size = monitor.size();
    let screen_origin = monitor.position();
    let window_origin = window.outer_position().unwrap_or_default();
    let window_size = window.outer_size();

    let whole_screen = (
        (screen_origin.x, screen_origin.y),
        (screen_size.width, screen_size.height),
    );
    // Only macOS reports the area outside the menu bar and Dock; elsewhere it is the whole screen.
    #[cfg(target_os = "macos")]
    let (available_origin, available_size) =
        macos_available_area(&monitor, (screen_origin.x, screen_origin.y)).unwrap_or(whole_screen);
    #[cfg(not(target_os = "macos"))]
    let (available_origin, available_size) = whole_screen;

    Some(ScreenMetrics {
        screen_size: (screen_size.width, screen_size.height),
        available_origin,
        available_size,
        window_origin: (window_origin.x, window_origin.y),
        window_size: (window_size.width, window_size.height),
    })
}

/// The monitor's area outside the menu bar and Dock (`NSScreen.visibleFrame`).
#[cfg(target_os = "macos")]
fn macos_available_area(
    monitor: &slint::winit_030::winit::monitor::MonitorHandle,
    screen_origin: (i32, i32),
) -> Option<((i32, i32), (u32, u32))> {
    use objc2_app_kit::NSScreen;
    use objc2_foundation::{MainThreadMarker, NSNumber, NSString};
    use slint::winit_030::winit::platform::macos::MonitorHandleExtMacOS;

    // AppKit screens may only be used on the main thread; this is checked, not assumed.
    let main_thread = MainThreadMarker::new()?;
    let screen_number_key = NSString::from_str("NSScreenNumber");
    let screens = NSScreen::screens(main_thread);
    let screen = screen_with_display_id(
        screens.iter().map(|screen| {
            let number = screen
                .deviceDescription()
                .objectForKey(&screen_number_key)
                .and_then(|number| number.downcast::<NSNumber>().ok())
                .map(|number| number.unsignedIntValue());
            (number, screen)
        }),
        monitor.native_id(),
    )?;
    let (frame, visible_frame) = (screen.frame(), screen.visibleFrame());
    Some(available_area(
        PointRect {
            x: frame.origin.x,
            y: frame.origin.y,
            width: frame.size.width,
            height: frame.size.height,
        },
        PointRect {
            x: visible_frame.origin.x,
            y: visible_frame.origin.y,
            width: visible_frame.size.width,
            height: visible_frame.size.height,
        },
        monitor.scale_factor(),
        screen_origin,
    ))
}

/// A rectangle in macOS screen points: origin at the bottom left, y growing upwards.
#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, Debug, PartialEq)]
struct PointRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

/// Converts the visible part of a screen into device pixels, with its origin in the same
/// top-left, y-down coordinates as the screen's own `screen_origin`.
#[cfg(any(target_os = "macos", test))]
fn available_area(
    screen_frame: PointRect,
    visible_frame: PointRect,
    scale_factor: f64,
    screen_origin: (i32, i32),
) -> ((i32, i32), (u32, u32)) {
    let to_pixels = |points: f64| (points * scale_factor).round();
    let left_inset = to_pixels(visible_frame.x - screen_frame.x);
    let top_inset = to_pixels(
        (screen_frame.y + screen_frame.height) - (visible_frame.y + visible_frame.height),
    );
    (
        (
            screen_origin.0.saturating_add(left_inset as i32),
            screen_origin.1.saturating_add(top_inset as i32),
        ),
        (
            to_pixels(visible_frame.width).max(0.0) as u32,
            to_pixels(visible_frame.height).max(0.0) as u32,
        ),
    )
}

/// The item whose display number equals `display_id`; items without a number never match.
#[cfg(any(target_os = "macos", test))]
fn screen_with_display_id<T>(
    screens: impl IntoIterator<Item = (Option<u32>, T)>,
    display_id: u32,
) -> Option<T> {
    screens
        .into_iter()
        .find_map(|(number, screen)| (number == Some(display_id)).then_some(screen))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_screen_with_the_monitors_display_id_is_chosen() {
        let screens = [
            (Some(1), "built-in"),
            (None, "unknown"),
            (Some(7), "external"),
        ];

        assert_eq!(screen_with_display_id(screens, 7), Some("external"));
        assert_eq!(screen_with_display_id(screens, 1), Some("built-in"));
        assert_eq!(screen_with_display_id(screens, 3), None);
    }

    #[test]
    fn the_menu_bar_and_a_bottom_dock_are_excluded() {
        let screen = PointRect {
            x: 0.0,
            y: 0.0,
            width: 1512.0,
            height: 982.0,
        };
        // Dock 70 points high at the bottom, menu bar 37 points at the top.
        let visible = PointRect {
            x: 0.0,
            y: 70.0,
            width: 1512.0,
            height: 875.0,
        };

        assert_eq!(
            available_area(screen, visible, 2.0, (0, 0)),
            ((0, 74), (3024, 1750))
        );
    }

    #[test]
    fn a_left_dock_on_a_secondary_screen_moves_the_origin() {
        let screen = PointRect {
            x: 1512.0,
            y: -200.0,
            width: 1920.0,
            height: 1080.0,
        };
        let visible = PointRect {
            x: 1572.0,
            y: -200.0,
            width: 1860.0,
            height: 1055.0,
        };

        assert_eq!(
            available_area(screen, visible, 1.0, (3024, 100)),
            ((3084, 125), (1860, 1055))
        );
    }
}
