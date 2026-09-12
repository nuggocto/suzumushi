// SPDX-License-Identifier: Apache-2.0

//! CPAL output fed by one realtime-safe SPSC ring.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    BufferSize, Device, Error, ErrorKind, FromSample, I24, OutputCallbackInfo, SampleFormat,
    SizedSample, Stream, StreamConfig, StreamInstant, SupportedStreamConfigRange, U24,
};
use rtrb::{Consumer, Producer, RingBuffer};

use super::spectrum::{SpectrumAnalyzer, SpectrumLane};
use super::{AudioFormat, AudioMetrics, OutputStream};

const PCM_RING_SAMPLES: usize = 262_144;
const MIN_SAMPLE_RATE: u32 = 8_000;
const MAX_SAMPLE_RATE: u32 = 192_000;
const MAX_SOURCE_CHANNELS: u16 = 2;
const MAX_OUTPUT_CHANNELS: u16 = 8;

pub(super) struct CpalOutput {
    source: Option<AudioFormat>,
    output_channels: usize,
    stream: Option<Stream>,
    producer: Option<Producer<f32>>,
    progress: Arc<OutputProgress>,
    end_of_stream: Arc<AtomicBool>,
    failure: Arc<AtomicU8>,
    gain: Arc<AtomicU32>,
    spectrum: Arc<SpectrumLane>,
    metrics: Arc<AudioMetrics>,
    written: u64,
    drain_deadline: Option<StreamInstant>,
}

#[derive(Default)]
struct OutputProgress {
    playing: AtomicBool,
    consumed: AtomicU64,
    playback_delay_nanos: AtomicU64,
}

impl CpalOutput {
    pub(super) fn with_metrics(spectrum: Arc<SpectrumLane>, metrics: Arc<AudioMetrics>) -> Self {
        Self {
            source: None,
            output_channels: 0,
            stream: None,
            producer: None,
            progress: Arc::new(OutputProgress::default()),
            end_of_stream: Arc::new(AtomicBool::new(false)),
            failure: Arc::new(AtomicU8::new(0)),
            gain: Arc::new(AtomicU32::new(1.0_f32.to_bits())),
            spectrum,
            metrics,
            written: 0,
            drain_deadline: None,
        }
    }

    fn drained_at(&mut self, now: StreamInstant) -> bool {
        if !self.progress.playing.load(Ordering::Acquire)
            || self.progress.consumed.load(Ordering::Acquire) < self.written
        {
            self.drain_deadline = None;
            return false;
        }
        if let Some(deadline) = self.drain_deadline {
            return now >= deadline;
        }
        // Ring consumption precedes hardware playback. Wait conservatively for
        // the last callback's whole buffer plus its reported backend delay. The
        // worker owns this deadline so pause/resume can restart the wait safely.
        let delay =
            Duration::from_nanos(self.progress.playback_delay_nanos.load(Ordering::Relaxed));
        self.drain_deadline = now.checked_add(delay);
        if self.drain_deadline.is_none() {
            self.failure.store(1, Ordering::Release);
        }
        false
    }
}

