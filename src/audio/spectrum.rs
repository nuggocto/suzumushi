// SPDX-License-Identifier: Apache-2.0

//! Fixed-cost spectrum analysis and lock-free UI projection.

use std::f64::consts::TAU;
use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) const SPECTRUM_BANDS: usize = 32;

const WINDOW_FRAMES: usize = 1_024;
const MIN_FREQUENCY_HZ: f64 = 50.0;
const MAX_FREQUENCY_HZ: f64 = 16_000.0;
const NYQUIST_MARGIN: f64 = 0.45;
/// Bands quieter than this are silence, whatever the loudness reference.
const SILENCE_DB: f64 = -72.0;
/// Levels span this many decibels below the track's loudness reference, so loud
/// masters and quiet recordings move across the same visual range.
const RANGE_DB: f64 = 28.0;
/// The reference follows a louder window at once and relaxes this much per
/// window, about 2.5 dB per second, so quiet passages regain their range.
const REFERENCE_DECAY_DB: f64 = 0.055;
/// Music loses energy with frequency; this tilt keeps treble bands visible.
const TILT_DB_PER_OCTAVE: f64 = 1.5;
const TILT_REFERENCE_HZ: f64 = 1_000.0;
const ATTACK: f64 = 0.85;
const RELEASE: f64 = 0.45;
/// About a third of an octave per band, with overlap between neighbors.
const FILTER_Q: f64 = 3.2;
const BANDS_PER_WORD: usize = 8;
const LANE_WORDS: usize = SPECTRUM_BANDS / BANDS_PER_WORD;
// 1,024 is exactly representable, so the conversion cannot lose precision.
#[allow(clippy::cast_precision_loss)]
const WINDOW_FRAMES_F64: f64 = WINDOW_FRAMES as f64;

/// One bounded visual spectrum, bass to treble. Each band spans the full `u8` range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AudioSpectrum {
    levels: [u8; SPECTRUM_BANDS],
}

impl AudioSpectrum {
    #[must_use]
    pub(crate) const fn new(levels: [u8; SPECTRUM_BANDS]) -> Self {
        Self { levels }
    }

    #[must_use]
    pub(crate) const fn levels(self) -> [u8; SPECTRUM_BANDS] {
        self.levels
    }

    #[must_use]
    pub(crate) const fn silent() -> Self {
        Self::new([0; SPECTRUM_BANDS])
    }
}

impl Default for AudioSpectrum {
    fn default() -> Self {
        Self::silent()
    }
}

/// Lock-free spectrum hand-off from the audio callback to the terminal loop.
///
/// The bands span several words, so a reader racing a publish can combine two
/// consecutive windows about 20 ms apart. That is invisible in the animation,
/// and it keeps the realtime writer wait-free.
#[derive(Default)]
pub(super) struct SpectrumLane {
    packed: [AtomicU64; LANE_WORDS],
}

impl SpectrumLane {
    pub(super) fn publish(&self, spectrum: AudioSpectrum) {
        for (word, packed) in self.packed.iter().zip(pack(spectrum)) {
            word.store(packed, Ordering::Release);
        }
    }

    pub(super) fn clear(&self) {
        for word in &self.packed {
            word.store(0, Ordering::Release);
        }
    }

    pub(super) fn latest(&self) -> AudioSpectrum {
        unpack(std::array::from_fn(|index| {
            self.packed[index].load(Ordering::Acquire)
        }))
    }
}

#[derive(Clone, Copy, Default)]
struct FilterState {
    delay_one: f64,
    delay_two: f64,
    energy: f64,
}

#[derive(Clone, Copy, Default)]
struct BandState {
    tilt_db: f64,
    input_gain: f64,
    delayed_input_gain: f64,
    feedback_one: f64,
    feedback_two: f64,
    channels: [FilterState; 2],
    displayed: f64,
}

pub(super) struct SpectrumAnalyzer {
    bands: [BandState; SPECTRUM_BANDS],
    reference_db: f64,
    channels: usize,
    channel_index: usize,
    frame_count: usize,
}

