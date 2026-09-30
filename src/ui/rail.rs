// SPDX-License-Identifier: Apache-2.0

//! The progress rail: a firefly's path through the dew.
//!
//! Ahead lies a dotted path of dew; behind, a beaded trail in the meadow's
//! pastels; at the play position, a small light like the meadow's own. The
//! light is placed below a single dot and its glow spreads over the dots
//! around it, so it glides along the rail instead of stepping between cells.

use ratatui::style::Style;
use ratatui::text::Span;

use super::palette::{self, ColorDepth, FIREFLY, TRACK};
use crate::app::PlaybackStatus;

/// The widest rail; a timeline centers anything narrower.
pub(super) const MAX_RAIL_CELLS: usize = 30;
const DOT_COLUMNS: usize = MAX_RAIL_CELLS * 2;
const DOT_ROWS: usize = 4;
/// The rail runs along the third dot row, just below the middle of the line.
const PATH_ROW: usize = 2;

const TRAIL_LIGHT: f32 = 0.72;
const DEW_LIGHT: f32 = 0.24;
/// Dots dimmer than this stay dark.
const VISIBLE: f32 = 0.12;
const CORE_RADIUS: f32 = 0.8;
const GLOW_RADIUS: f32 = 2.4;
const SPARKLE_LIGHT: f32 = 0.55;
/// The playing light breathes slowly; a paused one blinks like a firefly.
const BREATH_HZ: f32 = 0.6;
const BLINK_HZ: f32 = 0.35;
const HEAD_WHITENING: f32 = 0.35;
const WHITE: [u8; 3] = [255, 255, 245];

#[derive(Clone, Copy, Default)]
struct Dot {
    light: f32,
    color: [u8; 3],
}

impl Dot {
    fn light_up(&mut self, light: f32, color: [u8; 3]) {
        if light > self.light {
            *self = Self { light, color };
        }
    }
}

/// Renders `cells` of rail as colored braille. `progress` places the light,
/// which shows while a track plays or is paused; `seconds` animates it.
pub(super) fn spans(
    cells: usize,
    progress: Option<f32>,
    status: PlaybackStatus,
    seconds: f32,
    colors: Option<ColorDepth>,
) -> Vec<Span<'static>> {
    let cells = cells.min(MAX_RAIL_CELLS);
    let width = cells * 2;
    let mut dots = [[Dot::default(); DOT_ROWS]; DOT_COLUMNS];
    let head = progress.map(|progress| progress.clamp(0.0, 1.0) * dots_f32(width));

    for (x, column) in dots.iter_mut().enumerate().take(width) {
        let center = dots_f32(x) + 0.5;
        if head.is_some_and(|head| center <= head) {
            let place = center / dots_f32(width);
            column[PATH_ROW].light_up(TRAIL_LIGHT, palette::pastel(place));
        } else if x % 2 == 0 {
            column[PATH_ROW].light_up(DEW_LIGHT, TRACK);
        }
    }

    if let Some(head) =
        head.filter(|_| matches!(status, PlaybackStatus::Playing | PlaybackStatus::Paused))
    {
        let (color, glow) = light_mood(status, head / dots_f32(width.max(1)), seconds);
        let nearest = (0..width).min_by(|left, right| {
            let distance = |x: &usize| (dots_f32(*x) + 0.5 - head).abs();
            distance(left).total_cmp(&distance(right))
        });
        for (x, column) in dots.iter_mut().enumerate().take(width) {
            let distance = (dots_f32(x) + 0.5 - head).abs();
            let light = if distance <= CORE_RADIUS {
                glow
            } else if distance <= GLOW_RADIUS {
                let falloff = 1.0 - (distance - CORE_RADIUS) / (GLOW_RADIUS - CORE_RADIUS);
                0.5 * glow * falloff * falloff
            } else {
                continue;
            };
            column[PATH_ROW - 1].light_up(light, color);
            column[PATH_ROW].light_up(light, color);
            if nearest == Some(x) {
                column[PATH_ROW - 2].light_up(SPARKLE_LIGHT * glow, color);
                column[PATH_ROW + 1].light_up(SPARKLE_LIGHT * glow, color);
            }
        }
    }

    (0..cells)
        .map(|cell| {
            let mut bits = 0_u8;
            let mut brightest = Dot::default();
            for (column, row, bit) in BRAILLE_DOTS {
                let dot = dots[cell * 2 + column][row];
                if dot.light >= VISIBLE {
                    bits |= bit;
                    if dot.light > brightest.light {
                        brightest = dot;
                    }
                }
            }
            let glyph = char::from_u32(0x2800 + u32::from(bits)).unwrap_or(' ');
            let style = colors.map_or_else(Style::default, |depth| {
                Style::default().fg(depth.color(palette::shade(brightest.color, brightest.light)))
            });
            Span::styled(glyph.to_string(), style)
        })
        .collect()
}

