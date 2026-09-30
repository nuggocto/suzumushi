// SPDX-License-Identifier: Apache-2.0

//! The Player's pastel night palette and its terminal color conversion.

use ratatui::style::Color;

/// Bass to treble, and start to end of a track: peach, rose, lilac, powder
/// blue, mint. Warm lows cool into airy highs, beside Suzu's green.
const PASTEL: [(f32, [u8; 3]); 5] = [
    (0.0, [0xFF, 0xBF, 0xA3]),
    (0.25, [0xF5, 0xA9, 0xC6]),
    (0.5, [0xCD, 0xB4, 0xF6]),
    (0.75, [0xA9, 0xCC, 0xF4]),
    (1.0, [0xA6, 0xE9, 0xD2]),
];

/// A light that has lifted off as a firefly.
pub(super) const FIREFLY: [u8; 3] = [0xFF, 0xF1, 0xA8];
/// The crescent moon.
pub(super) const MOON: [u8; 3] = [0xFF, 0xF3, 0xD6];
/// Faint stars.
pub(super) const STAR: [u8; 3] = [0xD9, 0xD6, 0xF2];
/// The unplayed progress rail, close to a dark theme's dim grey.
pub(super) const TRACK: [u8; 3] = [0x5A, 0x60, 0x66];
const HOT_CORE: [f32; 3] = [255.0, 255.0, 240.0];

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

    pub(super) const fn color(self, rgb: [u8; 3]) -> Color {
        match self {
            Self::TrueColor => Color::Rgb(rgb[0], rgb[1], rgb[2]),
            Self::Indexed => Color::Indexed(xterm_index(rgb)),
        }
    }
}

/// Dims a color with its light, and whitens the hottest cores.
pub(super) fn shade(color: [u8; 3], light: f32) -> [u8; 3] {
    let brightness = 0.18 + 0.82 * light.clamp(0.0, 1.0);
    let heat = ((light - 0.9) * 3.0).clamp(0.0, 0.4);
    std::array::from_fn(|channel| {
        let base = f32::from(color[channel]) * brightness;
        to_channel(base + (HOT_CORE[channel] - base) * heat)
    })
}

/// The pastel gradient at `position`, zero to one.
pub(super) fn pastel(position: f32) -> [u8; 3] {
    let position = position.clamp(0.0, 1.0);
    let upper = PASTEL
        .iter()
        .position(|(stop, _)| *stop >= position)
        .unwrap_or(PASTEL.len() - 1)
        .max(1);
    let (from_stop, from) = PASTEL[upper - 1];
    let (to_stop, to) = PASTEL[upper];
    mix(from, to, (position - from_stop) / (to_stop - from_stop))
}

pub(super) fn mix(from: [u8; 3], to: [u8; 3], amount: f32) -> [u8; 3] {
    let amount = amount.clamp(0.0, 1.0);
    std::array::from_fn(|channel| {
        let from = f32::from(from[channel]);
        to_channel(from + (f32::from(to[channel]) - from) * amount)
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
    use super::{ColorDepth, PASTEL, pastel};
    use ratatui::style::Color;

    #[test]
    fn the_gradient_meets_every_stop_and_stays_between_them() {
        for (stop, color) in PASTEL {
            assert_eq!(pastel(stop), color);
        }
        assert_eq!(pastel(-1.0), PASTEL[0].1);
        assert_eq!(pastel(2.0), PASTEL[PASTEL.len() - 1].1);
    }

    #[test]
    fn indexed_depth_picks_the_nearest_cube_color() {
        assert_eq!(ColorDepth::Indexed.color([0, 0, 0]), Color::Indexed(16));
        assert_eq!(
            ColorDepth::Indexed.color([255, 255, 255]),
            Color::Indexed(231)
        );
        // 0xFF, 0xBF, 0xA3 is closest to levels 255, 175, 175.
        assert_eq!(ColorDepth::Indexed.color(PASTEL[0].1), Color::Indexed(217));
        assert_eq!(ColorDepth::TrueColor.color([1, 2, 3]), Color::Rgb(1, 2, 3));
    }
}
