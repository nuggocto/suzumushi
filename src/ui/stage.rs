// SPDX-License-Identifier: Apache-2.0

//! Night meadow: the spectrum as grass stalks tipped with light around Suzu.
//!
//! Each frequency band is a stalk whose light rides the music. On pause the
//! lights lift off as fireflies and drift around Suzu; on resume they fly home.
//! Above the title, faint stars twinkle and a crescent moon crosses the sky as
//! the track plays.
//! Everything renders into a fixed dot field, then into braille cells: solid
//! shapes stay solid and glow becomes an ordered stipple, so the bloom is drawn
//! with dots instead of a painted background that could clash with the theme.

use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use super::palette::{self, ColorDepth, FIREFLY, MOON, STAR};
use crate::app::{ColorMode, PlaybackStatus};
use crate::audio::SPECTRUM_BANDS;
use crate::config::{MEADOW_MAX_ROWS, STAGE_MAX_COLUMNS, STAGE_MAX_ROWS};

const DOTS_PER_COLUMN: usize = 2;
const DOTS_PER_ROW: usize = 4;
const FIELD_DOTS: usize = STAGE_MAX_COLUMNS * DOTS_PER_COLUMN * STAGE_MAX_ROWS * DOTS_PER_ROW;
/// The fixed dot field reserved once for the whole session.
pub(crate) const STAGE_FIELD_BYTES: usize = FIELD_DOTS * std::mem::size_of::<Dot>();

const FAST_FRAME: Duration = Duration::from_millis(33);
const DRIFT_FRAME: Duration = Duration::from_millis(50);
const IDLE_FRAME: Duration = Duration::from_millis(100);
/// Frames further apart than this (a hidden Player, a stalled terminal) resume
/// the physics without a jump.
const MAX_STEP_SECONDS: f32 = 0.1;

const STEM_RISE_RATE: f32 = 30.0;
const STEM_FALL_PER_SECOND: f32 = 1.3;
const GRAVITY: f32 = 6.5;
const KICK: f32 = 0.2;
const MAX_KICK: f32 = 2.0;
const MAX_BALL: f32 = 1.06;
const SETTLED: f32 = 0.002;

const LIFT_SECONDS: f32 = 1.6;
const COLOR_TURN_SECONDS: f32 = 0.9;
/// Each light takes this long to settle home, and each band starts a little
/// after the one below it, so the meadow refills in a wave from bass to treble.
const RETURN_SECONDS: f32 = 1.1;
const RETURN_STAGGER_SECONDS: f32 = 0.02;
/// Sideways swing of a homecoming light, as a share of the stage width.
const RETURN_ARC: f32 = 0.05;

/// Dots between the ground and the lowest light, and above the tallest one.
const MIN_STEM_DOTS: f32 = 2.0;
const TOP_MARGIN_DOTS: f32 = 3.0;
const BALL_LIFT_DOTS: f32 = 1.5;
/// Radii of a light's solid core, its motion trail, and a drop of dew, in dots.
const CORE: f32 = 1.15;
const TRAIL: f32 = 0.9;
const DEW: f32 = 0.5;

/// One star per this many sky cells, up to a quiet maximum. Every seventh
/// star is brighter, so the sky has depth rather than an even dusting.
const SKY_CELLS_PER_STAR: usize = 15;
const BRIGHT_STAR_EVERY: usize = 7;
const MAX_STARS: usize = 96;
/// The moon needs this much sky, in dots, to be drawn.
const MOON_MIN_SKY_DOTS: f32 = 12.0;
const MOON_GLOW_DOTS: f32 = 2.4;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Point {
    /// Left to right, 0 to 1 across the stage.
    x: f32,
    /// Bottom to top, 0 to 1 up the stage.
    y: f32,
}

#[derive(Clone, Copy, Debug, Default)]
struct BandMotion {
    stem: f32,
    ball: f32,
    velocity: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Flight {
    /// Lights ride their stalks.
    Attached,
    /// Lights drift as fireflies since this time.
    Drifting { since: f32 },
    /// Lights fly back to their stalks since this time, still drifting along
    /// the flight that began at `drifting_since` while home pulls them in.
    Returning { since: f32, drifting_since: f32 },
}

/// Animation state that persists between frames. It is owned by the terminal
/// loop and advanced by each Player render.
pub struct Stage {
    bands: [BandMotion; SPECTRUM_BANDS],
    flight: Flight,
    /// Where each light was when it last lifted off.
    flight_from: [Point; SPECTRUM_BANDS],
    /// Stalk heights when the lights started home; stalks grow to meet them.
    stalks_from: [f32; SPECTRUM_BANDS],
    last_seconds: Option<f32>,
    last_status: PlaybackStatus,
    depth: ColorDepth,
    field: Box<[Dot]>,
}

#[derive(Clone, Copy, Debug, Default)]
struct Dot {
    /// How strongly the dot is drawn; below one it is stippled.
    cover: f32,
    /// Brightness of the light at this dot.
    light: f32,
    color: [u8; 3],
}

/// Fixed geometry for one frame: sky, then title, then meadow, top to bottom.
#[derive(Clone, Copy)]
struct Field {
    width: usize,
    height: usize,
    /// Dots of open sky above the title.
    sky: usize,
    /// Dots of meadow at the bottom, where stalks grow.
    meadow: usize,
}

impl Field {
    fn ground(self) -> f32 {
        dots_f32(self.height) - 1.0
    }

    fn span(self) -> f32 {
        (dots_f32(self.meadow) - 1.0 - MIN_STEM_DOTS - TOP_MARGIN_DOTS - BALL_LIFT_DOTS).max(1.0)
    }

    fn band_x(self, band: usize) -> f32 {
        (dots_f32(band) + 0.5) / dots_f32(SPECTRUM_BANDS) * dots_f32(self.width)
    }

    /// The dot height of a stalk or light at `height`, zero to one.
    fn lift(self, height: f32) -> f32 {
        self.ground() - MIN_STEM_DOTS - height * self.span()
    }