impl OutputStream for CpalOutput {
    fn prepare(&mut self, source: AudioFormat) -> Result<(), String> {
        if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&source.sample_rate) {
            return Err(format!(
                "track sample rate {} Hz is outside the supported {}..={} Hz range",
                source.sample_rate, MIN_SAMPLE_RATE, MAX_SAMPLE_RATE
            ));
        }
        if source.channels == 0 || source.channels > MAX_SOURCE_CHANNELS {
            return Err(format!(
                "track has {} channels; this build supports mono and stereo",
                source.channels
            ));
        }

        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| "no default audio output device is available".to_owned())?;
        let ranges = device
            .supported_output_configs()
            .map_err(|error| format!("cannot query the default output device: {error}"))?;
        let choice = select_output_config(ranges, source.sample_rate).ok_or_else(|| {
            format!(
                "the default output device has no supported PCM configuration for {} channel audio",
                source.channels
            )
        })?;
        self.output_channels = usize::from(choice.channels);
        let sample_format = choice.sample_format;
        let config = choice.config;
        let (producer, pcm_reader) = RingBuffer::new(PCM_RING_SAMPLES);
        self.progress = Arc::new(OutputProgress::default());
        self.end_of_stream.store(false, Ordering::Release);
        self.failure.store(0, Ordering::Release);
        self.written = 0;
        self.drain_deadline = None;
        self.spectrum.clear();
        let progress = Arc::clone(&self.progress);
        let end_of_stream = Arc::clone(&self.end_of_stream);
        let failure = Arc::clone(&self.failure);
        let gain = Arc::clone(&self.gain);
        let spectrum = Arc::clone(&self.spectrum);
        let metrics = Arc::clone(&self.metrics);
        let stream = build_stream(
            &device,
            &config,
            sample_format,
            pcm_reader,
            CallbackState {
                progress,
                end_of_stream,
                failure,
                gain,
                spectrum,
                metrics,
            },
        )
        .map_err(|error| {
            // The device may not advertise this rate at all: name it, so a genuine
            // rate refusal is not left behind an opaque backend message.
            format!(
                "{error} (requested {} Hz on {} channels as {sample_format})",
                source.sample_rate, choice.channels
            )
        })?;
        // Some backends start callbacks immediately. The callback's playback
        // gate remains closed even before this backend pause is processed.
        stream
            .pause()
            .map_err(|error| format!("cannot prepare paused audio output: {error}"))?;
        self.source = Some(source);
        self.producer = Some(producer);
        self.stream = Some(stream);
        Ok(())
    }

    fn set_gain(&mut self, volume_percent: u8, muted: bool) {
        let gain = if muted {
            0.0
        } else {
            f32::from(volume_percent.min(100)) * 0.01
        };
        self.gain.store(gain.to_bits(), Ordering::Relaxed);
    }

    fn write(&mut self, samples: &[f32]) -> Result<usize, String> {
        let source = self
            .source
            .ok_or_else(|| "audio output was not prepared".to_owned())?;
        let source_channels = usize::from(source.channels);
        if !samples.len().is_multiple_of(source_channels) {
            return Err("decoder returned a partial PCM frame".into());
        }
        let producer = self
            .producer
            .as_mut()
            .ok_or_else(|| "audio output ring is unavailable".to_owned())?;
        let frames = (samples.len() / source_channels).min(producer.slots() / self.output_channels);
        let output_samples = frames
            .checked_mul(self.output_channels)
            .ok_or_else(|| "audio output sample count overflow".to_owned())?;
        self.written = self
            .written
            .checked_add(
                u64::try_from(output_samples)
                    .map_err(|_| "audio output sample count overflow".to_owned())?,
            )
            .ok_or_else(|| "audio output sample generation exhausted".to_owned())?;
        self.drain_deadline = None;
        for frame in samples.chunks_exact(source_channels).take(frames) {
            match (source_channels, self.output_channels) {
                (1, output_channels) => {
                    for _ in 0..output_channels {
                        producer
                            .push(finite_or_silence(frame[0]))
                            .map_err(|_| "audio output ring changed while writing".to_owned())?;
                    }
                }
                (2, 1) => {
                    producer
                        .push(finite_or_silence(frame[0].midpoint(frame[1])))
                        .map_err(|_| "audio output ring changed while writing".to_owned())?;
                }
                (2, output_channels) => {
                    producer
                        .push(finite_or_silence(frame[0]))
                        .map_err(|_| "audio output ring changed while writing".to_owned())?;
                    producer
                        .push(finite_or_silence(frame[1]))
                        .map_err(|_| "audio output ring changed while writing".to_owned())?;
                    for _ in 2..output_channels {
                        producer
                            .push(0.0)
                            .map_err(|_| "audio output ring changed while writing".to_owned())?;
                    }
                }
                _ => return Err("unsupported channel conversion".into()),
            }
        }
        Ok(frames * source_channels)
    }

    fn finish_input(&mut self) {
        self.end_of_stream.store(true, Ordering::Release);
    }

    fn play(&mut self) -> Result<(), String> {
        self.stream
            .as_ref()
            .ok_or_else(|| "audio output stream is unavailable".to_owned())?
            .play()
            .map_err(|error| format!("cannot start the audio output stream: {error}"))?;
        self.drain_deadline = None;
        self.progress.playing.store(true, Ordering::Release);
        Ok(())
    }

    fn pause(&mut self) -> Result<(), String> {
        self.drain_deadline = None;
        self.spectrum.clear();
        if !self.progress.playing.swap(false, Ordering::AcqRel) {
            return Ok(());
        }
        self.stream
            .as_ref()
            .ok_or_else(|| "audio output stream is unavailable".to_owned())?
            .pause()
            .map_err(|error| format!("cannot pause the audio output stream: {error}"))?;
        Ok(())
    }

    fn resume(&mut self) -> Result<(), String> {
        self.play()
    }

    fn stop(&mut self) -> Result<(), String> {
        let pause_error = self.pause().err();
        self.stream.take();
        self.producer.take();
        self.source = None;
        self.spectrum.clear();
        pause_error.map_or(Ok(()), Err)
    }

    fn consumed_frames(&self) -> u64 {
        let channels = u64::try_from(self.output_channels).unwrap_or(u64::MAX);
        self.progress
            .consumed
            .load(Ordering::Acquire)
            .checked_div(channels)
            .unwrap_or(0)
    }

    fn drained(&mut self) -> bool {
        let Some(now) = self.stream.as_ref().map(StreamTrait::now) else {
            return false;
        };
        self.drained_at(now)
    }

    fn failure(&self) -> Option<String> {
        (self.failure.load(Ordering::Acquire) != 0)
            .then(|| "the audio output callback reported a device failure".into())
    }
}

