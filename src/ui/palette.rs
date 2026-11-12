// SPDX-License-Identifier: Apache-2.0

//! The Player's colors for dark and light terminals, and their terminal color
//! conversion.
//!
//! Every light has a brightness from zero to one. Brightness means distance
//! from the background: on a dark terminal a dim light fades toward black, and
//! on a light terminal it fades into the page. Each background has its own set
//! of hues, pastel by night and deepened by day, so the brightest lights keep
//! the most contrast on either.

use ratatui::style::Color;

/// The hues the Player draws with on one kind of background.
#[derive(Debug, PartialEq)]
struct Tones {
    /// Bass to treble, and start to end of a track: peach, rose, lilac, powder
    /// blue, mint. Warm lows cool into airy highs, beside Suzu's green.
    gradient: [(f32, [u8; 3]); 5],
    /// A light that has lifted off as a firefly.
    firefly: [u8; 3],
    /// The crescent moon.
    moon: [u8; 3],
    /// Faint stars.
    star: [u8; 3],
    /// The unplayed dew on the progress rail, drawn dim.
    dew: [u8; 3],
    /// The brightest lights burn toward this white core, by night only.
    hot_core: Option<[f32; 3]>,
    /// The rail's playing light is whitened toward this, by night only.
    head_tint: Option<[u8; 3]>,
}

static NIGHT: Tones = Tones {
    gradient: [
        (0.0, [0xFF, 0xBF, 0xA3]),
        (0.25, [0xF5, 0xA9, 0xC6]),
        (0.5, [0xCD, 0xB4, 0xF6]),
        (0.75, [0xA9, 0xCC, 0xF4]),
        (1.0, [0xA6, 0xE9, 0xD2]),
    ],
    firefly: [0xFF, 0xF1, 0xA8],
    moon: [0xFF, 0xF3, 0xD6],
    star: [0xD9, 0xD6, 0xF2],
    dew: [0xB8, 0xBE, 0xC6],
    hot_core: Some([255.0, 255.0, 240.0]),
    head_tint: Some([255, 255, 245]),
};

/// The night hues deepened until each reads as text on white or cream: every
/// point of the gradient keeps at least 4.5:1, and the moon at least 3.7:1.
static DAY: Tones = Tones {
    gradient: [
        (0.0, [0xB8, 0x4A, 0x26]),
        (0.25, [0xC0, 0x30, 0x6E]),
        (0.5, [0x7B, 0x4F, 0xD0]),
        (0.75, [0x2F, 0x6F, 0xC0]),
        (1.0, [0x12, 0x80, 0x6A]),
    ],
    // Dark enough that a paused light outshines the trail even at the
    // bottom of its blink.
    firefly: [0x7A, 0x40, 0x00],
    moon: [0xA8, 0x74, 0x1A],
    star: [0x6E, 0x70, 0x90],
    dew: [0x3A, 0x40, 0x46],
    hot_core: None,
    head_tint: None,
};

const BLACK: [u8; 3] = [0, 0, 0];
const HEAD_WHITENING: f32 = 0.35;

/// How the terminal can show the Player's colors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ColorDepth {
    /// 24-bit color: the full gradient and glow falloff.
    TrueColor,
    /// The xterm 256-color cube, nearest match.
    Indexed,
}

impl ColorDepth {
    /// Reads the conventional `COLORTERM` capability advertisement.
    #[must_use]
    pub fn from_environment() -> Self {
        match std::env::var_os("COLORTERM") {
            Some(value) if value == "truecolor" || value == "24bit" => Self::TrueColor,
            _ => Self::Indexed,
        }
    }

    const fn color(self, rgb: [u8; 3]) -> Color {
        match self {
            Self::TrueColor => Color::Rgb(rgb[0], rgb[1], rgb[2]),
            Self::Indexed => Color::Indexed(xterm_index(rgb)),
        }
    }
}

/// The Player's colors for one terminal: its color depth and the background
/// the lights fade into.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    depth: ColorDepth,
    tones: &'static Tones,
    /// What a light fades toward as it dims: black by night, the terminal's
    /// own background by day.
    fade_to: [u8; 3],
}