    fn to_point(self, x: f32, y: f32) -> Point {
        Point {
            x: x / dots_f32(self.width),
            y: 1.0 - y / dots_f32(self.height),
        }
    }

    fn to_dots(self, point: Point) -> (f32, f32) {
        (
            point.x * dots_f32(self.width),
            (1.0 - point.y) * dots_f32(self.height),
        )
    }
}

impl Stage {
    #[must_use]
    pub fn new(depth: ColorDepth) -> Self {
        Self {
            bands: [BandMotion::default(); SPECTRUM_BANDS],
            flight: Flight::Attached,
            flight_from: [Point::default(); SPECTRUM_BANDS],
            stalks_from: [0.0; SPECTRUM_BANDS],
            last_seconds: None,
            last_status: PlaybackStatus::Stopped,
            depth,
            field: vec![Dot::default(); FIELD_DOTS].into_boxed_slice(),
        }
    }

    /// How soon the next frame is worth drawing: smooth while the meadow moves,
    /// relaxed while fireflies drift, and the ordinary tick once it is still.
    #[must_use]
    pub fn frame_interval(&self) -> Duration {
        match self.flight {
            Flight::Drifting { .. } => DRIFT_FRAME,
            Flight::Returning { .. } => FAST_FRAME,
            Flight::Attached
                if self.last_status == PlaybackStatus::Playing
                    || self
                        .bands
                        .iter()
                        .any(|band| band.stem > SETTLED || band.ball > SETTLED) =>
            {
                FAST_FRAME
            }
            Flight::Attached => IDLE_FRAME,
        }
    }

    /// Advances the meadow to `now` and draws it, with the title and Suzu in
    /// front. The meadow and title sit at the bottom of `area`; any room left
    /// above them becomes sky.
    pub(super) fn render(
        &mut self,
        buffer: &mut Buffer,
        area: Rect,
        frame: &StageFrame<'_>,
        now: Duration,
    ) {
        let columns = usize::from(area.width).min(STAGE_MAX_COLUMNS);
        let rows = usize::from(area.height).min(STAGE_MAX_ROWS);
        if columns == 0 || rows == 0 {
            return;
        }
        let title_rows = frame.title.len().min(rows.saturating_sub(1));
        let meadow_rows = (rows - title_rows).min(MEADOW_MAX_ROWS);
        let sky_rows = rows - title_rows - meadow_rows;
        let area = Rect::new(
            area.x + (area.width - to_u16(columns)) / 2,
            area.y + area.height - to_u16(rows),
            to_u16(columns),
            to_u16(rows),
        );
        let field = Field {
            width: columns * DOTS_PER_COLUMN,
            height: rows * DOTS_PER_ROW,
            sky: sky_rows * DOTS_PER_ROW,
            meadow: meadow_rows * DOTS_PER_ROW,
        };
        let seconds = now.as_secs_f32();
        self.advance(frame.status, frame.levels, field, seconds);
        self.paint(field, seconds, frame.progress);
        self.present(buffer, area, field, frame.color_mode);
        let title_area = Rect::new(
            area.x,
            area.y + to_u16(sky_rows),
            area.width,
            to_u16(title_rows),
        );
        draw_overlay(buffer, title_area, &frame.title[..title_rows], 1);
        let suzu_rows = to_u16(frame.suzu.len()).min(area.height);
        let suzu_area = Rect::new(
            area.x,
            area.y + area.height - suzu_rows,
            area.width,
            suzu_rows,
        );
        draw_overlay(buffer, suzu_area, frame.suzu, 0);
    }

    /// The colors this stage draws with, for the rest of the Player to match.
    pub(super) const fn depth(&self) -> ColorDepth {
        self.depth
    }

    fn advance(
        &mut self,
        status: PlaybackStatus,
        levels: [u8; SPECTRUM_BANDS],
        field: Field,
        seconds: f32,
    ) {
        let step = self
            .last_seconds
            .map_or(0.0, |last| (seconds - last).clamp(0.0, MAX_STEP_SECONDS));
        self.last_seconds = Some(seconds);
        let drifting = status == PlaybackStatus::Paused;
        match self.flight {
            Flight::Attached | Flight::Returning { .. } if drifting => {
                self.capture_flight(field, seconds);
                self.flight = Flight::Drifting { since: seconds };
            }
            Flight::Drifting { since } if !drifting => {
                // The flight goes on; only its destination changes, so a light
                // never stops to turn around.
                self.stalks_from = self.bands.map(|band| band.stem);
                self.flight = Flight::Returning {
                    since: seconds,
                    drifting_since: since,
                };
            }
            Flight::Returning { since, .. }
                if seconds - since
                    >= RETURN_SECONDS + RETURN_STAGGER_SECONDS * dots_f32(SPECTRUM_BANDS) =>
            {
                self.flight = Flight::Attached;
            }
            _ => {}
        }

        // Transitions above capture the frame as it was last shown; only then
        // does the meadow move on.
        let playing = status == PlaybackStatus::Playing;
        for (band, level) in self.bands.iter_mut().zip(levels) {
            let target = if playing {
                f32::from(level) / f32::from(u8::MAX)
            } else {
                0.0
            };
            band.step(target, step);
        }
        self.last_status = status;
    }

    fn capture_flight(&mut self, field: Field, seconds: f32) {
        self.flight_from = std::array::from_fn(|band| self.light_at(band, field, seconds).0);
    }

    /// Where a band's light is now, and its color and brightness.
    fn light_at(&self, band: usize, field: Field, seconds: f32) -> (Point, [u8; 3], f32) {
        let attached = self.attached_point(band, field);
        let hue = meadow_color(band);
        match self.flight {
            Flight::Attached => (attached, hue, 1.0),
            Flight::Drifting { since } => self.drift(band, seconds, since),
            Flight::Returning { drifting_since, .. } => {
                let (drifting, color, glow) = self.drift(band, seconds, drifting_since);
                let progress = self.homecoming(band, seconds);
                let swing = (std::f32::consts::PI * progress).sin()
                    * RETURN_ARC
                    * Firefly::for_band(band).arc;
                let path = lerp_point(drifting, attached, progress);
                (
                    Point {
                        x: (path.x + swing).clamp(0.0, 1.0),
                        y: path.y,
                    },
                    palette::mix(color, hue, progress),
                    glow + (1.0 - glow) * progress,
                )
            }
        }
    }

