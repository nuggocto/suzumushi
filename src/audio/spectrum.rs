// SPDX-License-Identifier: Apache-2.0

//! Fixed-cost spectrum analysis and lock-free UI projection.

use std::f64::consts::TAU;
use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) const SPECTRUM_BANDS: usize = 16;

const MAX_LEVEL: u8 = 7;
const WINDOW_FRAMES: usize = 1_024;
const MIN_FREQUENCY_HZ: f64 = 60.0;
const MAX_FREQUENCY_HZ: f64 = 16_000.0;
const NYQUIST_MARGIN: f64 = 0.45;
const FLOOR_DB: f64 = -60.0;
const CEILING_DB: f64 = -6.0;
const ATTACK: f64 = 0.8;
const RELEASE: f64 = 0.35;
const FILTER_Q: f64 = 1.2;
const WINDOW_FRAMES_F64: f64 = 1_024.0;
const BAND_EXPONENTS: [i32; SPECTRUM_BANDS] =
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];

/// One bounded visual spectrum. Each band is an integer from zero through seven.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AudioSpectrum {
    levels: [u8; SPECTRUM_BANDS],
}

impl AudioSpectrum {
    #[must_use]
    pub(crate) const fn new(mut levels: [u8; SPECTRUM_BANDS]) -> Self {
        let mut index = 0;
        while index < SPECTRUM_BANDS {
            if levels[index] > MAX_LEVEL {
                levels[index] = MAX_LEVEL;
            }
            index += 1;
        }
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

#[derive(Default)]
pub(super) struct SpectrumLane {
    packed: AtomicU64,
}

impl SpectrumLane {
    pub(super) fn publish(&self, spectrum: AudioSpectrum) {
        self.packed.store(pack(spectrum), Ordering::Release);
    }

    pub(super) fn clear(&self) {
        self.packed.store(0, Ordering::Release);
    }

    pub(super) fn latest(&self) -> AudioSpectrum {
        unpack(self.packed.load(Ordering::Acquire))
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
    input_gain: f64,
    delayed_input_gain: f64,
    feedback_one: f64,
    feedback_two: f64,
    channels: [FilterState; 2],
    displayed: f64,
}

pub(super) struct SpectrumAnalyzer {
    bands: [BandState; SPECTRUM_BANDS],
    channels: usize,
    channel_index: usize,
    frame_count: usize,
}

impl SpectrumAnalyzer {
    pub(super) fn new(sample_rate: u32, channels: u16) -> Self {
        let sample_rate = f64::from(sample_rate);
        let maximum = (sample_rate * NYQUIST_MARGIN).min(MAX_FREQUENCY_HZ);
        let ratio = (maximum / MIN_FREQUENCY_HZ).max(1.0).powf(1.0 / 15.0);
        let bands = std::array::from_fn(|index| {
            let frequency = MIN_FREQUENCY_HZ * ratio.powi(BAND_EXPONENTS[index]);
            let angle = TAU * frequency / sample_rate;
            let alpha = angle.sin() / (2.0 * FILTER_Q);
            let normalization = 1.0 / (1.0 + alpha);
            let input_gain = alpha * normalization;
            BandState {
                input_gain,
                delayed_input_gain: -input_gain,
                feedback_one: -2.0 * angle.cos() * normalization,
                feedback_two: (1.0 - alpha) * normalization,
                ..BandState::default()
            }
        });
        Self {
            bands,
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
        let mut levels = [0; SPECTRUM_BANDS];
        let analyzed_channels = self.channels.min(2);
        let energy_divisor = WINDOW_FRAMES_F64
            * f64::from(u16::try_from(analyzed_channels).expect("at most two analyzed channels"));
        for (index, band) in self.bands.iter_mut().enumerate() {
            let energy = band.channels[..analyzed_channels]
                .iter()
                .map(|channel| channel.energy)
                .sum::<f64>();
            let amplitude = (energy / energy_divisor).max(0.0).sqrt();
            let db = 20.0 * amplitude.max(f64::MIN_POSITIVE).log10();
            let raw = ((db - FLOOR_DB) / (CEILING_DB - FLOOR_DB)).clamp(0.0, 1.0);
            let smoothing = if raw > band.displayed {
                ATTACK
            } else {
                RELEASE
            };
            band.displayed += (raw - band.displayed) * smoothing;
            levels[index] = level_for(band.displayed);
            for channel in &mut band.channels[..analyzed_channels] {
                channel.energy = 0.0;
            }
        }
        self.frame_count = 0;
        AudioSpectrum::new(levels)
    }
}

fn level_for(normalized: f64) -> u8 {
    const THRESHOLDS: [f64; 7] = [
        0.071_429, 0.214_286, 0.357_143, 0.5, 0.642_857, 0.785_714, 0.928_571,
    ];
    u8::try_from(
        THRESHOLDS
            .into_iter()
            .take_while(|threshold| normalized >= *threshold)
            .count(),
    )
    .expect("seven thresholds fit in u8")
}

fn pack(spectrum: AudioSpectrum) -> u64 {
    spectrum
        .levels()
        .into_iter()
        .enumerate()
        .fold(0, |packed, (index, level)| {
            packed | (u64::from(level) << (index * 4))
        })
}

fn unpack(packed: u64) -> AudioSpectrum {
    AudioSpectrum::new(std::array::from_fn(|index| {
        u8::try_from((packed >> (index * 4)) & 0x0f).expect("four bits fit in u8")
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

        assert!(levels.into_iter().max().unwrap_or(0) >= 5, "{levels:?}");
        assert!((7..=9).contains(&dominant_band), "{levels:?}");
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
    fn levels_are_bounded_for_terminal_glyphs() {
        let spectrum = AudioSpectrum::new([u8::MAX; SPECTRUM_BANDS]);

        assert_eq!(spectrum.levels(), [7; SPECTRUM_BANDS]);
    }

    #[test]
    fn latest_projection_replaces_older_spectra() {
        let lane = SpectrumLane::default();
        lane.publish(AudioSpectrum::new([1; SPECTRUM_BANDS]));
        lane.publish(AudioSpectrum::new([6; SPECTRUM_BANDS]));

        assert_eq!(lane.latest(), AudioSpectrum::new([6; SPECTRUM_BANDS]));
        lane.clear();
        assert_eq!(lane.latest(), AudioSpectrum::default());
    }
}
