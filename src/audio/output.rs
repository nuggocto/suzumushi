// SPDX-License-Identifier: Apache-2.0

//! CPAL output fed by one realtime-safe SPSC ring.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    Device, FromSample, I24, OutputCallbackInfo, SampleFormat, SizedSample, Stream, StreamConfig,
    U24,
};
use rtrb::{Consumer, Producer, RingBuffer};

use super::{AudioFormat, OutputStream};

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
    consumed: Arc<AtomicU64>,
    failure: Arc<AtomicU8>,
    written: u64,
}

impl CpalOutput {
    pub(super) fn new() -> Self {
        Self {
            source: None,
            output_channels: 0,
            stream: None,
            producer: None,
            consumed: Arc::new(AtomicU64::new(0)),
            failure: Arc::new(AtomicU8::new(0)),
            written: 0,
        }
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
        let range = device
            .supported_output_configs()
            .map_err(|error| format!("cannot query the default output device: {error}"))?
            .filter(|range| {
                range.contains_rate(source.sample_rate)
                    && range.channels() > 0
                    && range.channels() <= MAX_OUTPUT_CHANNELS
                    && is_pcm_format(range.sample_format())
            })
            .max_by(cpal::SupportedStreamConfigRange::cmp_default_heuristics)
            .ok_or_else(|| {
                format!(
                    "the default output device has no supported PCM configuration at {} Hz",
                    source.sample_rate
                )
            })?;
        let supported = range.with_sample_rate(source.sample_rate);
        self.output_channels = usize::from(supported.channels());
        let sample_format = supported.sample_format();
        let config = supported.config();
        let (producer, pcm_reader) = RingBuffer::new(PCM_RING_SAMPLES);
        let frames_read = Arc::clone(&self.consumed);
        let failure = Arc::clone(&self.failure);
        let stream = build_stream(
            &device,
            &config,
            sample_format,
            pcm_reader,
            frames_read,
            failure,
        )?;
        self.source = Some(source);
        self.producer = Some(producer);
        self.stream = Some(stream);
        Ok(())
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
                        .push(finite_or_silence((frame[0] + frame[1]) * 0.5))
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
        Ok(frames * source_channels)
    }

    fn play(&mut self) -> Result<(), String> {
        self.stream
            .as_ref()
            .ok_or_else(|| "audio output stream is unavailable".to_owned())?
            .play()
            .map_err(|error| format!("cannot start the audio output stream: {error}"))
    }

    fn pause(&mut self) -> Result<(), String> {
        self.stream
            .as_ref()
            .ok_or_else(|| "audio output stream is unavailable".to_owned())?
            .pause()
            .map_err(|error| format!("cannot pause the audio output stream: {error}"))
    }

    fn resume(&mut self) -> Result<(), String> {
        self.play()
    }

    fn stop(&mut self) -> Result<(), String> {
        let pause_error = self
            .stream
            .as_ref()
            .and_then(|stream| stream.pause().err())
            .map(|error| format!("cannot stop the audio output stream: {error}"));
        self.stream.take();
        self.producer.take();
        self.source = None;
        pause_error.map_or(Ok(()), Err)
    }

    fn drained(&self) -> bool {
        self.consumed.load(Ordering::Acquire) >= self.written
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
    frames_read: Arc<AtomicU64>,
    failure: Arc<AtomicU8>,
) -> Result<Stream, String> {
    match format {
        SampleFormat::I8 => typed_stream::<i8>(device, config, pcm_reader, frames_read, failure),
        SampleFormat::I16 => typed_stream::<i16>(device, config, pcm_reader, frames_read, failure),
        SampleFormat::I24 => typed_stream::<I24>(device, config, pcm_reader, frames_read, failure),
        SampleFormat::I32 => typed_stream::<i32>(device, config, pcm_reader, frames_read, failure),
        SampleFormat::I64 => typed_stream::<i64>(device, config, pcm_reader, frames_read, failure),
        SampleFormat::U8 => typed_stream::<u8>(device, config, pcm_reader, frames_read, failure),
        SampleFormat::U16 => typed_stream::<u16>(device, config, pcm_reader, frames_read, failure),
        SampleFormat::U24 => typed_stream::<U24>(device, config, pcm_reader, frames_read, failure),
        SampleFormat::U32 => typed_stream::<u32>(device, config, pcm_reader, frames_read, failure),
        SampleFormat::U64 => typed_stream::<u64>(device, config, pcm_reader, frames_read, failure),
        SampleFormat::F32 => typed_stream::<f32>(device, config, pcm_reader, frames_read, failure),
        SampleFormat::F64 => typed_stream::<f64>(device, config, pcm_reader, frames_read, failure),
        _ => Err(format!("unsupported output sample format {format}")),
    }
}

fn typed_stream<T>(
    device: &Device,
    config: &StreamConfig,
    mut pcm_reader: Consumer<f32>,
    frames_read: Arc<AtomicU64>,
    failure: Arc<AtomicU8>,
) -> Result<Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    device
        .build_output_stream(
            *config,
            move |output: &mut [T], _: &OutputCallbackInfo| {
                let mut read = 0_u64;
                for sample in output {
                    if let Ok(value) = pcm_reader.pop() {
                        *sample = T::from_sample(value);
                        read += 1;
                    } else {
                        *sample = T::from_sample(0.0);
                    }
                }
                frames_read.fetch_add(read, Ordering::Release);
            },
            move |_| {
                failure.store(1, Ordering::Release);
            },
            None,
        )
        .map_err(|error| format!("cannot build the audio output stream: {error}"))
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