    /// A light's firefly flight since it lifted off at `since`.
    fn drift(&self, band: usize, seconds: f32, since: f32) -> (Point, [u8; 3], f32) {
        let elapsed = seconds - since;
        let firefly = Firefly::for_band(band);
        let home = firefly.home(self.flight_from[band], elapsed);
        let lift = ease_out_cubic(elapsed / LIFT_SECONDS);
        (
            lerp_point(self.flight_from[band], home, lift),
            palette::mix(
                meadow_color(band),
                FIREFLY,
                smoothstep(elapsed / COLOR_TURN_SECONDS),
            ),
            firefly.glow(elapsed),
        )
    }

    /// How far a returning light is on its way home, zero to one.
    fn homecoming(&self, band: usize, seconds: f32) -> f32 {
        match self.flight {
            Flight::Attached => 1.0,
            Flight::Drifting { .. } => 0.0,
            Flight::Returning { since, .. } => {
                let delay = RETURN_STAGGER_SECONDS * dots_f32(band);
                smootherstep((seconds - since - delay) / RETURN_SECONDS)
            }
        }
    }

    fn attached_point(&self, band: usize, field: Field) -> Point {
        let motion = self.bands[band];
        field.to_point(field.band_x(band), field.lift(motion.ball) - BALL_LIFT_DOTS)
    }

    fn paint(&mut self, field: Field, seconds: f32, progress: Option<f32>) {
        let used = field.width * field.height;
        self.field[..used].fill(Dot::default());
        self.stars(field, seconds);
        if dots_f32(field.sky) >= MOON_MIN_SKY_DOTS {
            self.moon(field, progress);
        }
        for band in 0..SPECTRUM_BANDS {
            let motion = self.bands[band];
            let color = meadow_color(band);
            let x = field.band_x(band);
            let (point, light_color, glow) = self.light_at(band, field, seconds);
            let (light_x, light_y) = field.to_dots(point);
            // An attached light always sits on its stalk, which runs into its
            // core. Flying lights leave the grass behind, and a returning light
            // is met by its stalk growing up to where it will land.
            let progress = self.homecoming(band, seconds);
            let top = match self.flight {
                Flight::Attached => light_y,
                Flight::Drifting { .. } => field.lift(motion.stem),
                Flight::Returning { .. } => {
                    let from = field.lift(self.stalks_from[band]);
                    let (_, landing) = field.to_dots(self.attached_point(band, field));
                    from + (landing - from) * progress
                }
            };
            self.stalk(field, x, top, color);

            // Quiet lights are dew on the grass; loud ones bloom. A flying
            // light keeps a steady glow that settles into the music's.
            let energy = 0.85 + (motion.ball.clamp(0.0, 1.0) - 0.85) * progress;
            let halo = (0.12 + 0.36 * energy) * glow;
            if self.flight == Flight::Attached && motion.velocity > 0.4 {
                self.trail(field, light_x, light_y, motion.velocity, light_color);
            }
            // A resting light is one dim drop of dew; a flying one keeps a
            // solid core and blinks through its brightness.
            if energy < 0.02 && self.flight == Flight::Attached {
                self.splat(field, (light_x, light_y), DEW, 0.45, 0.0, light_color);
            } else {
                self.splat(field, (light_x, light_y), CORE, glow, halo, light_color);
            }
        }
    }

    /// Faint stars above the meadow, each twinkling slowly at its own pace.
    /// Their places are fixed shares of the sky, so they keep the same
    /// constellation when the terminal is resized.
    fn stars(&mut self, field: Field, seconds: f32) {
        let height = field.height - field.meadow;
        let cells = field.width / DOTS_PER_COLUMN * (height / DOTS_PER_ROW);
        let count = (cells / SKY_CELLS_PER_STAR).min(MAX_STARS);
        for star in 0..count {
            let mut seed = u64::try_from(star).unwrap_or(0) ^ 0x5747_2D5E_ED00_0000;
            let mut next = || unit(splitmix(&mut seed));
            let x = next() * dots_f32(field.width);
            let y = next() * dots_f32(height);
            let rate = 0.06 + 0.16 * next();
            let phase = std::f32::consts::TAU * next();
            let pulse = 0.5 + 0.5 * (std::f32::consts::TAU * rate).mul_add(seconds, phase).sin();
            let peak = if star % BRIGHT_STAR_EVERY == 0 {
                0.62
            } else {
                0.34
            };
            if let (Some(x), Some(y)) = (index_checked(x, field.width), index_checked(y, height)) {
                self.merge(field, x, y, 1.0, (peak * pulse).mul_add(pulse, 0.1), STAR);
            }
        }
    }

    /// A crescent moon with a soft halo, crossing the sky as the track plays.
    fn moon(&mut self, field: Field, progress: Option<f32>) {
        let (x, y) = moon_center(progress, dots_f32(field.width), dots_f32(field.sky));
        let radius = (dots_f32(field.sky) * 0.22).clamp(2.2, 6.0);
        // A second disc bites the crescent out of the upper right.
        let (bite_x, bite_y) = (radius.mul_add(0.55, x), radius.mul_add(-0.3, y));
        let reach = radius + MOON_GLOW_DOTS;
        let mut row = (y - reach).floor().max(0.0);
        while row <= y + reach {
            let mut column = (x - reach).floor().max(0.0);
            while column <= x + reach {
                let (center_x, center_y) = (column + 0.5, row + 0.5);
                let distance = (center_x - x).hypot(center_y - y);
                let bitten = (center_x - bite_x).hypot(center_y - bite_y) < radius * 0.92;
                let (cover, light) = if distance <= radius && !bitten {
                    (1.0, 0.95)
                } else if distance > radius && distance <= reach {
                    let falloff = 1.0 - (distance - radius) / MOON_GLOW_DOTS;
                    let glow = 0.3 * falloff * falloff;
                    (glow, glow)
                } else {
                    (0.0, 0.0)
                };
                if cover > 0.0
                    && let (Some(dot_x), Some(dot_y)) = (
                        index_checked(column, field.width),
                        index_checked(row, field.sky),
                    )
                {
                    self.merge(field, dot_x, dot_y, cover, light, MOON);
                }
                column += 1.0;
            }
            row += 1.0;
        }
    }