impl Palette {
    /// Picks night or day colors for `background`, the terminal's background
    /// color when it is known. An unknown background is taken to be dark.
    #[must_use]
    pub fn new(depth: ColorDepth, background: Option<[u8; 3]>) -> Self {
        match background.filter(|rgb| is_light(*rgb)) {
            Some(page) => Self {
                depth,
                tones: &DAY,
                fade_to: page,
            },
            None => Self {
                depth,
                tones: &NIGHT,
                fade_to: BLACK,
            },
        }
    }

    /// Whether these are the colors for a light background.
    #[must_use]
    pub fn is_day(&self) -> bool {
        self.tones == &DAY
    }

    pub(super) const fn color(&self, rgb: [u8; 3]) -> Color {
        self.depth.color(rgb)
    }

    /// The gradient at `position`, zero to one.
    pub(super) fn gradient(&self, position: f32) -> [u8; 3] {
        let stops = &self.tones.gradient;
        let position = position.clamp(0.0, 1.0);
        let upper = stops
            .iter()
            .position(|(stop, _)| *stop >= position)
            .unwrap_or(stops.len() - 1)
            .max(1);
        let (from_stop, from) = stops[upper - 1];
        let (to_stop, to) = stops[upper];
        mix(from, to, (position - from_stop) / (to_stop - from_stop))
    }

    pub(super) const fn firefly(&self) -> [u8; 3] {
        self.tones.firefly
    }

    pub(super) const fn moon(&self) -> [u8; 3] {
        self.tones.moon
    }

    pub(super) const fn star(&self) -> [u8; 3] {
        self.tones.star
    }

    pub(super) const fn dew(&self) -> [u8; 3] {
        self.tones.dew
    }

    /// The rail's playing light: whitened by night so it outshines the trail;
    /// by day its full hue already does.
    pub(super) fn head(&self, color: [u8; 3]) -> [u8; 3] {
        self.tones
            .head_tint
            .map_or(color, |tint| mix(color, tint, HEAD_WHITENING))
    }

    /// Fades a color by its light toward the background, and by night whitens
    /// the hottest cores.
    pub(super) fn shade(&self, color: [u8; 3], light: f32) -> [u8; 3] {
        let brightness = 0.18 + 0.82 * light.clamp(0.0, 1.0);
        let lit = mix_unclamped(self.fade_to, color, brightness);
        match self.tones.hot_core {
            Some(core) => {
                let heat = ((light - 0.9) * 3.0).clamp(0.0, 0.4);
                std::array::from_fn(|channel| {
                    to_channel(lit[channel] + (core[channel] - lit[channel]) * heat)
                })
            }
            None => lit.map(to_channel),
        }
    }
}

/// Whether text in black reads better on `rgb` than text in white: its
/// relative luminance is above the point where both contrast equally.
fn is_light(rgb: [u8; 3]) -> bool {
    const EQUAL_CONTRAST: f32 = 0.179;
    relative_luminance(rgb) > EQUAL_CONTRAST
}

/// WCAG relative luminance, zero for black to one for white.
fn relative_luminance(rgb: [u8; 3]) -> f32 {
    let linear = rgb.map(|channel| {
        let value = f32::from(channel) / 255.0;
        if value <= 0.040_45 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    });
    0.0722f32.mul_add(linear[2], 0.2126f32.mul_add(linear[0], 0.7152 * linear[1]))
}

/// The WCAG contrast ratio between two colors, from 1 to 21.
#[cfg(test)]
pub(super) fn contrast(first: [u8; 3], second: [u8; 3]) -> f32 {
    let (first, second) = (relative_luminance(first), relative_luminance(second));
    (first.max(second) + 0.05) / (first.min(second) + 0.05)
}

pub(super) fn mix(from: [u8; 3], to: [u8; 3], amount: f32) -> [u8; 3] {
    mix_unclamped(from, to, amount.clamp(0.0, 1.0)).map(to_channel)
}

fn mix_unclamped(from: [u8; 3], to: [u8; 3], amount: f32) -> [f32; 3] {
    std::array::from_fn(|channel| {
        let from = f32::from(from[channel]);
        from + (f32::from(to[channel]) - from) * amount
    })
}

pub(super) fn to_channel(value: f32) -> u8 {
    // Clamped to the u8 range before conversion.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let channel = value.clamp(0.0, 255.0).round() as u8;
    channel
}

