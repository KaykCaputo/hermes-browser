use dpi::LogicalSize;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ViewportSize {
    width: u32,
    height: u32,
}

impl ViewportSize {
    pub fn new(width: u32, height: u32) -> Option<Self> {
        (width > 0 && height > 0).then_some(Self { width, height })
    }

    pub fn from_logical(width: f32, height: f32, scale_factor: f32) -> Option<Self> {
        if !width.is_finite()
            || !height.is_finite()
            || !scale_factor.is_finite()
            || width <= 0.0
            || height <= 0.0
            || scale_factor <= 0.0
        {
            return None;
        }

        let logical_size = LogicalSize::new(f64::from(width), f64::from(height));
        let physical_size = logical_size.to_physical::<u32>(f64::from(scale_factor));
        Self::new(physical_size.width, physical_size.height)
    }

    pub fn width(self) -> u32 {
        self.width
    }

    pub fn height(self) -> u32 {
        self.height
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_or_invalid_viewports() {
        assert_eq!(ViewportSize::new(0, 10), None);
        assert_eq!(ViewportSize::from_logical(f32::NAN, 10.0, 1.0), None);
        assert_eq!(ViewportSize::from_logical(10.0, 10.0, 0.0), None);
    }

    #[test]
    fn converts_logical_dimensions_to_physical_pixels() {
        assert_eq!(
            ViewportSize::from_logical(640.0, 360.0, 1.5),
            ViewportSize::new(960, 540)
        );
    }
}