/// The light's color and brightness: its place on the gradient while playing,
/// breathing gently; firefly yellow while paused, with a slow blink. Either
/// way it stays brighter than the trail behind it.
fn light_mood(status: PlaybackStatus, place: f32, seconds: f32) -> ([u8; 3], f32) {
    let wave = |hertz: f32| 0.5 + 0.5 * (std::f32::consts::TAU * hertz * seconds).sin();
    if status == PlaybackStatus::Paused {
        let blink = wave(BLINK_HZ);
        (FIREFLY, (0.3 * blink * blink).mul_add(blink, 0.7))
    } else {
        let color = palette::mix(palette::pastel(place), WHITE, HEAD_WHITENING);
        (color, 0.15f32.mul_add(wave(BREATH_HZ), 0.85))
    }
}

/// The eight dots of a braille cell: column, row, and code-point bit.
const BRAILLE_DOTS: [(usize, usize, u8); 8] = [
    (0, 0, 0x01),
    (0, 1, 0x02),
    (0, 2, 0x04),
    (1, 0, 0x08),
    (1, 1, 0x10),
    (1, 2, 0x20),
    (0, 3, 0x40),
    (1, 3, 0x80),
];

fn dots_f32(value: usize) -> f32 {
    u16::try_from(value).map_or(f32::from(u16::MAX), f32::from)
}

#[cfg(test)]
mod tests {
    use ratatui::style::Color;
    use ratatui::text::Span;

    use super::{MAX_RAIL_CELLS, spans};
    use crate::app::PlaybackStatus;
    use crate::ui::ColorDepth;

    fn glyphs(rail: &[Span<'_>]) -> String {
        rail.iter().map(|span| span.content.as_ref()).collect()
    }

    /// The cell holding the light's sparkle, the only dots above the path.
    fn light_cell(rail: &[Span<'_>]) -> usize {
        rail.iter()
            .position(|span| {
                span.content
                    .chars()
                    .any(|glyph| (u32::from(glyph) - 0x2800) & (0x01 | 0x08) != 0)
            })
            .expect("the rail shows its light")
    }

    #[test]
    fn an_idle_rail_is_a_quiet_path_of_dew() {
        let rail = spans(10, None, PlaybackStatus::Stopped, 0.0, None);

        assert_eq!(glyphs(&rail), "⠄".repeat(10));
        assert!(rail.iter().all(|span| span.style.fg.is_none()));
    }

    #[test]
    fn the_played_trail_is_pastel_beads_and_the_light_leads_it() {
        let rail = spans(
            20,
            Some(0.5),
            PlaybackStatus::Playing,
            0.0,
            Some(ColorDepth::TrueColor),
        );
        let light = light_cell(&rail);

        assert!((9..=10).contains(&light), "{}", glyphs(&rail));
        assert!(rail[..light - 1].iter().all(|span| span.content == "⠤"));
        assert!(rail[light + 2..].iter().all(|span| span.content == "⠄"));
        assert_ne!(
            rail[0].style.fg,
            rail[light - 2].style.fg,
            "the trail follows the gradient"
        );
        assert!(
            rail.iter()
                .all(|span| matches!(span.style.fg, Some(Color::Rgb(..))))
        );
    }

    #[test]
    fn the_light_glides_within_a_cell_instead_of_stepping() {
        // A quarter of a dot apart: the same cells, but the glow has moved.
        let at = |progress: f32| {
            spans(
                20,
                Some(progress),
                PlaybackStatus::Playing,
                0.0,
                Some(ColorDepth::TrueColor),
            )
        };
        // 20.6 and 20.85 dots along a 40-dot rail: inside one dot column.
        let before = at(20.6 / 40.0);
        let after = at(20.85 / 40.0);

        assert_eq!(light_cell(&before), light_cell(&after));
        assert_ne!(
            before.iter().map(|span| span.style.fg).collect::<Vec<_>>(),
            after.iter().map(|span| span.style.fg).collect::<Vec<_>>(),
            "the glow shifts with sub-dot progress"
        );
    }

    #[test]
    fn a_paused_light_turns_into_a_firefly_and_a_stopped_rail_has_none() {
        let paused = spans(
            20,
            Some(0.3),
            PlaybackStatus::Paused,
            0.0,
            Some(ColorDepth::TrueColor),
        );
        let Some(Color::Rgb(red, green, blue)) = paused[light_cell(&paused)].style.fg else {
            panic!("the paused light is colored");
        };
        assert!(red > blue && green > blue, "warm firefly yellow");

        let stopped = spans(20, Some(0.3), PlaybackStatus::Stopped, 0.0, None);
        assert!(
            glyphs(&stopped)
                .chars()
                .all(|glyph| matches!(glyph, '⠤' | '⠄'))
        );
    }

    #[test]
    fn the_rail_never_exceeds_its_widest_size() {
        let rail = spans(500, Some(1.0), PlaybackStatus::Playing, 0.0, None);
        assert_eq!(rail.len(), MAX_RAIL_CELLS);
    }
}
