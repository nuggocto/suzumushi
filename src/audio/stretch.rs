// SPDX-License-Identifier: Apache-2.0

//! Bounded worker-side pitch-preserving time stretching.

use wsola::TimeStretch;

use super::{AudioFormat, PlaybackSpeed};

const MAX_STRETCH_BLOCK_SAMPLES: usize = 32_768;
const MAX_STRETCH_BUFFERED_SAMPLES: usize = 262_144;

pub(super) struct TempoProcessor {
    inner: TimeStretch,
    channels: usize,
    speed: PlaybackSpeed,
    finished: bool,
    produced: bool,
    short_input: Vec<f32>,
    flushed: Option<(Vec<f32>, usize)>,
}

impl TempoProcessor {
    pub(super) fn new(format: AudioFormat, speed: PlaybackSpeed) -> Result<Self, String> {
        let mut inner = TimeStretch::new(format.sample_rate, format.channels)
            .map_err(|error| format!("cannot create the pitch-preserving stretcher: {error}"))?;
        inner.set_tempo(speed.multiplier());
        Ok(Self {
            inner,
            channels: usize::from(format.channels),
            speed,
            finished: false,
            produced: false,
            short_input: Vec::new(),
            flushed: None,
        })
    }

    pub(super) fn push(&mut self, samples: &[f32]) -> Result<(), String> {
        if samples.is_empty() || !samples.len().is_multiple_of(self.channels) {
            return Err("time stretcher received a partial PCM frame".into());
        }
        if samples.len() > MAX_STRETCH_BLOCK_SAMPLES {
            return Err(format!(
                "time stretcher input exceeded {MAX_STRETCH_BLOCK_SAMPLES} samples"
            ));
        }
        let buffered = self
            .inner
            .buffered()
            .checked_mul(self.channels)
            .and_then(|value| value.checked_add(samples.len()))
            .ok_or_else(|| "time stretcher buffer accounting overflow".to_owned())?;
        if buffered > MAX_STRETCH_BUFFERED_SAMPLES {
            return Err(format!(
                "time stretcher exceeded its {MAX_STRETCH_BUFFERED_SAMPLES}-sample input bound"
            ));
        }
        if !self.produced {
            self.short_input
                .try_reserve(samples.len())
                .map_err(|error| format!("cannot reserve short-clip PCM: {error}"))?;
            self.short_input.extend_from_slice(samples);
        }
        self.inner.push(samples);
        Ok(())
    }

    pub(super) fn pull(&mut self) -> Result<Option<Vec<f32>>, String> {
        if let Some((samples, offset)) = self.flushed.as_mut() {
            let end = offset
                .saturating_add(MAX_STRETCH_BLOCK_SAMPLES)
                .min(samples.len());
            let mut block = Vec::new();
            block
                .try_reserve_exact(end - *offset)
                .map_err(|error| format!("cannot reserve stretched PCM block: {error}"))?;
            block.extend_from_slice(&samples[*offset..end]);
            *offset = end;
            if end == samples.len() {
                self.flushed = None;
            }
            return Ok((!block.is_empty()).then_some(block));
        }
        if self.finished {
            return Ok(None);
        }
        let block = self.inner.pull(MAX_STRETCH_BLOCK_SAMPLES);
        validate_output(&block)?;
        if !block.is_empty() {
            self.produced = true;
            self.short_input.clear();
        }
        Ok((!block.is_empty()).then_some(block))
    }

    pub(super) fn finish(&mut self) -> Result<(), String> {
        if self.flushed.is_some() {
            return Err("time stretcher was finished twice".into());
        }
        let mut samples = self.inner.flush();
        if samples.is_empty() && !self.short_input.is_empty() {
            samples = stretch_short_clip(&self.short_input, self.channels, self.speed)?;
            self.short_input.clear();
        } else {
            self.short_input.clear();
        }
        if samples.len() > MAX_STRETCH_BUFFERED_SAMPLES {
            return Err(format!(
                "time stretcher flush exceeded {MAX_STRETCH_BUFFERED_SAMPLES} samples"
            ));
        }
        validate_output(&samples)?;
        self.finished = true;
        self.flushed = Some((samples, 0));
        Ok(())
    }

    pub(super) fn drained(&self) -> bool {
        self.finished && self.flushed.is_none()
    }
}

