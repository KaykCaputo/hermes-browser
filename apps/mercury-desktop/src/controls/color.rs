//! The colour picker for `<input type=color>`: swatches, a hue/lightness area and a hex field.

use talaria_core::{ControlAnswer, Rgb};

use super::Outcome;

const fn rgb(red: u8, green: u8, blue: u8) -> Rgb {
    Rgb { red, green, blue }
}

/// Common colours, in two rows of eight; `ui/controls/color-picker.slint` lays them out.
pub(crate) const SWATCHES: [Rgb; 16] = [
    rgb(0x00, 0x00, 0x00),
    rgb(0x55, 0x55, 0x55),
    rgb(0xaa, 0xaa, 0xaa),
    rgb(0xff, 0xff, 0xff),
    rgb(0xe5, 0x39, 0x35),
    rgb(0xfb, 0x8c, 0x00),
    rgb(0xfd, 0xd8, 0x35),
    rgb(0x43, 0xa0, 0x47),
    rgb(0x00, 0x89, 0x7b),
    rgb(0x00, 0xac, 0xc1),
    rgb(0x1e, 0x88, 0xe5),
    rgb(0x39, 0x49, 0xab),
    rgb(0x8e, 0x24, 0xaa),
    rgb(0xd8, 0x1b, 0x60),
    rgb(0x6d, 0x4c, 0x41),
    rgb(0x54, 0x6e, 0x7a),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ColorKey {
    Return,
    Escape,
    Backspace,
    Character(char),
}

#[derive(Debug)]
pub(crate) struct ColorModel {
    colour: Rgb,
    hex: String,
}

impl ColorModel {
    pub(crate) fn new(current: Option<Rgb>) -> Self {
        let colour = current.unwrap_or(rgb(0, 0, 0));
        Self {
            colour,
            hex: format_hex(colour),
        }
    }

    pub(crate) fn colour(&self) -> Rgb {
        self.colour
    }

    /// What the hex field shows, as typed.
    pub(crate) fn hex(&self) -> &str {
        &self.hex
    }

    pub(crate) fn hex_is_valid(&self) -> bool {
        parse_hex(&self.hex).is_some()
    }

    /// A swatch or a point in the hue/lightness area was chosen.
    pub(crate) fn pick(&mut self, colour: Rgb) {
        self.colour = colour;
        self.hex = format_hex(colour);
    }

    /// Hex digits are typed into the field (at most six); the colour follows whenever the
    /// field holds a complete value.
    pub(crate) fn key(&mut self, key: ColorKey) -> Outcome {
        match key {
            ColorKey::Return => return self.done(),
            ColorKey::Escape => return Outcome::Close(ControlAnswer::Dismiss),
            ColorKey::Backspace => {
                if self.hex.len() > 1 {
                    self.hex.pop();
                }
            }
            ColorKey::Character(character) => {
                if character.is_ascii_hexdigit() && self.hex.len() < 7 {
                    self.hex.push(character.to_ascii_lowercase());
                }
            }
        }
        if self.hex.len() == 7
            && let Some(colour) = parse_hex(&self.hex)
        {
            self.colour = colour;
        }
        Outcome::Stay
    }

    pub(crate) fn done(&self) -> Outcome {
        Outcome::Close(ControlAnswer::Color(self.colour))
    }
}

/// `#rrggbb`, `rrggbb`, `#rgb` or `rgb`, in either case.
pub(crate) fn parse_hex(text: &str) -> Option<Rgb> {
    let digits = text.trim();
    let digits = digits.strip_prefix('#').unwrap_or(digits);
    if !digits
        .chars()
        .all(|character| character.is_ascii_hexdigit())
    {
        return None;
    }
    let channel = |range: std::ops::Range<usize>| u8::from_str_radix(&digits[range], 16).ok();
    match digits.len() {
        6 => Some(rgb(channel(0..2)?, channel(2..4)?, channel(4..6)?)),
        3 => Some(rgb(
            channel(0..1)? * 17,
            channel(1..2)? * 17,
            channel(2..3)? * 17,
        )),
        _ => None,
    }
}

pub(crate) fn format_hex(colour: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", colour.red, colour.green, colour.blue)
}

/// The colour at a point of the hue/lightness area: hue and lightness from 0 to 1, full
/// saturation.
pub(crate) fn from_hue_lightness(hue: f32, lightness: f32) -> Rgb {
    let hue = hue.clamp(0.0, 1.0) * 6.0;
    let lightness = lightness.clamp(0.0, 1.0);
    let chroma = 1.0 - (2.0 * lightness - 1.0).abs();
    let second = chroma * (1.0 - (hue % 2.0 - 1.0).abs());
    let (red, green, blue) = match hue as u32 {
        0 => (chroma, second, 0.0),
        1 => (second, chroma, 0.0),
        2 => (0.0, chroma, second),
        3 => (0.0, second, chroma),
        4 => (second, 0.0, chroma),
        _ => (chroma, 0.0, second),
    };
    let offset = lightness - chroma / 2.0;
    let channel = |value: f32| ((value + offset) * 255.0).round().clamp(0.0, 255.0) as u8;
    rgb(channel(red), channel(green), channel(blue))
}

/// Where a colour sits in the hue/lightness area (its saturation is not shown there).
pub(crate) fn hue_lightness(colour: Rgb) -> (f32, f32) {
    let [red, green, blue] =
        [colour.red, colour.green, colour.blue].map(|channel| f32::from(channel) / 255.0);
    let max = red.max(green).max(blue);
    let min = red.min(green).min(blue);
    let lightness = (max + min) / 2.0;
    let delta = max - min;
    if delta == 0.0 {
        return (0.0, lightness);
    }
    let sector = if max == red {
        ((green - blue) / delta).rem_euclid(6.0)
    } else if max == green {
        (blue - red) / delta + 2.0
    } else {
        (red - green) / delta + 4.0
    };
    (sector / 6.0, lightness)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_text_parses_in_long_and_short_forms() {
        assert_eq!(parse_hex("#1E88e5"), Some(rgb(0x1e, 0x88, 0xe5)));
        assert_eq!(parse_hex("1e88e5"), Some(rgb(0x1e, 0x88, 0xe5)));
        assert_eq!(parse_hex("#f80"), Some(rgb(0xff, 0x88, 0x00)));
        assert_eq!(parse_hex(" #000000 "), Some(rgb(0, 0, 0)));
        assert_eq!(parse_hex("#12345"), None);
        assert_eq!(parse_hex("#gggggg"), None);
        assert_eq!(parse_hex(""), None);
        assert_eq!(parse_hex("#é12345"), None);
    }

    #[test]
    fn colours_format_as_lowercase_hex() {
        assert_eq!(format_hex(rgb(0x1e, 0x88, 0xe5)), "#1e88e5");
        assert_eq!(format_hex(rgb(0, 0, 0)), "#000000");
    }

    #[test]
    fn the_area_runs_through_the_hues_at_mid_lightness() {
        assert_eq!(from_hue_lightness(0.0, 0.5), rgb(255, 0, 0));
        assert_eq!(from_hue_lightness(1.0 / 3.0, 0.5), rgb(0, 255, 0));
        assert_eq!(from_hue_lightness(2.0 / 3.0, 0.5), rgb(0, 0, 255));
        assert_eq!(from_hue_lightness(0.25, 1.0), rgb(255, 255, 255));
        assert_eq!(from_hue_lightness(0.75, 0.0), rgb(0, 0, 0));
        assert_eq!(from_hue_lightness(1.5, -1.0), rgb(0, 0, 0));
    }

    #[test]
    fn a_colour_maps_back_to_its_point_in_the_area() {
        let (hue, lightness) = hue_lightness(rgb(0, 0, 255));
        assert!((hue - 2.0 / 3.0).abs() < 0.01, "{hue}");
        assert!((lightness - 0.5).abs() < 0.01, "{lightness}");
        assert_eq!(hue_lightness(rgb(255, 255, 255)), (0.0, 1.0));
        let (hue, _) = hue_lightness(rgb(255, 0, 128));
        assert!(hue > 0.9, "{hue}");
    }

    #[test]
    fn the_picker_starts_from_the_inputs_colour_or_black() {
        let model = ColorModel::new(Some(rgb(0x43, 0xa0, 0x47)));
        assert_eq!(model.hex(), "#43a047");
        assert!(model.hex_is_valid());
        assert_eq!(ColorModel::new(None).colour(), rgb(0, 0, 0));
    }

    #[test]
    fn typing_a_hex_value_changes_the_colour_once_it_is_complete() {
        let mut model = ColorModel::new(Some(rgb(0, 0, 0)));
        for _ in 0..7 {
            model.key(ColorKey::Backspace);
        }
        assert_eq!(model.hex(), "#");
        for character in "1e88e".chars() {
            assert_eq!(model.key(ColorKey::Character(character)), Outcome::Stay);
        }
        assert!(!model.hex_is_valid());
        assert_eq!(model.colour(), rgb(0, 0, 0));
        model.key(ColorKey::Character('5'));
        assert_eq!(model.colour(), rgb(0x1e, 0x88, 0xe5));
        model.key(ColorKey::Character('7'));
        assert_eq!(model.hex(), "#1e88e5", "the field holds at most six digits");
    }

    #[test]
    fn only_hex_digits_are_typed() {
        let mut model = ColorModel::new(Some(rgb(0, 0, 0)));
        model.key(ColorKey::Backspace);
        model.key(ColorKey::Character('x'));
        model.key(ColorKey::Character('#'));
        model.key(ColorKey::Character('A'));

        assert_eq!(model.hex(), "#00000a");
    }

    #[test]
    fn return_sets_the_colour_and_escape_cancels() {
        let mut model = ColorModel::new(Some(rgb(1, 2, 3)));
        model.pick(SWATCHES[4]);

        assert_eq!(model.hex(), "#e53935");
        assert_eq!(
            model.key(ColorKey::Return),
            Outcome::Close(ControlAnswer::Color(SWATCHES[4]))
        );
        assert_eq!(
            model.key(ColorKey::Escape),
            Outcome::Close(ControlAnswer::Dismiss)
        );
        assert_eq!(
            model.done(),
            Outcome::Close(ControlAnswer::Color(SWATCHES[4]))
        );
    }

    #[test]
    fn return_with_an_incomplete_hex_value_keeps_the_last_valid_colour() {
        let mut model = ColorModel::new(Some(rgb(1, 2, 3)));
        model.key(ColorKey::Backspace);

        assert_eq!(
            model.key(ColorKey::Return),
            Outcome::Close(ControlAnswer::Color(rgb(1, 2, 3)))
        );
    }
}