    fn stalk(&mut self, field: Field, x: f32, top: f32, color: [u8; 3]) {
        let ground = field.ground();
        let length = (ground - top).max(1.0);
        let column = round_index(x, field.width);
        let mut y = ground;
        while y >= top.max(0.0) {
            // Stalks brighten toward the light they carry.
            let rise = (ground - y) / length;
            let light = 0.3 + 0.42 * rise;
            if let Some(row) = round_index_checked(y, field.height) {
                self.merge(field, column, row, 1.0, light, color);
            }
            y -= 1.0;
        }
    }

    fn trail(&mut self, field: Field, x: f32, y: f32, velocity: f32, color: [u8; 3]) {
        let length = (velocity * 2.2).min(5.0);
        let mut step = 1.0;
        while step <= length {
            let fade = 0.55 * (1.0 - step / (length + 1.0));
            self.splat(field, (x, y + step + 1.0), TRAIL, fade, 0.0, color);
            step += 1.0;
        }
    }

    /// Draws a round light: a solid core and a halo that falls off with distance.
    fn splat(
        &mut self,
        field: Field,
        center: (f32, f32),
        core_radius: f32,
        core: f32,
        halo: f32,
        color: [u8; 3],
    ) {
        const HALO_RADIUS: f32 = 3.0;
        let (x, y) = center;
        let reach = if halo > 0.0 { HALO_RADIUS } else { core_radius };
        let left = (x - reach).floor().max(0.0);
        let top = (y - reach).floor().max(0.0);
        let mut row = top;
        while row <= y + reach {
            let mut column = left;
            while column <= x + reach {
                let distance = (column + 0.5 - x).hypot(row + 0.5 - y);
                let (cover, light) = if distance <= core_radius {
                    (1.0, core * 1.1)
                } else if halo > 0.0 && distance <= HALO_RADIUS {
                    let falloff = 1.0 - (distance - core_radius) / (HALO_RADIUS - core_radius);
                    let glow = halo * falloff * falloff;
                    (glow, glow)
                } else {
                    (0.0, 0.0)
                };
                if cover > 0.0
                    && let (Some(dot_x), Some(dot_y)) = (
                        index_checked(column, field.width),
                        index_checked(row, field.height),
                    )
                {
                    self.merge(field, dot_x, dot_y, cover, light, color);
                }
                column += 1.0;
            }
            row += 1.0;
        }
    }

    fn merge(&mut self, field: Field, x: usize, y: usize, cover: f32, light: f32, color: [u8; 3]) {
        let dot = &mut self.field[y * field.width + x];
        dot.cover = dot.cover.max(cover);
        if light > dot.light {
            dot.light = light;
            dot.color = color;
        }
    }

    fn present(&self, buffer: &mut Buffer, area: Rect, field: Field, color_mode: ColorMode) {
        for row in 0..usize::from(area.height) {
            for column in 0..usize::from(area.width) {
                let mut bits = 0_u8;
                let mut brightest = Dot::default();
                for (dot_x, dot_y, bit) in BRAILLE_DOTS {
                    let x = column * DOTS_PER_COLUMN + dot_x;
                    let y = row * DOTS_PER_ROW + dot_y;
                    let dot = self.field[y * field.width + x];
                    if dot.cover >= 1.0 || dot.cover > dither_threshold(x, y) {
                        bits |= bit;
                        if dot.light > brightest.light {
                            brightest = dot;
                        }
                    }
                }
                if bits == 0 {
                    continue;
                }
                let glyph = cell_glyph(bits);
                let style = match color_mode {
                    ColorMode::Mono => Style::default(),
                    ColorMode::Terminal => Style::default().fg(self
                        .depth
                        .color(palette::shade(brightest.color, brightest.light))),
                };
                let x = area.x + u16::try_from(column).unwrap_or(u16::MAX);
                let y = area.y + u16::try_from(row).unwrap_or(u16::MAX);
                if let Some(cell) = buffer.cell_mut((x, y)) {
                    cell.set_char(glyph).set_style(style);
                }
            }
        }
    }
}

impl BandMotion {
    /// Stalks rise with the music and sink slowly; lights are thrown up by a
    /// rising stalk and fall back under gravity.
    fn step(&mut self, target: f32, seconds: f32) {
        let previous = self.stem;
        if target > self.stem {
            self.stem += (target - self.stem) * (1.0 - (-STEM_RISE_RATE * seconds).exp());
        } else {
            self.stem = (self.stem - STEM_FALL_PER_SECOND * seconds).max(target);
        }
        if seconds <= 0.0 {
            return;
        }
        if self.stem >= self.ball {
            let rise = ((self.stem - previous) / seconds).clamp(0.0, MAX_KICK);
            self.ball = self.stem;
            self.velocity = self.velocity.max(rise * KICK);
        } else {
            self.velocity -= GRAVITY * seconds;
            self.ball += self.velocity * seconds;
            if self.ball <= self.stem {
                self.ball = self.stem;
                self.velocity = 0.0;
            }
        }
        if self.ball > MAX_BALL {
            self.ball = MAX_BALL;
            self.velocity = self.velocity.min(0.0);
        }
    }
}

/// A light's deterministic flight plan while paused.
struct Firefly {
    home_x: f32,
    home_y: f32,
    wander: [f32; 3],
    speed: [f32; 3],
    phase: [f32; 4],
    blink: f32,
    /// Which way, and how far, the light swings on its way home.
    arc: f32,
}

impl Firefly {
    fn for_band(band: usize) -> Self {
        let mut seed = u64::try_from(band).unwrap_or(0).wrapping_add(0x5EED_5EED);
        let mut next = || unit(splitmix(&mut seed));
        Self {
            home_x: (next() - 0.5) * 0.12,
            home_y: 0.18 + 0.72 * next(),
            wander: [0.015 + 0.03 * next(), 0.03 + 0.05 * next(), 0.01 * next()],
            speed: [
                0.07 + 0.11 * next(),
                0.1 + 0.15 * next(),
                0.25 + 0.3 * next(),
            ],
            phase: std::array::from_fn(|_| std::f32::consts::TAU * next()),
            blink: 0.22 + 0.35 * next(),
            arc: 2.0f32.mul_add(next(), -1.0),
        }
    }