impl SpectrumAnalyzer {
    pub(super) fn new(sample_rate: u32, channels: u16) -> Self {
        let sample_rate = f64::from(sample_rate);
        let maximum = (sample_rate * NYQUIST_MARGIN).min(MAX_FREQUENCY_HZ);
        let steps = f64::from(u8::try_from(SPECTRUM_BANDS - 1).expect("bands fit u8"));
        let ratio = (maximum / MIN_FREQUENCY_HZ).max(1.0).powf(1.0 / steps);
        let bands = std::array::from_fn(|index| {
            let frequency =
                MIN_FREQUENCY_HZ * ratio.powi(i32::try_from(index).expect("bands fit i32"));
            let angle = TAU * frequency / sample_rate;
            let alpha = angle.sin() / (2.0 * FILTER_Q);
            let normalization = 1.0 / (1.0 + alpha);
            let input_gain = alpha * normalization;
            BandState {
                tilt_db: TILT_DB_PER_OCTAVE * (frequency / TILT_REFERENCE_HZ).log2(),
                input_gain,
                delayed_input_gain: -input_gain,
                feedback_one: -2.0 * angle.cos() * normalization,
                feedback_two: (1.0 - alpha) * normalization,
                ..BandState::default()
            }
        });
        Self {
            bands,
            reference_db: SILENCE_DB,
            channels: usize::from(channels).max(1),
            channel_index: 0,
            frame_count: 0,
        }
    }

    pub(super) fn push_interleaved(&mut self, sample: f32) -> Option<AudioSpectrum> {
        let analyzed_channels = self.channels.min(2);
        if self.channel_index < analyzed_channels {
            for band in &mut self.bands {
                let channel = &mut band.channels[self.channel_index];
                let sample = f64::from(sample);
                let filtered = band.input_gain.mul_add(sample, channel.delay_one);
                channel.delay_one = (-band.feedback_one).mul_add(filtered, channel.delay_two);
                channel.delay_two = band
                    .delayed_input_gain
                    .mul_add(sample, -band.feedback_two * filtered);
                channel.energy = filtered.mul_add(filtered, channel.energy);
            }
        }
        self.channel_index += 1;
        if self.channel_index < self.channels {
            return None;
        }

        self.channel_index = 0;
        self.frame_count += 1;
        (self.frame_count == WINDOW_FRAMES).then(|| self.finish_window())
    }

    fn finish_window(&mut self) -> AudioSpectrum {
        let analyzed_channels = self.channels.min(2);
        let energy_divisor = WINDOW_FRAMES_F64
            * f64::from(u16::try_from(analyzed_channels).expect("at most two analyzed channels"));
        let mut decibels = [SILENCE_DB; SPECTRUM_BANDS];
        for (db, band) in decibels.iter_mut().zip(&mut self.bands) {
            let energy = band.channels[..analyzed_channels]
                .iter()
                .map(|channel| channel.energy)
                .sum::<f64>();
            let amplitude = (energy / energy_divisor).max(0.0).sqrt();
            *db = 20.0f64.mul_add(amplitude.max(f64::MIN_POSITIVE).log10(), band.tilt_db);
            for channel in &mut band.channels[..analyzed_channels] {
                channel.energy = 0.0;
            }
        }
        let loudest = decibels.iter().copied().fold(SILENCE_DB, f64::max);
        self.reference_db = if loudest > self.reference_db {
            loudest
        } else {
            (self.reference_db - REFERENCE_DECAY_DB).max(SILENCE_DB)
        };
        let floor_db = self.reference_db - RANGE_DB;
        let mut levels = [0; SPECTRUM_BANDS];
        for ((level, band), db) in levels.iter_mut().zip(&mut self.bands).zip(decibels) {
            let raw = if db <= SILENCE_DB {
                0.0
            } else {
                ((db - floor_db) / RANGE_DB).clamp(0.0, 1.0)
            };
            let smoothing = if raw > band.displayed {
                ATTACK
            } else {
                RELEASE
            };
            band.displayed += (raw - band.displayed) * smoothing;
            *level = level_for(band.displayed);
        }
        self.frame_count = 0;
        AudioSpectrum::new(levels)
    }
}

