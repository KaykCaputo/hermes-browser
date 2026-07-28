#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageIcon {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl PageIcon {
    pub fn from_rgba(width: u32, height: u32, rgba: Vec<u8>) -> Option<Self> {
        let expected_length = width.checked_mul(height)?.checked_mul(4)? as usize;

        if width == 0 || height == 0 || rgba.len() != expected_length {
            return None;
        }

        Some(Self {
            width,
            height,
            rgba,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_rgba_pixels_create_an_icon() {
        let icon = PageIcon::from_rgba(1, 1, vec![10, 20, 30, 255])
            .expect("one RGBA pixel should be valid");

        assert_eq!(icon.width(), 1);
        assert_eq!(icon.height(), 1);
        assert_eq!(icon.rgba(), &[10, 20, 30, 255]);
    }

    #[test]
    fn zero_dimensions_are_rejected() {
        assert_eq!(PageIcon::from_rgba(0, 1, Vec::new()), None);
        assert_eq!(PageIcon::from_rgba(1, 0, Vec::new()), None);
    }

    #[test]
    fn mismatched_pixel_length_is_rejected() {
        assert_eq!(PageIcon::from_rgba(1, 1, vec![10, 20, 30]), None);
    }

    #[test]
    fn overflowing_dimensions_are_rejected() {
        assert_eq!(PageIcon::from_rgba(u32::MAX, u32::MAX, Vec::new()), None);
    }
}