    fn home(&self, from: Point, elapsed: f32) -> Point {
        let tau = std::f32::consts::TAU;
        let x = (from.x + self.home_x).clamp(0.04, 0.96)
            + self.wander[0] * (tau * self.speed[0]).mul_add(elapsed, self.phase[0]).sin()
            + self.wander[2] * (tau * self.speed[2]).mul_add(elapsed, self.phase[2]).sin();
        let y = self.home_y
            + self.wander[1] * (tau * self.speed[1]).mul_add(elapsed, self.phase[1]).sin();
        Point {
            x: x.clamp(0.02, 0.98),
            y: y.clamp(0.08, 0.95),
        }
    }

    /// A soft glow with brief brighter pulses, like a firefly.
    fn glow(&self, elapsed: f32) -> f32 {
        let wave = 0.5
            + 0.5
                * (std::f32::consts::TAU * self.blink)
                    .mul_add(elapsed, self.phase[3])
                    .sin();
        0.7 + 0.45 * wave * wave * wave
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

/// A full dot column alone is a stalk: draw it as a solid hairline, which
/// braille would break into dots. Anything else is a braille pattern.
fn cell_glyph(bits: u8) -> char {
    const LEFT_COLUMN: u8 = 0x01 | 0x02 | 0x04 | 0x40;
    const RIGHT_COLUMN: u8 = 0x08 | 0x10 | 0x20 | 0x80;
    match bits {
        LEFT_COLUMN => '▏',
        RIGHT_COLUMN => '▕',
        _ => char::from_u32(0x2800 + u32::from(bits)).unwrap_or(' '),
    }
}

/// A 4x4 Bayer matrix spreads partial coverage into an even stipple.
fn dither_threshold(x: usize, y: usize) -> f32 {
    const BAYER: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
    (f32::from(BAYER[y % 4][x % 4]) + 0.5) / 16.0 * 0.74 + 0.06
}

/// Everything the stage needs from the rest of the Player for one frame.
pub(super) struct StageFrame<'a> {
    pub(super) status: PlaybackStatus,
    pub(super) levels: [u8; SPECTRUM_BANDS],
    /// How much of the track has played, when its length is known.
    pub(super) progress: Option<f32>,
    pub(super) color_mode: ColorMode,
    pub(super) title: &'a [Line<'static>],
    pub(super) suzu: &'a [Line<'static>],
}

/// Where the moon is: rising from the left as a track starts, highest halfway
/// through, setting on the right at its end, and resting high on the right
/// when no track length is known.
fn moon_center(progress: Option<f32>, width: f32, sky: f32) -> (f32, f32) {
    progress.map_or((0.84 * width, 0.35 * sky), |progress| {
        let progress = progress.clamp(0.0, 1.0);
        (
            progress.mul_add(0.76, 0.12) * width,
            (std::f32::consts::PI * progress).sin().mul_add(-0.45, 0.72) * sky,
        )
    })
}