fn level_for(normalized: f64) -> u8 {
    // Clamped to 0..=255 before conversion, so the cast cannot truncate.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let level = (normalized.clamp(0.0, 1.0) * f64::from(u8::MAX)).round() as u8;
    level
}

fn pack(spectrum: AudioSpectrum) -> [u64; LANE_WORDS] {
    let levels = spectrum.levels();
    std::array::from_fn(|word| {
        levels[word * BANDS_PER_WORD..(word + 1) * BANDS_PER_WORD]
            .iter()
            .enumerate()
            .fold(0, |packed, (index, level)| {
                packed | (u64::from(*level) << (index * 8))
            })
    })
}

fn unpack(packed: [u64; LANE_WORDS]) -> AudioSpectrum {
    AudioSpectrum::new(std::array::from_fn(|band| {
        let word = packed[band / BANDS_PER_WORD];
        u8::try_from((word >> ((band % BANDS_PER_WORD) * 8)) & 0xff).expect("eight bits fit u8")
    }))
}

#[cfg(test)]
mod tests {
    use super::{AudioSpectrum, SPECTRUM_BANDS, SpectrumAnalyzer, SpectrumLane, WINDOW_FRAMES};

    fn analyze(samples: impl IntoIterator<Item = f32>, channels: u16) -> AudioSpectrum {
        let mut analyzer = SpectrumAnalyzer::new(48_000, channels);
        samples
            .into_iter()
            .filter_map(|sample| analyzer.push_interleaved(sample))
            .last()
            .expect("one complete analysis window")
    }

    #[test]
    fn silence_has_no_spectral_energy() {
        let spectrum = analyze([0.0].repeat(WINDOW_FRAMES * 2), 2);

        assert_eq!(spectrum, AudioSpectrum::default());
    }

    #[test]
    fn audible_tone_produces_visible_frequency_energy() {
        let samples =
            (0_u16..u16::try_from(WINDOW_FRAMES).expect("window fits in u16")).flat_map(|frame| {
                let phase = std::f32::consts::TAU * 1_000.0 * f32::from(frame) / 48_000.0;
                [phase.sin() * 0.5; 2]
            });

        let spectrum = analyze(samples, 2);
        let levels = spectrum.levels();
        let dominant_band = levels
            .into_iter()
            .enumerate()
            .max_by_key(|(_, level)| *level)
            .map(|(index, _)| index)
            .expect("the fixed spectrum has bands");

        // 1 kHz sits about 4.4 octaves above 50 Hz, near band 17 of 32.
        assert!(levels.into_iter().max().unwrap_or(0) >= 160, "{levels:?}");
        assert!((16..=18).contains(&dominant_band), "{levels:?}");
    }

    #[test]
    fn anti_phase_stereo_keeps_the_same_energy_as_in_phase_stereo() {
        let tone = |frame: u16| {
            let phase = std::f32::consts::TAU * 1_000.0 * f32::from(frame) / 48_000.0;
            phase.sin() * 0.5
        };
        let frames = 0_u16..u16::try_from(WINDOW_FRAMES).expect("window fits in u16");
        let in_phase = analyze(frames.clone().flat_map(|frame| [tone(frame); 2]), 2);
        let anti_phase = analyze(frames.flat_map(|frame| [tone(frame), -tone(frame)]), 2);

        assert_eq!(anti_phase, in_phase);
        assert!(anti_phase.levels().into_iter().any(|level| level > 0));
    }

    #[test]
    fn every_band_keeps_its_full_level_through_the_lane() {
        let levels =
            std::array::from_fn(|band| u8::try_from(band * 8 + 3).expect("distinct levels fit u8"));
        let lane = SpectrumLane::default();
        lane.publish(AudioSpectrum::new([1; SPECTRUM_BANDS]));
        lane.publish(AudioSpectrum::new(levels));

        assert_eq!(lane.latest().levels(), levels);
        lane.clear();
        assert_eq!(lane.latest(), AudioSpectrum::default());
    }
}