/// The nearest color in the xterm 6x6x6 cube.
const fn xterm_index(rgb: [u8; 3]) -> u8 {
    16 + 36 * cube_level(rgb[0]) + 6 * cube_level(rgb[1]) + cube_level(rgb[2])
}

/// The cube's levels are 0, 95, 135, 175, 215, and 255; each threshold below
/// is the midpoint between two neighbors.
const fn cube_level(value: u8) -> u8 {
    match value {
        0..=47 => 0,
        48..=114 => 1,
        115..=154 => 2,
        155..=194 => 3,
        195..=234 => 4,
        _ => 5,
    }
}

#[cfg(test)]
mod tests {
    use super::{ColorDepth, DAY, NIGHT, Palette, contrast};
    use ratatui::style::Color;

    const WHITE: [u8; 3] = [0xFF, 0xFF, 0xFF];
    const CREAM: [u8; 3] = [0xFD, 0xF6, 0xE3];
    const MOCHA: [u8; 3] = [0x1E, 0x1E, 0x2E];

    fn night() -> Palette {
        Palette::new(ColorDepth::TrueColor, None)
    }

    fn day(page: [u8; 3]) -> Palette {
        Palette::new(ColorDepth::TrueColor, Some(page))
    }

    #[test]
    fn the_gradient_meets_every_stop_and_stays_between_them() {
        for (palette, tones) in [(night(), &NIGHT), (day(WHITE), &DAY)] {
            for (stop, color) in tones.gradient {
                assert_eq!(palette.gradient(stop), color);
            }
            assert_eq!(palette.gradient(-1.0), tones.gradient[0].1);
            assert_eq!(palette.gradient(2.0), tones.gradient[4].1);
        }
    }

    #[test]
    fn indexed_depth_picks_the_nearest_cube_color() {
        assert_eq!(ColorDepth::Indexed.color([0, 0, 0]), Color::Indexed(16));
        assert_eq!(
            ColorDepth::Indexed.color([255, 255, 255]),
            Color::Indexed(231)
        );
        // 0xFF, 0xBF, 0xA3 is closest to levels 255, 175, 175.
        assert_eq!(
            ColorDepth::Indexed.color(NIGHT.gradient[0].1),
            Color::Indexed(217)
        );
        assert_eq!(ColorDepth::TrueColor.color([1, 2, 3]), Color::Rgb(1, 2, 3));
    }

    #[test]
    fn light_backgrounds_get_day_colors_and_dark_or_unknown_ones_night() {
        assert!(day(WHITE).is_day());
        assert!(day(CREAM).is_day());
        assert!(!day(MOCHA).is_day());
        assert!(!day([0, 0, 0]).is_day());
        assert!(!night().is_day());
    }

    #[test]
    fn night_shading_fades_toward_black_whatever_the_dark_background() {
        // A dark terminal keeps the look it had before backgrounds were read.
        for light in [0.0, 0.3, 0.72, 0.95, 1.1] {
            assert_eq!(
                day(MOCHA).shade(NIGHT.moon, light),
                night().shade(NIGHT.moon, light)
            );
        }
        assert_eq!(night().shade([200, 100, 50], 0.0), [36, 18, 9]);
    }

    #[test]
    fn by_day_the_brightest_lights_have_the_most_contrast() {
        for page in [WHITE, CREAM] {
            let palette = day(page);
            for color in [palette.moon(), palette.firefly(), palette.gradient(0.0)] {
                let core = contrast(palette.shade(color, 1.0), page);
                let glow = contrast(palette.shade(color, 0.3), page);
                let edge = contrast(palette.shade(color, 0.0), page);
                assert!(core > glow && glow > edge, "{color:?} on {page:?}");
                assert!(core >= 3.7, "{color:?} core on {page:?}: {core}");
                assert!(edge < 1.4, "{color:?} edge on {page:?}: {edge}");
            }
        }
    }

    #[test]
    fn day_gradient_is_readable_as_text_on_white_and_cream() {
        for page in [WHITE, CREAM] {
            let palette = day(page);
            for step in 0..=100_u8 {
                let color = palette.gradient(f32::from(step) / 100.0);
                let ratio = contrast(color, page);
                assert!(ratio >= 4.5, "{color:?} at {step}% on {page:?}: {ratio}");
            }
        }
    }
}