/// Draws lines over the stage from the top of `area`. Each line is centered as
/// a paragraph line would be, and its silhouette, widened by `margin` cells on
/// each side, hides the stage behind it. Positions are display cells, so wide
/// and combining characters land exactly where a paragraph would put them.
fn draw_overlay(buffer: &mut Buffer, area: Rect, lines: &[Line<'static>], margin: u16) {
    let right = area.x + area.width;
    for (row, line) in (area.y..area.y + area.height).zip(lines) {
        let width = to_u16(line.width());
        // Match paragraph centering exactly, so the art keeps its alignment.
        let start = area.x + (area.width / 2).saturating_sub(width / 2);
        let mut offset = 0_u16;
        let mut inked: Option<(u16, u16)> = None;
        for grapheme in line.styled_graphemes(Style::default()) {
            let cells = to_u16(Span::raw(grapheme.symbol).width());
            if grapheme.symbol != " " {
                let first = inked.map_or(offset, |(first, _)| first);
                inked = Some((first, offset + cells));
            }
            offset = offset.saturating_add(cells);
        }
        let Some((first, end)) = inked else {
            continue;
        };
        let masked = start
            .saturating_add(first)
            .saturating_sub(margin)
            .max(area.x)
            ..start.saturating_add(end).saturating_add(margin).min(right);
        for x in masked {
            if let Some(cell) = buffer.cell_mut((x, row)) {
                cell.reset();
            }
        }
        let mut x = start;
        for grapheme in line.styled_graphemes(Style::default()) {
            let cells = to_u16(Span::raw(grapheme.symbol).width());
            if x.saturating_add(cells) > right {
                break;
            }
            if grapheme.symbol != " " {
                buffer.set_stringn(x, row, grapheme.symbol, usize::from(cells), grapheme.style);
            }
            x = x.saturating_add(cells);
        }
    }
}

fn meadow_color(band: usize) -> [u8; 3] {
    palette::pastel(dots_f32(band) / dots_f32(SPECTRUM_BANDS - 1))
}

fn lerp_point(from: Point, to: Point, amount: f32) -> Point {
    let amount = amount.clamp(0.0, 1.0);
    Point {
        x: from.x + (to.x - from.x) * amount,
        y: from.y + (to.y - from.y) * amount,
    }
}

fn smoothstep(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    value * value * 2.0f32.mul_add(-value, 3.0)
}

fn ease_out_cubic(value: f32) -> f32 {
    let rest = 1.0 - value.clamp(0.0, 1.0);
    1.0 - rest * rest * rest
}

/// Starts and ends with zero speed and zero acceleration, so a light eases
/// out of its drift and settles without a visible jolt.
fn smootherstep(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    value * value * value * value.mul_add(6.0f32.mul_add(value, -15.0), 10.0)
}

fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut value = *state;
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

/// A uniform value in `0..1` from the top 24 bits, exactly representable.
fn unit(value: u64) -> f32 {
    let bits = u32::try_from(value >> 40).unwrap_or(0);
    // Fewer than 24 significant bits convert to f32 exactly.
    #[allow(clippy::cast_precision_loss)]
    let unit = bits as f32 / 16_777_216.0;
    unit
}

/// Small counts and dot coordinates, far below f32's exact-integer range.
fn dots_f32(value: usize) -> f32 {
    u16::try_from(value).map_or(f32::from(u16::MAX), f32::from)
}

fn to_u16(value: usize) -> u16 {
    u16::try_from(value).unwrap_or(u16::MAX)
}

fn index_checked(value: f32, limit: usize) -> Option<usize> {
    if value < 0.0 || value >= dots_f32(limit) {
        return None;
    }
    // Bounded by `limit` above, so the conversion cannot truncate.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let index = value as usize;
    Some(index)
}

fn round_index_checked(value: f32, limit: usize) -> Option<usize> {
    index_checked(value.round(), limit)
}

fn round_index(value: f32, limit: usize) -> usize {
    round_index_checked(value, limit).unwrap_or_else(|| limit.saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::Color;
    use ratatui::text::{Line, Span};

    use super::{
        BandMotion, ColorDepth, DRIFT_FRAME, FAST_FRAME, Field, Flight, IDLE_FRAME, MAX_BALL,
        Stage, StageFrame, cell_glyph, moon_center,
    };
    use crate::app::{ColorMode, PlaybackStatus};
    use crate::audio::SPECTRUM_BANDS;

    const AREA: Rect = Rect::new(2, 1, 40, 12);

    fn frame<'a>(
        status: PlaybackStatus,
        level: u8,
        color_mode: ColorMode,
        suzu: &'a [Line<'static>],
    ) -> StageFrame<'a> {
        StageFrame {
            status,
            levels: [level; SPECTRUM_BANDS],
            progress: None,
            color_mode,
            title: &[],
            suzu,
        }
    }

    /// Plays `seconds` of frames at 30 fps from `start`, returning the end time.
    fn play(
        stage: &mut Stage,
        buffer: &mut Buffer,
        stage_frame: &StageFrame<'_>,
        start: f32,
        seconds: f32,
    ) -> f32 {
        let mut now = start;
        while now < start + seconds {
            stage.render(buffer, AREA, stage_frame, Duration::from_secs_f32(now));
            now += 1.0 / 30.0;
        }
        now
    }

    fn field() -> Field {
        Field {
            width: usize::from(AREA.width) * 2,
            height: usize::from(AREA.height) * 4,
            sky: 0,
            meadow: usize::from(AREA.height) * 4,
        }
    }

    #[test]
    fn a_light_is_thrown_by_its_stalk_and_falls_back_under_gravity() {
        let mut band = BandMotion::default();
        let mut highest_gap = 0.0_f32;
        for _ in 0..10 {
            band.step(0.8, 1.0 / 30.0);
            highest_gap = highest_gap.max(band.ball - band.stem);
        }
        assert!(band.stem > 0.75, "{band:?}");
        assert!(band.ball >= band.stem && band.ball <= MAX_BALL, "{band:?}");

        for _ in 0..30 {
            band.step(0.0, 1.0 / 30.0);
            assert!(band.ball >= band.stem, "a light never sinks into its stalk");
            highest_gap = highest_gap.max(band.ball - band.stem);
        }
        assert!(highest_gap > 0.0, "a sudden rise throws the light");
        for _ in 0..60 {
            band.step(0.0, 1.0 / 30.0);
        }
        assert!(band.ball < 0.01 && band.stem < 0.01, "{band:?}");
    }

    #[test]
    fn pausing_releases_fireflies_that_stay_on_stage_and_resuming_brings_them_home() {
        let mut stage = Stage::new(ColorDepth::TrueColor);
        let mut buffer = Buffer::empty(Rect::new(0, 0, 44, 14));
        let playing = frame(PlaybackStatus::Playing, 200, ColorMode::Terminal, &[]);
        let paused = frame(PlaybackStatus::Paused, 0, ColorMode::Terminal, &[]);
        let now = play(&mut stage, &mut buffer, &playing, 0.0, 1.0);
        let now = play(&mut stage, &mut buffer, &paused, now, 3.0);

        assert!(matches!(stage.flight, Flight::Drifting { .. }));
        let seconds = now - 1.0 / 30.0;
        let mut moved = 0;
        for band in 0..SPECTRUM_BANDS {
            let (point, _, glow) = stage.light_at(band, field(), seconds);
            assert!((0.0..=1.0).contains(&point.x) && (0.0..=1.0).contains(&point.y));
            assert!(glow > 0.0);
            if (point.y - stage.attached_point(band, field()).y).abs() > 0.1 {
                moved += 1;
            }
        }
        assert!(moved > SPECTRUM_BANDS / 2, "most lights leave the grass");

        let now = play(&mut stage, &mut buffer, &playing, now, 1.0);
        assert!(matches!(stage.flight, Flight::Returning { .. }));
        play(&mut stage, &mut buffer, &playing, now, 1.0);
        assert_eq!(stage.flight, Flight::Attached, "every light is home");
    }

    #[test]
    fn resuming_glides_every_light_home_without_a_jump() {
        let mut stage = Stage::new(ColorDepth::TrueColor);
        let mut buffer = Buffer::empty(Rect::new(0, 0, 44, 14));
        let playing = frame(PlaybackStatus::Playing, 170, ColorMode::Terminal, &[]);
        let paused = frame(PlaybackStatus::Paused, 0, ColorMode::Terminal, &[]);
        let now = play(&mut stage, &mut buffer, &playing, 0.0, 1.0);
        let mut now = play(&mut stage, &mut buffer, &paused, now, 3.0);

        let positions = |stage: &Stage, now: f32| -> Vec<super::Point> {
            (0..SPECTRUM_BANDS)
                .map(|band| stage.light_at(band, field(), now).0)
                .collect()
        };
        let mut previous = positions(&stage, now - 1.0 / 30.0);
        let mut largest_step = 0.0_f32;
        let mut returning_frames = 0;
        for frame_index in 0..90 {
            stage.render(&mut buffer, AREA, &playing, Duration::from_secs_f32(now));
            if frame_index == 3 {
                // The grass starts from its paused height: just after resuming,
                // only flying lights are up in the air, not stalks.
                let tall_stalks = (AREA.y..AREA.y + AREA.height / 2)
                    .flat_map(|y| (AREA.x..AREA.x + AREA.width).map(move |x| (x, y)))
                    .filter(|position| matches!(buffer[*position].symbol(), "▏" | "▕"))
                    .count();
                assert_eq!(tall_stalks, 0, "stalks grow with their returning lights");
            }
            let current = positions(&stage, now);
            for (before, after) in previous.iter().zip(&current) {
                largest_step = largest_step.max((after.x - before.x).hypot(after.y - before.y));
            }
            // Frames in which the first light is on its way, neither drifting
            // nor home: its own flight, apart from the wave's stagger.
            if (0.001..0.999).contains(&stage.homecoming(0, now)) {
                returning_frames += 1;
            }
            previous = current;
            now += 1.0 / 30.0;
        }

        assert_eq!(stage.flight, Flight::Attached);
        assert!(
            returning_frames >= 30,
            "each light glides home for over a second: {returning_frames} frames"
        );
        assert!(
            largest_step < 0.07,
            "no light jumps between frames: {largest_step}"
        );
    }

    #[test]
    fn identical_inputs_draw_identical_frames() {
        let draw = || {
            let mut stage = Stage::new(ColorDepth::TrueColor);
            let mut buffer = Buffer::empty(Rect::new(0, 0, 44, 14));
            let playing = frame(PlaybackStatus::Playing, 180, ColorMode::Terminal, &[]);
            let paused = frame(PlaybackStatus::Paused, 0, ColorMode::Terminal, &[]);
            let now = play(&mut stage, &mut buffer, &playing, 0.0, 0.5);
            play(&mut stage, &mut buffer, &paused, now, 1.5);
            buffer
        };

        assert_eq!(draw(), draw());
    }

    #[test]
    fn the_meadow_draws_only_inside_its_area() {
        for (width, height) in [(1, 1), (8, 3), (40, 12), (300, 40)] {
            let outer = Rect::new(0, 0, width + 4, height + 4);
            let mut buffer = Buffer::filled(outer, ratatui::buffer::Cell::new("x"));
            let area = Rect::new(2, 2, width, height);
            let mut stage = Stage::new(ColorDepth::TrueColor);
            let playing = frame(PlaybackStatus::Playing, 255, ColorMode::Terminal, &[]);
            for step in 0..20 {
                stage.render(
                    &mut buffer,
                    area,
                    &playing,
                    Duration::from_millis(step * 33),
                );
            }
            for y in 0..outer.height {
                for x in 0..outer.width {
                    if !area.contains((x, y).into()) {
                        assert_eq!(buffer[(x, y)].symbol(), "x", "{width}x{height} at {x},{y}");
                    }
                }
            }
        }
    }

    #[test]
    fn suzu_stays_readable_in_front_of_the_meadow() {
        let suzu = [Line::raw("  ▐▀^ ^▀▌  "), Line::raw("▄▄█  ▴  █▄▄")];
        let mut stage = Stage::new(ColorDepth::TrueColor);
        let mut buffer = Buffer::empty(Rect::new(0, 0, 44, 14));
        let loud = frame(PlaybackStatus::Playing, 255, ColorMode::Terminal, &suzu);
        play(&mut stage, &mut buffer, &loud, 0.0, 1.0);

        let row = |y: u16| -> String {
            (AREA.x..AREA.x + AREA.width)
                .map(|x| buffer[(x, y)].symbol().to_owned())
                .collect()
        };
        let bottom = AREA.y + AREA.height - 1;
        assert!(row(bottom - 1).contains("▐▀^ ^▀▌"), "{}", row(bottom - 1));
        assert!(row(bottom).contains("▄▄█  ▴  █▄▄"), "{}", row(bottom));
        assert!(
            row(bottom - 4).chars().any(|glyph| glyph != ' '),
            "loud stalks rise above Suzu"
        );
    }

    #[test]
    fn colors_follow_the_terminal_depth_and_mono_leaves_the_meadow_unstyled() {
        let cases = [
            (ColorDepth::TrueColor, ColorMode::Terminal),
            (ColorDepth::Indexed, ColorMode::Terminal),
            (ColorDepth::TrueColor, ColorMode::Mono),
        ];
        for (depth, color_mode) in cases {
            let mut stage = Stage::new(depth);
            let mut buffer = Buffer::empty(Rect::new(0, 0, 44, 14));
            let playing = frame(PlaybackStatus::Playing, 220, color_mode, &[]);
            play(&mut stage, &mut buffer, &playing, 0.0, 0.5);

            let colors: Vec<Color> = buffer
                .content()
                .iter()
                .filter(|cell| cell.symbol() != " ")
                .map(|cell| cell.fg)
                .collect();
            assert!(!colors.is_empty());
            for color in colors {
                let expected = match (depth, color_mode) {
                    (_, ColorMode::Mono) => matches!(color, Color::Reset),
                    (ColorDepth::TrueColor, _) => matches!(color, Color::Rgb(..)),
                    (ColorDepth::Indexed, _) => matches!(color, Color::Indexed(16..=231)),
                };
                assert!(expected, "{depth:?} {color_mode:?}: {color:?}");
            }
        }
    }

    #[test]
    fn frames_are_smooth_while_moving_and_relaxed_once_still() {
        let mut stage = Stage::new(ColorDepth::TrueColor);
        let mut buffer = Buffer::empty(Rect::new(0, 0, 44, 14));
        assert_eq!(stage.frame_interval(), IDLE_FRAME);

        let playing = frame(PlaybackStatus::Playing, 200, ColorMode::Terminal, &[]);
        let now = play(&mut stage, &mut buffer, &playing, 0.0, 0.5);
        assert_eq!(stage.frame_interval(), FAST_FRAME);

        let paused = frame(PlaybackStatus::Paused, 0, ColorMode::Terminal, &[]);
        let now = play(&mut stage, &mut buffer, &paused, now, 0.5);
        assert_eq!(stage.frame_interval(), DRIFT_FRAME);

        let stopped = frame(PlaybackStatus::Stopped, 0, ColorMode::Terminal, &[]);
        play(&mut stage, &mut buffer, &stopped, now, 3.0);
        assert_eq!(stage.frame_interval(), IDLE_FRAME, "a settled meadow idles");
    }

    #[test]
    fn a_bare_stalk_column_is_a_solid_hairline() {
        assert_eq!(cell_glyph(0x01 | 0x02 | 0x04 | 0x40), '▏');
        assert_eq!(cell_glyph(0x08 | 0x10 | 0x20 | 0x80), '▕');
        assert_eq!(cell_glyph(0x01), '⠁');
        assert_eq!(cell_glyph(0xFF), '⣿');
    }

    #[test]
    fn the_moon_rises_crosses_and_sets_with_the_track() {
        let (width, sky) = (100.0, 40.0);
        let (start_x, start_y) = moon_center(Some(0.0), width, sky);
        let (middle_x, middle_y) = moon_center(Some(0.5), width, sky);
        let (end_x, end_y) = moon_center(Some(1.0), width, sky);

        assert!(start_x < middle_x && middle_x < end_x, "left to right");
        assert!(middle_y < start_y && middle_y < end_y, "highest halfway");
        assert!(
            (start_y - end_y).abs() < 1e-3,
            "rises and sets at one height"
        );
        let (resting_x, resting_y) = moon_center(None, width, sky);
        assert!(resting_x > 0.8 * width && resting_y < 0.5 * sky);
    }

    /// Renders two seconds of a paused track into a tall stage with a title.
    fn tall_paused_stage(rows: u16) -> (Stage, Buffer, Rect) {
        let area = Rect::new(0, 0, 44, rows);
        let title = [Line::raw("Night Song")];
        let mut stage = Stage::new(ColorDepth::TrueColor);
        let mut buffer = Buffer::empty(area);
        let mut stage_frame = frame(PlaybackStatus::Playing, 160, ColorMode::Terminal, &[]);
        stage_frame.title = &title;
        stage_frame.progress = Some(0.3);
        let mut now = 0.0;
        for status in [PlaybackStatus::Playing, PlaybackStatus::Paused] {
            stage_frame.status = status;
            for _ in 0..60 {
                stage.render(
                    &mut buffer,
                    area,
                    &stage_frame,
                    Duration::from_secs_f32(now),
                );
                now += 1.0 / 30.0;
            }
        }
        (stage, buffer, area)
    }

    #[test]
    fn a_tall_player_opens_a_night_sky_above_the_title() {
        let (_, buffer, area) = tall_paused_stage(30);
        let row = |y: u16| -> String {
            (0..area.width)
                .map(|x| buffer[(x, y)].symbol().to_owned())
                .collect()
        };
        // Sixteen meadow rows at the bottom, the title above them, sky above.
        let title_row = area.height - 16 - 1;
        assert!(row(title_row).contains("Night Song"), "{}", row(title_row));
        let lit_sky = (0..title_row)
            .flat_map(|y| (0..area.width).map(move |x| (x, y)))
            .filter(|position| buffer[*position].symbol() != " ")
            .count();
        assert!(
            lit_sky > 20,
            "stars and the moon fill the sky: {lit_sky} cells"
        );

        let (_, short, short_area) = tall_paused_stage(12);
        assert!(
            (0..short_area.width)
                .map(|x| short[(x, 0)].symbol().to_owned())
                .collect::<String>()
                .contains("Night Song"),
            "without room for sky, the title sits at the top"
        );
    }

    #[test]
    fn paused_fireflies_rise_into_the_sky() {
        let (stage, _, area) = tall_paused_stage(30);
        let field = Field {
            width: usize::from(area.width) * 2,
            height: usize::from(area.height) * 4,
            sky: usize::from(area.height - 17) * 4,
            meadow: 16 * 4,
        };
        let meadow_top = 16.0 / f32::from(area.height);
        let above = (0..SPECTRUM_BANDS)
            .filter(|band| stage.light_at(*band, field, 4.0).0.y > meadow_top)
            .count();
        assert!(above > 4, "fireflies drift above the meadow: {above}");
    }

    #[test]
    fn wide_and_combining_titles_keep_every_character() {
        for title in ["東京夜", "Cafe\u{301} au lait", "🦗 night"] {
            let line = [Line::raw(title)];
            let area = Rect::new(0, 0, 44, 30);
            let mut stage = Stage::new(ColorDepth::TrueColor);
            let mut buffer = Buffer::empty(area);
            let mut stage_frame = frame(PlaybackStatus::Playing, 180, ColorMode::Terminal, &[]);
            stage_frame.title = &line;
            stage.render(&mut buffer, area, &stage_frame, Duration::ZERO);

            let title_row = area.height - 16 - 1;
            // Read the row as a terminal shows it: a wide symbol covers the
            // cell after it.
            let mut shown = String::new();
            let mut x = 0;
            while x < area.width {
                let symbol = buffer[(x, title_row)].symbol();
                shown.push_str(symbol);
                x += u16::try_from(Span::raw(symbol).width().max(1)).expect("narrow cell");
            }
            assert!(shown.contains(title), "{title:?} rendered as {shown:?}");
        }
    }
}