fn stretch_short_clip(
    samples: &[f32],
    channels: usize,
    speed: PlaybackSpeed,
) -> Result<Vec<f32>, String> {
    let input_frames = samples.len() / channels;
    let speed_percent = usize::from(speed.percent());
    let target_frames = input_frames
        .checked_mul(100)
        .and_then(|frames| frames.checked_add(speed_percent / 2))
        .map_or(0, |frames| frames / speed_percent)
        .max(1);
    let target_samples = target_frames
        .checked_mul(channels)
        .ok_or_else(|| "short-clip stretch size overflow".to_owned())?;
    if target_samples > MAX_STRETCH_BUFFERED_SAMPLES {
        return Err(format!(
            "short-clip stretch exceeded {MAX_STRETCH_BUFFERED_SAMPLES} samples"
        ));
    }
    let mut output = Vec::new();
    output
        .try_reserve_exact(target_samples)
        .map_err(|error| format!("cannot reserve short-clip stretch: {error}"))?;
    // A sub-window clip has no second grain to overlap. Keeping each frame's
    // sample cadence while truncating or repeating avoids silently reverting to 1.0x.
    for frame in 0..target_frames {
        let source_frame = frame % input_frames;
        let start = source_frame * channels;
        output.extend_from_slice(&samples[start..start + channels]);
    }
    Ok(output)
}

fn validate_output(samples: &[f32]) -> Result<(), String> {
    if samples.iter().any(|sample| !sample.is_finite()) {
        Err("time stretcher produced a non-finite sample".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::PI;

    use super::TempoProcessor;
    use crate::audio::{AudioFormat, PlaybackSpeed};

    const SAMPLE_RATE: u32 = 48_000;

    fn tone(seconds: usize) -> Vec<f32> {
        (0..SAMPLE_RATE as usize * seconds)
            .map(|sample| {
                let sample = u16::try_from(sample).expect("one-second fixture fits u16");
                let rate = u16::try_from(SAMPLE_RATE).expect("fixture rate fits u16");
                (2.0 * PI * 440.0 * f32::from(sample) / f32::from(rate)).sin()
            })
            .collect()
    }

    fn process(speed: PlaybackSpeed) -> Vec<f32> {
        let input = tone(1);
        let mut processor = TempoProcessor::new(
            AudioFormat {
                sample_rate: SAMPLE_RATE,
                channels: 1,
            },
            speed,
        )
        .expect("create stretcher");
        let mut output = Vec::new();
        for block in input.chunks(8_192) {
            processor.push(block).expect("push PCM");
            while let Some(block) = processor.pull().expect("pull PCM") {
                output.extend(block);
            }
        }
        processor.finish().expect("finish PCM");
        while let Some(block) = processor.pull().expect("drain PCM") {
            output.extend(block);
        }
        assert!(processor.drained());
        output
    }

    fn zero_crossing_frequency(samples: &[f32]) -> f64 {
        let middle = &samples[samples.len() / 8..samples.len() * 7 / 8];
        let crossings = middle
            .windows(2)
            .filter(|pair| pair[0].is_sign_negative() != pair[1].is_sign_negative())
            .count();
        let crossings = u32::try_from(crossings).expect("fixture crossing count fits u32");
        let samples = u32::try_from(middle.len()).expect("fixture length fits u32");
        f64::from(crossings) * f64::from(SAMPLE_RATE) / (2.0 * f64::from(samples))
    }

    #[test]
    fn half_and_double_speed_change_duration_without_moving_pitch() {
        let half = process(PlaybackSpeed::HALF);
        let double = process(PlaybackSpeed::DOUBLE);

        assert!((90_000..=102_000).contains(&half.len()), "{}", half.len());
        assert!(
            (21_000..=27_000).contains(&double.len()),
            "{}",
            double.len()
        );
        assert!((zero_crossing_frequency(&half) - 440.0).abs() < 30.0);
        assert!((zero_crossing_frequency(&double) - 440.0).abs() < 30.0);
    }

    #[test]
    fn a_sub_window_clip_still_honors_the_selected_speed() {
        let input = vec![0.25; 512];
        let mut faster = TempoProcessor::new(
            AudioFormat {
                sample_rate: SAMPLE_RATE,
                channels: 1,
            },
            PlaybackSpeed::DOUBLE,
        )
        .expect("create stretcher");
        faster.push(&input).expect("push short PCM");
        faster.finish().expect("finish short PCM");
        let faster = faster
            .pull()
            .expect("pull faster PCM")
            .expect("faster output");

        let mut slower = TempoProcessor::new(
            AudioFormat {
                sample_rate: SAMPLE_RATE,
                channels: 1,
            },
            PlaybackSpeed::HALF,
        )
        .expect("create stretcher");
        slower.push(&input).expect("push short PCM");
        slower.finish().expect("finish short PCM");
        let slower = slower
            .pull()
            .expect("pull slower PCM")
            .expect("slower output");

        assert_eq!(faster.len(), input.len() / 2);
        assert_eq!(slower.len(), input.len() * 2);
    }
}