fn build_stream(
    device: &Device,
    config: &StreamConfig,
    format: SampleFormat,
    pcm_reader: Consumer<f32>,
    callback: CallbackState,
) -> Result<Stream, String> {
    match format {
        SampleFormat::I8 => typed_stream::<i8>(device, config, pcm_reader, callback),
        SampleFormat::I16 => typed_stream::<i16>(device, config, pcm_reader, callback),
        SampleFormat::I24 => typed_stream::<I24>(device, config, pcm_reader, callback),
        SampleFormat::I32 => typed_stream::<i32>(device, config, pcm_reader, callback),
        SampleFormat::I64 => typed_stream::<i64>(device, config, pcm_reader, callback),
        SampleFormat::U8 => typed_stream::<u8>(device, config, pcm_reader, callback),
        SampleFormat::U16 => typed_stream::<u16>(device, config, pcm_reader, callback),
        SampleFormat::U24 => typed_stream::<U24>(device, config, pcm_reader, callback),
        SampleFormat::U32 => typed_stream::<u32>(device, config, pcm_reader, callback),
        SampleFormat::U64 => typed_stream::<u64>(device, config, pcm_reader, callback),
        SampleFormat::F32 => typed_stream::<f32>(device, config, pcm_reader, callback),
        SampleFormat::F64 => typed_stream::<f64>(device, config, pcm_reader, callback),
        _ => Err(format!("unsupported output sample format {format}")),
    }
}

struct CallbackState {
    progress: Arc<OutputProgress>,
    end_of_stream: Arc<AtomicBool>,
    failure: Arc<AtomicU8>,
    gain: Arc<AtomicU32>,
    spectrum: Arc<SpectrumLane>,
    metrics: Arc<AudioMetrics>,
}

fn typed_stream<T>(
    device: &Device,
    config: &StreamConfig,
    mut pcm_reader: Consumer<f32>,
    callback: CallbackState,
) -> Result<Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    let CallbackState {
        progress,
        end_of_stream,
        failure,
        gain,
        spectrum,
        metrics,
    } = callback;
    let mut analyzer = SpectrumAnalyzer::new(config.sample_rate, config.channels);
    let format = AudioFormat {
        sample_rate: config.sample_rate,
        channels: config.channels,
    };
    let callback_metrics = Arc::clone(&metrics);
    let error_metrics = Arc::clone(&metrics);
    device
        .build_output_stream(
            *config,
            move |output: &mut [T], info: &OutputCallbackInfo| {
                let timestamp = info.timestamp();
                let gain = f32::from_bits(gain.load(Ordering::Relaxed));
                render_output(
                    output,
                    RenderContext {
                        pcm_reader: &mut pcm_reader,
                        gain,
                        analyzer: &mut analyzer,
                        progress: &progress,
                        timing: OutputTiming {
                            format,
                            backend_delay: timestamp.playback.duration_since(timestamp.callback),
                        },
                        end_of_stream: &end_of_stream,
                        metrics: &callback_metrics,
                        spectrum: &spectrum,
                    },
                );
            },
            move |error| record_stream_error(&error, &failure, &error_metrics),
            None,
        )
        .map_err(|error| format!("cannot build the audio output stream: {error}"))
}

