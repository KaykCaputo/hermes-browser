use servo::{DeviceIntPoint, DeviceIntRect, DeviceIntSize, ScreenGeometry};

/// Screen and window geometry in device pixels, as the page's `window.screen` and
/// `outerWidth`/`screenX` should report them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenMetrics {
    pub screen_size: (u32, u32),
    /// Top-left of the area not covered by the menu bar, Dock or task bar. Servo 0.8.0 has no
    /// field for it (`screen.availLeft`/`availTop` are not implemented), so it is not passed on.
    pub available_origin: (i32, i32),
    pub available_size: (u32, u32),
    pub window_origin: (i32, i32),
    pub window_size: (u32, u32),
}

impl ScreenMetrics {
    pub(crate) fn to_servo_geometry(self) -> ScreenGeometry {
        ScreenGeometry {
            size: device_size(self.screen_size),
            available_size: device_size(self.available_size),
            window_rect: DeviceIntRect::from_origin_and_size(
                DeviceIntPoint::new(self.window_origin.0, self.window_origin.1),
                device_size(self.window_size),
            ),
        }
    }
}

fn device_size((width, height): (u32, u32)) -> DeviceIntSize {
    let clamp = |value: u32| i32::try_from(value).unwrap_or(i32::MAX);
    DeviceIntSize::new(clamp(width), clamp(height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_metrics_convert_to_servo_geometry() {
        let metrics = ScreenMetrics {
            screen_size: (3024, 1964),
            available_origin: (0, 74),
            available_size: (3024, 1780),
            window_origin: (100, 200),
            window_size: (2560, 1600),
        };

        let geometry = metrics.to_servo_geometry();

        assert_eq!(geometry.size, DeviceIntSize::new(3024, 1964));
        assert_eq!(geometry.available_size, DeviceIntSize::new(3024, 1780));
        assert_eq!(
            geometry.window_rect,
            DeviceIntRect::from_origin_and_size(
                DeviceIntPoint::new(100, 200),
                DeviceIntSize::new(2560, 1600)
            )
        );
    }

    #[test]
    fn oversized_dimensions_saturate_instead_of_wrapping() {
        let metrics = ScreenMetrics {
            screen_size: (u32::MAX, 1),
            available_origin: (0, 0),
            available_size: (1, 1),
            window_origin: (0, 0),
            window_size: (1, 1),
        };

        assert_eq!(metrics.to_servo_geometry().size.width, i32::MAX);
    }
}