struct RenderContext<'a> {
    pcm_reader: &'a mut Consumer<f32>,
    gain: f32,
    analyzer: &'a mut SpectrumAnalyzer,
    progress: &'a OutputProgress,
    timing: OutputTiming,
    end_of_stream: &'a AtomicBool,
    metrics: &'a AudioMetrics,
    spectrum: &'a SpectrumLane,
}

struct OutputTiming {
    format: AudioFormat,
    backend_delay: Duration,
}

fn render_output<T>(output: &mut [T], context: RenderContext<'_>)
where
    T: SizedSample + FromSample<f32>,
{
    let RenderContext {
        pcm_reader,
        gain,
        analyzer,
        progress,
        timing,
        end_of_stream,
        metrics,
        spectrum,
    } = context;
    if !progress.playing.load(Ordering::Acquire) {
        output.fill(T::EQUILIBRIUM);
        return;
    }
    let frames = output.len().div_ceil(usize::from(timing.format.channels));
    let buffer_nanos = u64::try_from(frames)
        .unwrap_or(u64::MAX)
        .saturating_mul(1_000_000_000)
        .div_ceil(u64::from(timing.format.sample_rate));
    let mut read = 0_u64;
    let mut underruns = 0_u64;
    let mut published = None;
    for sample in output {
        let value = if let Ok(value) = pcm_reader.pop() {
            read += 1;
            finite_or_silence(value * gain)
        } else {
            if !end_of_stream.load(Ordering::Acquire) {
                underruns += 1;
            }
            0.0
        };
        *sample = T::from_sample(value);
        if let Some(levels) = analyzer.push_interleaved(value) {
            published = Some(levels);
        }
    }
    if let Some(levels) = published {
        spectrum.publish(levels);
    }
    if underruns != 0 {
        metrics
            .underrun_samples
            .fetch_add(underruns, Ordering::Relaxed);
    }
    if read != 0 {
        let delay = timing
            .backend_delay
            .saturating_add(Duration::from_nanos(buffer_nanos));
        progress.playback_delay_nanos.store(
            u64::try_from(delay.as_nanos()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        // Publish the delay before consumption makes the worker eligible to drain.
        progress.consumed.fetch_add(read, Ordering::Release);
    }
}

/// Records one CPAL error callback.
///
/// The first three kinds are recoverable: CPAL keeps the stream running, so they
/// are counted for diagnostics only. `DeviceChanged` means the route moved and
/// CPAL rerouted the stream itself; a stream that genuinely needs rebuilding is
/// reported as `StreamInvalidated`, which falls through to the fatal arm.
fn record_stream_error(error: &Error, failure: &AtomicU8, metrics: &AudioMetrics) {
    match error.kind() {
        ErrorKind::RealtimeDenied => {
            metrics.realtime_denied.fetch_add(1, Ordering::Relaxed);
        }
        ErrorKind::Xrun => {
            metrics.xruns.fetch_add(1, Ordering::Relaxed);
        }
        ErrorKind::DeviceChanged => {
            metrics.device_changes.fetch_add(1, Ordering::Relaxed);
        }
        _ => failure.store(1, Ordering::Release),
    }
}

/// The output stream settings chosen for one track.
struct OutputChoice {
    channels: u16,
    sample_format: SampleFormat,
    config: StreamConfig,
}

/// Picks the output configuration for a track recorded at `sample_rate`.
///
/// A device that advertises the track's rate is preferred. When none does, the
/// best remaining configuration is opened at the track's rate anyway, because an
/// advertised range is not the same as a hard limit: `PipeWire` reports only its
/// current graph rate yet resamples internally, so refusing here would reject
/// every 44.1 kHz track on an otherwise working 48 kHz graph. A backend that
/// truly cannot accept the rate still fails, but later and with its own message.
fn select_output_config(
    ranges: impl Iterator<Item = SupportedStreamConfigRange>,
    sample_rate: u32,
) -> Option<OutputChoice> {
    let usable: Vec<SupportedStreamConfigRange> = ranges
        .filter(|range| {
            range.channels() > 0
                && range.channels() <= MAX_OUTPUT_CHANNELS
                && is_pcm_format(range.sample_format())
        })
        .collect();
    let chosen = usable
        .iter()
        .filter(|range| range.contains_rate(sample_rate))
        .max_by(|left, right| left.cmp_default_heuristics(right))
        .or_else(|| {
            usable
                .iter()
                .max_by(|left, right| left.cmp_default_heuristics(right))
        })?;
    let channels = chosen.channels();
    Some(OutputChoice {
        channels,
        sample_format: chosen.sample_format(),
        config: StreamConfig {
            channels,
            sample_rate,
            buffer_size: BufferSize::Default,
        },
    })
}

fn is_pcm_format(format: SampleFormat) -> bool {
    matches!(
        format,
        SampleFormat::I8
            | SampleFormat::I16
            | SampleFormat::I24
            | SampleFormat::I32
            | SampleFormat::I64
            | SampleFormat::U8
            | SampleFormat::U16
            | SampleFormat::U24
            | SampleFormat::U32
            | SampleFormat::U64
            | SampleFormat::F32
            | SampleFormat::F64
    )
}

fn finite_or_silence(sample: f32) -> f32 {
    if sample.is_finite() {
        sample.clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
    use std::time::Duration;

    use crate::audio::{AudioSpectrum, SPECTRUM_BANDS};

    use cpal::{SampleFormat, SupportedBufferSize, SupportedStreamConfigRange};

    use super::{
        AudioFormat, AudioMetrics, CpalOutput, Error, ErrorKind, OutputProgress, OutputStream,
        OutputTiming, SpectrumLane, StreamInstant, record_stream_error, render_output,
        select_output_config,
    };

    fn playing_progress() -> OutputProgress {
        OutputProgress {
            playing: AtomicBool::new(true),
            ..OutputProgress::default()
        }
    }

    fn timing(channels: u16) -> OutputTiming {
        OutputTiming {
            format: AudioFormat {
                sample_rate: 48_000,
                channels,
            },
            backend_delay: Duration::ZERO,
        }
    }

    fn range(
        channels: u16,
        min_rate: u32,
        max_rate: u32,
        format: SampleFormat,
    ) -> SupportedStreamConfigRange {
        SupportedStreamConfigRange::new(
            channels,
            min_rate,
            max_rate,
            SupportedBufferSize::Unknown,
            format,
        )
    }

    #[test]
    fn a_device_advertising_only_its_graph_rate_still_opens_at_the_track_rate() {
        // What PipeWire reports: one fixed 48 kHz node that resamples internally.
        let ranges = [range(2, 48_000, 48_000, SampleFormat::F32)];

        let choice = select_output_config(ranges.into_iter(), 44_100)
            .expect("a 48 kHz graph must still accept a 44.1 kHz track");

        assert_eq!(choice.config.sample_rate, 44_100);
        assert_eq!(choice.config.channels, 2);
        assert_eq!(choice.channels, 2);
    }

    #[test]
    fn an_advertised_rate_match_wins_over_the_resampling_fallback() {
        // What ALSA reports: a wide plug range alongside a fixed one.
        let ranges = [
            range(6, 48_000, 48_000, SampleFormat::F32),
            range(2, 8_000, 192_000, SampleFormat::F32),
        ];

        let choice = select_output_config(ranges.into_iter(), 44_100).expect("a config");

        assert_eq!(choice.config.sample_rate, 44_100);
        assert_eq!(choice.channels, 2, "the range covering 44.1 kHz must win");
    }

    #[test]
    fn ranges_outside_the_supported_channel_bounds_are_refused() {
        let ranges = [
            range(0, 8_000, 192_000, SampleFormat::F32),
            range(64, 8_000, 192_000, SampleFormat::F32),
        ];

        assert!(select_output_config(ranges.into_iter(), 44_100).is_none());
    }

    #[test]
    fn inactive_pause_clears_spectrum_and_remains_idempotent() {
        let spectrum = Arc::new(SpectrumLane::default());
        spectrum.publish(AudioSpectrum::new([5; SPECTRUM_BANDS]));
        let mut output =
            CpalOutput::with_metrics(Arc::clone(&spectrum), Arc::new(AudioMetrics::default()));
        output
            .pause()
            .expect("an inactive stream is already paused");
        assert_eq!(spectrum.latest(), AudioSpectrum::default());
        output.stop().expect("an inactive stream stops cleanly");
    }

    #[test]
    fn an_empty_ring_is_rendered_as_silence_and_counted_without_waiting() {
        let (_producer, mut consumer) = rtrb::RingBuffer::new(8);
        let metrics = AudioMetrics::default();
        let progress = playing_progress();
        let end_of_stream = AtomicBool::new(false);
        let spectrum = SpectrumLane::default();
        let mut analyzer = super::SpectrumAnalyzer::new(48_000, 2);
        let mut output = [1.0_f32; 4];

        render_output(
            &mut output,
            super::RenderContext {
                pcm_reader: &mut consumer,
                gain: 1.0,
                analyzer: &mut analyzer,
                progress: &progress,
                timing: timing(2),
                end_of_stream: &end_of_stream,
                metrics: &metrics,
                spectrum: &spectrum,
            },
        );

        assert_eq!(output.map(f32::to_bits), [0; 4]);
        assert_eq!(progress.consumed.load(Ordering::Relaxed), 0);
        assert_eq!(
            metrics.underrun_samples.load(Ordering::Relaxed),
            output.len() as u64
        );
    }

    #[test]
    fn expected_tail_padding_is_not_counted_as_an_underrun() {
        let (mut producer, mut consumer) = rtrb::RingBuffer::new(8);
        producer.push(0.25).expect("first scheduled sample");
        producer.push(-0.25).expect("second scheduled sample");
        let metrics = AudioMetrics::default();
        let progress = playing_progress();
        let end_of_stream = AtomicBool::new(true);
        let spectrum = SpectrumLane::default();
        let mut analyzer = super::SpectrumAnalyzer::new(48_000, 1);
        let mut output = [1.0_f32; 4];

        render_output(
            &mut output,
            super::RenderContext {
                pcm_reader: &mut consumer,
                gain: 1.0,
                analyzer: &mut analyzer,
                progress: &progress,
                timing: timing(1),
                end_of_stream: &end_of_stream,
                metrics: &metrics,
                spectrum: &spectrum,
            },
        );

        assert_eq!(progress.consumed.load(Ordering::Relaxed), 2);
        assert_eq!(metrics.underrun_samples.load(Ordering::Relaxed), 0);
        let bits = output.map(f32::to_bits);
        assert_eq!(&bits[2..], &[0, 0]);
    }

    #[test]
    fn decoder_starvation_after_consuming_committed_samples_is_counted() {
        let (mut producer, mut consumer) = rtrb::RingBuffer::new(8);
        producer.push(0.25).expect("committed sample");
        let metrics = AudioMetrics::default();
        let progress = playing_progress();
        let end_of_stream = AtomicBool::new(false);
        let spectrum = SpectrumLane::default();
        let mut analyzer = super::SpectrumAnalyzer::new(48_000, 1);
        let mut output = [1.0_f32; 4];
        render_output(
            &mut output,
            super::RenderContext {
                pcm_reader: &mut consumer,
                gain: 1.0,
                analyzer: &mut analyzer,
                progress: &progress,
                timing: timing(1),
                end_of_stream: &end_of_stream,
                metrics: &metrics,
                spectrum: &spectrum,
            },
        );
        assert_eq!(
            output.map(f32::to_bits),
            [0.25_f32, 0.0, 0.0, 0.0].map(f32::to_bits)
        );
        assert_eq!(metrics.underrun_samples.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn callbacks_before_play_preserve_pcm_and_render_silence() {
        let (mut producer, mut consumer) = rtrb::RingBuffer::new(8);
        producer.push(0.25).expect("left sample");
        producer.push(-0.25).expect("right sample");
        let progress = OutputProgress::default();
        let metrics = AudioMetrics::default();
        let end_of_stream = AtomicBool::new(true);
        let spectrum = SpectrumLane::default();
        let mut analyzer = super::SpectrumAnalyzer::new(48_000, 2);
        let mut output = [1.0_f32; 4];

        render_output(
            &mut output,
            super::RenderContext {
                pcm_reader: &mut consumer,
                gain: 1.0,
                analyzer: &mut analyzer,
                progress: &progress,
                timing: timing(2),
                end_of_stream: &end_of_stream,
                metrics: &metrics,
                spectrum: &spectrum,
            },
        );

        assert_eq!(output.map(f32::to_bits), [0; 4]);
        assert_eq!(progress.consumed.load(Ordering::Acquire), 0);
        assert_eq!(metrics.underrun_samples.load(Ordering::Relaxed), 0);

        progress.playing.store(true, Ordering::Release);
        render_output(
            &mut output,
            super::RenderContext {
                pcm_reader: &mut consumer,
                gain: 1.0,
                analyzer: &mut analyzer,
                progress: &progress,
                timing: timing(2),
                end_of_stream: &end_of_stream,
                metrics: &metrics,
                spectrum: &spectrum,
            },
        );
        assert_eq!(
            output.map(f32::to_bits),
            [0.25_f32, -0.25, 0.0, 0.0].map(f32::to_bits)
        );
        assert_eq!(progress.consumed.load(Ordering::Acquire), 2);
    }

    #[test]
    fn ring_consumption_waits_for_backend_latency_and_the_final_buffer() {
        let spectrum = Arc::new(SpectrumLane::default());
        let metrics = Arc::new(AudioMetrics::default());
        let mut device = CpalOutput::with_metrics(Arc::clone(&spectrum), Arc::clone(&metrics));
        let (mut producer, mut consumer) = rtrb::RingBuffer::new(80);
        for _ in 0..40 {
            producer.push(0.25).expect("final PCM samples");
        }
        device.written = 40;
        device.progress.playing.store(true, Ordering::Release);
        let end_of_stream = AtomicBool::new(true);
        let mut analyzer = super::SpectrumAnalyzer::new(8_000, 1);
        let mut buffer = [0.0_f32; 80];
        let now = StreamInstant::ZERO + Duration::from_secs(1);
        assert!(!device.drained_at(now), "queued PCM has not been consumed");

        render_output(
            &mut buffer,
            super::RenderContext {
                pcm_reader: &mut consumer,
                gain: 1.0,
                analyzer: &mut analyzer,
                progress: &device.progress,
                timing: OutputTiming {
                    format: AudioFormat {
                        sample_rate: 8_000,
                        channels: 1,
                    },
                    backend_delay: Duration::from_millis(40),
                },
                end_of_stream: &end_of_stream,
                metrics: &metrics,
                spectrum: &spectrum,
            },
        );

        assert!(consumer.is_empty());
        assert!(
            !device.drained_at(now),
            "an empty ring is not a drained device"
        );
        assert!(!device.drained_at(now + Duration::from_micros(49_999)));
        assert!(device.drained_at(now + Duration::from_millis(50)));

        device.progress.playing.store(false, Ordering::Release);
        assert!(
            !device.drained_at(now + Duration::from_secs(5)),
            "paused output cannot finish"
        );
        device.progress.playing.store(true, Ordering::Release);
        let resumed = now + Duration::from_secs(10);
        assert!(
            !device.drained_at(resumed),
            "resuming restarts the drain deadline"
        );
        assert!(!device.drained_at(resumed + Duration::from_micros(49_999)));
        assert!(device.drained_at(resumed + Duration::from_millis(50)));
    }

    #[test]
    fn recoverable_cpal_errors_are_counted_without_stopping_the_stream() {
        let metrics = AudioMetrics::default();
        let failure = AtomicU8::new(0);

        record_stream_error(&Error::from(ErrorKind::Xrun), &failure, &metrics);
        record_stream_error(&Error::from(ErrorKind::RealtimeDenied), &failure, &metrics);
        record_stream_error(&Error::from(ErrorKind::DeviceChanged), &failure, &metrics);

        assert_eq!(failure.load(Ordering::Relaxed), 0);
        assert_eq!(metrics.xruns.load(Ordering::Relaxed), 1);
        assert_eq!(metrics.realtime_denied.load(Ordering::Relaxed), 1);
        assert_eq!(metrics.device_changes.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn fatal_cpal_errors_still_fail_the_stream() {
        for kind in [ErrorKind::DeviceNotAvailable, ErrorKind::StreamInvalidated] {
            let metrics = AudioMetrics::default();
            let failure = AtomicU8::new(0);

            record_stream_error(&Error::from(kind), &failure, &metrics);

            assert_eq!(failure.load(Ordering::Relaxed), 1, "{kind:?} must be fatal");
        }
    }
}
