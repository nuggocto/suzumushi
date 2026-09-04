// SPDX-License-Identifier: Apache-2.0

//! Owned decoding, output, and playback worker.

mod decoder;
mod file;
mod output;
mod spectrum;

use std::fs::File;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender, TryRecvError, channel, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::errors::{AppError, AppResult};

pub(crate) use file::open_verified_media;
use spectrum::SpectrumLane;
pub(crate) use spectrum::{AudioSpectrum, SPECTRUM_BANDS};

/// Runs the isolated decoder entry point selected by the hidden CLI command.
#[must_use]
pub fn decoder_helper_main(position_micros: u64) -> i32 {
    decoder::helper_main(position_micros)
}

/// Exercises the bounded decoder adapter for fuzzing.
pub fn fuzz_decode(input: &[u8]) {
    decoder::fuzz_decode(input);
}

const COMMAND_CAPACITY: usize = 32;
const WORKER_POLL: Duration = Duration::from_millis(5);
const POSITION_INTERVAL: Duration = Duration::from_millis(250);

/// Match the decoder protocol's whole-microsecond position representation.
pub(crate) fn decoder_position(position: Duration) -> Duration {
    Duration::from_micros(u64::try_from(position.as_micros()).unwrap_or(u64::MAX))
}

/// Counters that can be read without touching the audio callback's control path.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct AudioDiagnostics {
    pub(crate) underrun_samples: u64,
    pub(crate) xruns: u64,
    pub(crate) realtime_denied: u64,
    pub(crate) device_changes: u64,
}

/// The counters behind [`AudioDiagnostics`], written by the audio callback and
/// the CPAL error callback, and read by the terminal loop.
///
/// These accumulate for the whole life of the runtime and must never be reset
/// when a stream is prepared, unlike the per-stream `consumed`, `written_samples`
/// and `failure` cells that sit beside them in `CpalOutput`. The terminal reports
/// them as deltas against its own last snapshot, so zeroing one here would make
/// that snapshot larger than the counter it is subtracted from.
#[derive(Default)]
pub(crate) struct AudioMetrics {
    pub(crate) underrun_samples: AtomicU64,
    pub(crate) xruns: AtomicU64,
    pub(crate) realtime_denied: AtomicU64,
    pub(crate) device_changes: AtomicU64,
}

impl AudioMetrics {
    fn snapshot(&self) -> AudioDiagnostics {
        AudioDiagnostics {
            underrun_samples: self.underrun_samples.load(Ordering::Relaxed),
            xruns: self.xruns.load(Ordering::Relaxed),
            realtime_denied: self.realtime_denied.load(Ordering::Relaxed),
            device_changes: self.device_changes.load(Ordering::Relaxed),
        }
    }
}

/// One decoded PCM format accepted by the current pipeline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioFormat {
    pub sample_rate: u32,
    pub channels: u16,
}

/// App-owned playback settings applied to each newly opened track.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlaybackSettings {
    pub volume_percent: u8,
    pub muted: bool,
}

impl Default for PlaybackSettings {
    fn default() -> Self {
        Self {
            volume_percent: 100,
            muted: false,
        }
    }
}

/// Latest coalesced source position for one playback generation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioPosition {
    pub generation: u64,
    pub timeline_revision: u64,
    pub position: Duration,
    pub duration: Option<Duration>,
}

/// Reliable commands sent by the app-owned terminal loop.
#[derive(Debug)]
pub enum AudioCommand {
    Play {
        generation: u64,
        file: File,
        position: Duration,
        settings: PlaybackSettings,
        paused: bool,
    },
    /// Supersedes previous playback even when opening the replacement failed.
    LoadFailed {
        generation: u64,
        message: String,
    },
    Pause {
        generation: u64,
    },
    Resume {
        generation: u64,
    },
    Stop {
        generation: u64,
    },
    SetGain {
        generation: u64,
        volume_percent: u8,
        muted: bool,
    },
    Seek {
        generation: u64,
        position: Duration,
    },
    Shutdown,
}

/// Reliable state changes returned to the app-owned terminal loop.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AudioEvent {
    Started {
        generation: u64,
        timeline_revision: u64,
        format: AudioFormat,
        duration: Option<Duration>,
        position: Duration,
    },
    Paused {
        generation: u64,
    },
    Resumed {
        generation: u64,
    },
    Stopped {
        generation: u64,
    },
    Seeked {
        generation: u64,
        timeline_revision: u64,
        position: Duration,
    },
    Finished {
        generation: u64,
    },
    Failed {
        generation: u64,
        message: String,
    },
}

/// Owns the audio worker and awaits it on every shutdown path.
pub struct AudioRuntime {
    commands: Option<SyncSender<AudioCommand>>,
    seeks: Option<Arc<SeekLane>>,
    events: Option<Receiver<AudioEvent>>,
    positions: Option<Arc<PositionLane>>,
    spectrum: Option<Arc<SpectrumLane>>,
    metrics: Arc<AudioMetrics>,
    worker: Option<JoinHandle<AppResult<()>>>,
}

impl AudioRuntime {
    /// Starts the idle worker without opening an audio device.
    ///
    /// # Errors
    ///
    /// Returns an audio error when the owned worker thread cannot start.
    pub fn start() -> AppResult<Self> {
        let (command_tx, command_rx) = sync_channel(COMMAND_CAPACITY);
        // State events must not backpressure the worker that keeps the PCM ring fed.
        let (event_tx, event_rx) = channel();
        let seeks = Arc::new(SeekLane::default());
        let worker_seeks = Arc::clone(&seeks);
        let positions = Arc::new(PositionLane::default());
        let worker_positions = Arc::clone(&positions);
        let spectrum = Arc::new(SpectrumLane::default());
        let worker_spectrum = Arc::clone(&spectrum);
        let metrics = Arc::new(AudioMetrics::default());
        let worker_metrics = Arc::clone(&metrics);
        let worker = thread::Builder::new()
            .name("suzumushi-audio".into())
            .spawn(move || {
                let result = worker_main(
                    &command_rx,
                    &worker_seeks,
                    &event_tx,
                    &worker_positions,
                    ProductionBackend {
                        spectrum: worker_spectrum,
                        metrics: worker_metrics,
                    },
                );
                worker_seeks.close();
                result
            })
            .map_err(|error| AppError::Audio(format!("cannot start worker: {error}")))?;
        Ok(Self {
            commands: Some(command_tx),
            seeks: Some(seeks),
            events: Some(event_rx),
            positions: Some(positions),
            spectrum: Some(spectrum),
            metrics,
            worker: Some(worker),
        })
    }

    /// Sends one playback command without queueing obsolete absolute seeks.
    ///
    /// # Errors
    ///
    /// Returns an audio error when the worker has stopped.
    pub fn send(&self, command: AudioCommand) -> AppResult<()> {
        let commands = self
            .commands
            .as_ref()
            .ok_or_else(|| AppError::Audio("worker is already stopped".into()))?;
        match command {
            AudioCommand::Seek {
                generation,
                position,
            } => {
                let accepted = self
                    .seeks
                    .as_ref()
                    .ok_or_else(|| AppError::Audio("worker seek lane is already closed".into()))?
                    .replace(SeekRequest {
                        generation,
                        position,
                    });
                if accepted {
                    Ok(())
                } else {
                    Err(AppError::Audio("worker seek lane disconnected".into()))
                }
            }
            command => commands
                .send(command)
                .map_err(|_| AppError::Audio("worker command lane disconnected".into())),
        }
    }

    /// Returns the next pending state change without blocking the terminal.
    ///
    /// # Errors
    ///
    /// Returns an audio error when the worker event lane has disconnected.
    pub fn try_event(&self) -> AppResult<Option<AudioEvent>> {
        match self
            .events
            .as_ref()
            .ok_or_else(|| AppError::Audio("worker event lane is already closed".into()))?
            .try_recv()
        {
            Ok(event) => Ok(Some(event)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => {
                Err(AppError::Audio("worker event lane disconnected".into()))
            }
        }
    }

    /// Returns the newest available coalesced playback position without blocking.
    ///
    /// # Errors
    ///
    /// Returns an audio error when the position lane has disconnected.
    pub fn try_position(&self) -> AppResult<Option<AudioPosition>> {
        Ok(self
            .positions
            .as_ref()
            .ok_or_else(|| AppError::Audio("worker position lane is already closed".into()))?
            .take())
    }

    /// Returns the newest lock-free spectrum snapshot without blocking.
    pub(crate) fn spectrum(&self) -> AudioSpectrum {
        self.spectrum
            .as_ref()
            .map_or_else(AudioSpectrum::default, |spectrum| spectrum.latest())
    }

    /// Returns callback and backend diagnostics without waiting for the worker.
    pub(crate) fn diagnostics(&self) -> AudioDiagnostics {
        self.metrics.snapshot()
    }

    /// Cancels active playback and awaits every owned thread and helper.
    ///
    /// # Errors
    ///
    /// Returns the worker's cleanup failure or panic.
    pub fn shutdown(mut self) -> AppResult<()> {
        self.stop()
    }

    fn stop(&mut self) -> AppResult<()> {
        if let Some(seeks) = self.seeks.take() {
            seeks.clear();
        }
        if let Some(commands) = self.commands.take() {
            let _ = commands.try_send(AudioCommand::Shutdown);
            drop(commands);
        }
        self.events.take();
        self.positions.take();
        self.spectrum.take();
        let Some(worker) = self.worker.take() else {
            return Ok(());
        };
        worker
            .join()
            .map_err(|_| AppError::Audio("worker panicked during shutdown".into()))?
    }
}

#[derive(Default)]
struct PositionLane {
    latest: Mutex<Option<AudioPosition>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SeekRequest {
    generation: u64,
    position: Duration,
}

#[derive(Default)]
struct SeekLane {
    state: Mutex<SeekLaneState>,
}

#[derive(Default)]
struct SeekLaneState {
    latest: Option<SeekRequest>,
    closed: bool,
}

impl SeekLane {
    fn replace(&self, request: SeekRequest) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.closed {
            return false;
        }
        state.latest = Some(request);
        true
    }

    fn take(&self) -> Option<SeekRequest> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .latest
            .take()
    }

    fn clear(&self) {
        let _ = self.take();
    }

    fn close(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.latest = None;
        state.closed = true;
    }
}

impl PositionLane {
    fn replace(&self, position: AudioPosition) {
        *self
            .latest
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(position);
    }

    fn take(&self) -> Option<AudioPosition> {
        self.latest
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }
}

impl Drop for AudioRuntime {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[derive(Debug)]
enum DecoderPoll {
    Pending,
    Ready(DecodedInfo),
    Samples(Vec<f32>),
    End,
    Failed(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DecodedInfo {
    format: AudioFormat,
    duration: Option<Duration>,
    position: Duration,
}

trait DecoderStream: Send {
    fn poll(&mut self) -> DecoderPoll;
    fn shutdown(&mut self) -> Result<(), String>;
}

trait OutputStream: Send {
    fn prepare(&mut self, source: AudioFormat) -> Result<(), String>;
    fn set_gain(&mut self, volume_percent: u8, muted: bool);
    fn write(&mut self, samples: &[f32]) -> Result<usize, String>;
    fn play(&mut self) -> Result<(), String>;
    fn pause(&mut self) -> Result<(), String>;
    fn resume(&mut self) -> Result<(), String>;
    fn stop(&mut self) -> Result<(), String>;
    fn consumed_frames(&self) -> u64;
    fn drained(&self) -> bool;
    fn failure(&self) -> Option<String>;
}

type PlaybackParts = (Box<dyn DecoderStream>, Box<dyn OutputStream>);

trait Backend: Send {
    fn open(&mut self, file: &File, position: Duration) -> Result<PlaybackParts, String>;
}

struct ProductionBackend {
    spectrum: Arc<SpectrumLane>,
    metrics: Arc<AudioMetrics>,
}

impl Backend for ProductionBackend {
    fn open(&mut self, file: &File, position: Duration) -> Result<PlaybackParts, String> {
        Ok((
            Box::new(decoder::HelperDecoder::start(file, position)?),
            Box::new(output::CpalOutput::with_metrics(
                Arc::clone(&self.spectrum),
                Arc::clone(&self.metrics),
            )),
        ))
    }
}

#[derive(Clone, Copy, Debug)]
enum StartNotice {
    Started,
    Seeked,
}

struct ActivePlayback {
    generation: u64,
    timeline_revision: u64,
    file: File,
    decoder: Box<dyn DecoderStream>,
    output: Box<dyn OutputStream>,
    info: Option<DecodedInfo>,
    settings: PlaybackSettings,
    pending: Option<(Vec<f32>, usize)>,
    started: bool,
    paused: bool,
    ended: bool,
    base_position: Duration,
    last_reported_position: Duration,
    start_notice: StartNotice,
}

struct WorkerCore<B: Backend> {
    backend: B,
    active: Option<ActivePlayback>,
}

impl<B: Backend> WorkerCore<B> {
    fn new(backend: B) -> Self {
        Self {
            backend,
            active: None,
        }
    }

    fn command(
        &mut self,
        command: AudioCommand,
        events: &Sender<AudioEvent>,
        positions: &PositionLane,
    ) -> Result<bool, String> {
        match command {
            AudioCommand::Play {
                generation,
                file,
                position,
                settings,
                paused,
            } => self.play(generation, file, position, settings, paused, events),
            AudioCommand::LoadFailed {
                generation,
                mut message,
            } => {
                if let Err(error) = self.stop_active() {
                    message.push_str("; playback cleanup failed: ");
                    message.push_str(&error);
                }
                emit(
                    events,
                    AudioEvent::Failed {
                        generation,
                        message,
                    },
                );
            }
            AudioCommand::Pause { generation } => {
                self.pause(generation, events, positions);
            }
            AudioCommand::Resume { generation } => self.resume(generation, events),
            AudioCommand::Stop { generation } => self.stop(generation, events),
            AudioCommand::SetGain {
                generation,
                volume_percent,
                muted,
            } => {
                if let Some(active) = self.matching_active(generation) {
                    active.settings.volume_percent = volume_percent.min(100);
                    active.settings.muted = muted;
                    active
                        .output
                        .set_gain(active.settings.volume_percent, active.settings.muted);
                }
            }
            AudioCommand::Seek {
                generation,
                position,
            } => self.seek(generation, position, events, positions),
            AudioCommand::Shutdown => {
                self.stop_active()?;
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn play(
        &mut self,
        generation: u64,
        file: File,
        position: Duration,
        mut settings: PlaybackSettings,
        paused: bool,
        events: &Sender<AudioEvent>,
    ) {
        settings.volume_percent = settings.volume_percent.min(100);
        if let Err(message) = self.stop_active() {
            emit(
                events,
                AudioEvent::Failed {
                    generation,
                    message,
                },
            );
            return;
        }
        match self.backend.open(&file, position) {
            Ok((decoder, mut output)) => {
                output.set_gain(settings.volume_percent, settings.muted);
                self.active = Some(ActivePlayback {
                    generation,
                    timeline_revision: 1,
                    file,
                    decoder,
                    output,
                    info: None,
                    settings,
                    pending: None,
                    started: false,
                    paused,
                    ended: false,
                    base_position: position,
                    last_reported_position: position,
                    start_notice: StartNotice::Started,
                });
            }
            Err(message) => emit(
                events,
                AudioEvent::Failed {
                    generation,
                    message,
                },
            ),
        }
    }

    fn pause(&mut self, generation: u64, events: &Sender<AudioEvent>, positions: &PositionLane) {
        if let Some(active) = self.matching_active(generation)
            && !active.paused
        {
            let result = if active.started {
                active.output.pause()
            } else {
                Ok(())
            };
            match result {
                Ok(()) => {
                    active.paused = true;
                    emit(events, AudioEvent::Paused { generation });
                }
                Err(message) => self.fail(events, generation, &message),
            }
        }
        self.publish_position(positions, true);
    }

    fn resume(&mut self, generation: u64, events: &Sender<AudioEvent>) {
        if let Some(active) = self.matching_active(generation)
            && active.paused
        {
            let result = if active.started {
                active.output.resume()
            } else {
                Ok(())
            };
            match result {
                Ok(()) => {
                    active.paused = false;
                    emit(events, AudioEvent::Resumed { generation });
                }
                Err(message) => self.fail(events, generation, &message),
            }
        }
    }

    fn stop(&mut self, generation: u64, events: &Sender<AudioEvent>) {
        if self
            .active
            .as_ref()
            .is_some_and(|active| active.generation == generation)
        {
            match self.stop_active() {
                Ok(()) => emit(events, AudioEvent::Stopped { generation }),
                Err(message) => emit(
                    events,
                    AudioEvent::Failed {
                        generation,
                        message,
                    },
                ),
            }
        }
    }

    fn seek(
        &mut self,
        generation: u64,
        position: Duration,
        events: &Sender<AudioEvent>,
        positions: &PositionLane,
    ) {
        let timing = self
            .active
            .as_ref()
            .filter(|active| active.generation == generation)
            .map(|active| {
                (
                    active.info.and_then(|info| info.duration),
                    active.timeline_revision,
                )
            });
        let (duration, timeline_revision) = timing.unwrap_or((None, 0));
        let position = duration.map_or(position, |duration| position.min(duration));
        if duration.is_some_and(|duration| position >= duration) {
            Self::publish_specific_position(
                positions,
                generation,
                timeline_revision,
                position,
                duration,
            );
            match self.stop_active() {
                Ok(()) => emit(events, AudioEvent::Finished { generation }),
                Err(message) => emit(
                    events,
                    AudioEvent::Failed {
                        generation,
                        message,
                    },
                ),
            }
        } else if self
            .active
            .as_ref()
            .is_some_and(|active| active.generation == generation)
            && let Err(message) = self.restart_active(generation, position)
        {
            emit(
                events,
                AudioEvent::Failed {
                    generation,
                    message,
                },
            );
        }
    }

    fn drive(&mut self, events: &Sender<AudioEvent>, positions: &PositionLane) -> bool {
        let Some(active) = self.active.as_ref() else {
            return false;
        };
        let generation = active.generation;
        if self.output_failed(events, generation) {
            return true;
        }

        self.publish_position(positions, false);
        self.write_pending(events, generation)
            || self.poll_decoder(events, generation)
            || self.finish_drained(events, positions, generation)
    }

    fn write_pending(&mut self, events: &Sender<AudioEvent>, generation: u64) -> bool {
        let active = self.active.as_mut().expect("active playback is retained");
        let Some((samples, mut offset)) = active.pending.take() else {
            return false;
        };
        match active.output.write(&samples[offset..]) {
            Ok(0) => {
                active.pending = Some((samples, offset));
                false
            }
            Ok(written) => {
                offset += written;
                if !active.started {
                    if !active.paused
                        && let Err(message) = active.output.play()
                    {
                        self.fail(events, generation, &message);
                        return true;
                    }
                    active.started = true;
                    emit_started(active, events);
                }
                if offset < samples.len() {
                    active.pending = Some((samples, offset));
                }
                true
            }
            Err(message) => {
                self.fail(events, generation, &message);
                true
            }
        }
    }

    fn poll_decoder(&mut self, events: &Sender<AudioEvent>, generation: u64) -> bool {
        let active = self.active.as_mut().expect("active playback is retained");
        if active.pending.is_some() || active.ended {
            return false;
        }
        match active.decoder.poll() {
            DecoderPoll::Pending => false,
            DecoderPoll::Ready(info) => {
                self.decoder_ready(events, generation, info);
                true
            }
            DecoderPoll::Samples(samples) => {
                self.decoder_samples(events, generation, samples);
                true
            }
            DecoderPoll::End => {
                active.ended = true;
                true
            }
            DecoderPoll::Failed(message) => {
                self.fail(events, generation, &message);
                true
            }
        }
    }

    fn decoder_ready(&mut self, events: &Sender<AudioEvent>, generation: u64, info: DecodedInfo) {
        let active = self.active.as_mut().expect("active playback is retained");
        if active.info.is_some() {
            self.fail(events, generation, "decoder sent two format headers");
        } else if let Err(message) = active.output.prepare(info.format) {
            self.fail(events, generation, &message);
        } else {
            active
                .output
                .set_gain(active.settings.volume_percent, active.settings.muted);
            active.base_position = info.position;
            active.last_reported_position = info.position;
            active.info = Some(info);
        }
    }

    fn decoder_samples(&mut self, events: &Sender<AudioEvent>, generation: u64, samples: Vec<f32>) {
        let active = self.active.as_mut().expect("active playback is retained");
        let invalid = if active.info.is_none() {
            Some("decoder sent samples before its format")
        } else if samples.is_empty() {
            Some("decoder sent an empty PCM block")
        } else if samples.iter().any(|sample| !sample.is_finite()) {
            Some("decoder sent a non-finite PCM sample")
        } else {
            None
        };
        if let Some(message) = invalid {
            self.fail(events, generation, message);
        } else {
            active.pending = Some((samples, 0));
        }
    }

    fn finish_drained(
        &mut self,
        events: &Sender<AudioEvent>,
        positions: &PositionLane,
        generation: u64,
    ) -> bool {
        let active = self.active.as_ref().expect("active playback is retained");
        if !active.ended || active.pending.is_some() {
            return false;
        }
        if !active.started {
            self.fail(
                events,
                generation,
                "track contained no decodable audio samples",
            );
            return true;
        }
        if !active.output.drained() {
            return false;
        }
        let final_position = active
            .info
            .and_then(|info| info.duration)
            .unwrap_or_else(|| position_for(active));
        let duration = active.info.and_then(|info| info.duration);
        Self::publish_specific_position(
            positions,
            generation,
            active.timeline_revision,
            final_position,
            duration,
        );
        match self.stop_active() {
            Ok(()) => emit(events, AudioEvent::Finished { generation }),
            Err(message) => emit(
                events,
                AudioEvent::Failed {
                    generation,
                    message,
                },
            ),
        }
        true
    }

    fn restart_active(&mut self, generation: u64, position: Duration) -> Result<(), String> {
        let Some(mut old) = self.active.take() else {
            return Ok(());
        };
        if old.generation != generation {
            self.active = Some(old);
            return Ok(());
        }
        let output_error = old.output.stop().err();
        let decoder_error = old.decoder.shutdown().err();
        let timeline_revision = old
            .timeline_revision
            .checked_add(1)
            .ok_or_else(|| "audio timeline revision exhausted".to_owned())?;
        let paused = old.paused;
        let file = old.file;
        if let Some(message) = combine_cleanup_errors(output_error, decoder_error) {
            return Err(message);
        }
        let (decoder, mut output) = self.backend.open(&file, position)?;
        output.set_gain(old.settings.volume_percent, old.settings.muted);
        self.active = Some(ActivePlayback {
            generation,
            timeline_revision,
            file,
            decoder,
            output,
            info: None,
            settings: old.settings,
            pending: None,
            started: false,
            paused,
            ended: false,
            base_position: position,
            last_reported_position: position,
            start_notice: StartNotice::Seeked,
        });
        Ok(())
    }

    fn publish_position(&mut self, positions: &PositionLane, force: bool) {
        let Some(active) = self.active.as_mut() else {
            return;
        };
        let position = position_for(active);
        let changed = position
            .checked_sub(active.last_reported_position)
            .is_some_and(|delta| delta >= POSITION_INTERVAL)
            || position < active.last_reported_position;
        if !force && !changed {
            return;
        }
        let update = AudioPosition {
            generation: active.generation,
            timeline_revision: active.timeline_revision,
            position,
            duration: active.info.and_then(|info| info.duration),
        };
        positions.replace(update);
        active.last_reported_position = position;
    }

    fn publish_specific_position(
        positions: &PositionLane,
        generation: u64,
        timeline_revision: u64,
        position: Duration,
        duration: Option<Duration>,
    ) {
        positions.replace(AudioPosition {
            generation,
            timeline_revision,
            position,
            duration,
        });
    }

    fn output_failed(&mut self, events: &Sender<AudioEvent>, generation: u64) -> bool {
        let message = self
            .active
            .as_ref()
            .and_then(|active| active.output.failure());
        if let Some(message) = message {
            self.fail(events, generation, &message);
            true
        } else {
            false
        }
    }

    fn matching_active(&mut self, generation: u64) -> Option<&mut ActivePlayback> {
        self.active
            .as_mut()
            .filter(|active| active.generation == generation)
    }

    fn fail(&mut self, events: &Sender<AudioEvent>, generation: u64, message: &str) {
        let cleanup = self.stop_active().err();
        let message = cleanup.map_or_else(
            || message.to_owned(),
            |cleanup| format!("{message}; {cleanup}"),
        );
        emit(
            events,
            AudioEvent::Failed {
                generation,
                message,
            },
        );
    }

    fn stop_active(&mut self) -> Result<(), String> {
        let Some(mut active) = self.active.take() else {
            return Ok(());
        };
        let output_error = active.output.stop().err();
        let decoder_error = active.decoder.shutdown().err();
        combine_cleanup_errors(output_error, decoder_error).map_or(Ok(()), Err)
    }
}

fn emit_started(active: &mut ActivePlayback, events: &Sender<AudioEvent>) {
    let info = active
        .info
        .expect("samples require prepared decoder information");
    let generation = active.generation;
    let timeline_revision = active.timeline_revision;
    let event = match active.start_notice {
        StartNotice::Started => AudioEvent::Started {
            generation,
            timeline_revision,
            format: info.format,
            duration: info.duration,
            position: info.position,
        },
        StartNotice::Seeked => AudioEvent::Seeked {
            generation,
            timeline_revision,
            position: info.position,
        },
    };
    emit(events, event);
}

fn combine_cleanup_errors(left: Option<String>, right: Option<String>) -> Option<String> {
    match (left, right) {
        (None, None) => None,
        (Some(error), None) | (None, Some(error)) => Some(error),
        (Some(left), Some(right)) => Some(format!("{left}; {right}")),
    }
}

fn position_for(active: &ActivePlayback) -> Duration {
    let Some(info) = active.info else {
        return active.base_position;
    };
    let frames = u128::from(active.output.consumed_frames());
    let denominator = u128::from(info.format.sample_rate);
    let nanos = frames
        .saturating_mul(1_000_000_000)
        .checked_div(denominator)
        .unwrap_or(0)
        .min(u128::from(u64::MAX));
    let position = active.base_position.saturating_add(Duration::from_nanos(
        u64::try_from(nanos).expect("source position was clamped to u64"),
    ));
    info.duration
        .map_or(position, |duration| position.min(duration))
}

fn worker_main<B: Backend>(
    commands: &Receiver<AudioCommand>,
    seeks: &SeekLane,
    events: &Sender<AudioEvent>,
    positions: &PositionLane,
    backend: B,
) -> AppResult<()> {
    let mut core = WorkerCore::new(backend);
    loop {
        match commands.try_recv() {
            Ok(command) => {
                if core
                    .command(command, events, positions)
                    .map_err(AppError::Audio)?
                {
                    return Ok(());
                }
                continue;
            }
            Err(TryRecvError::Disconnected) => {
                core.stop_active().map_err(AppError::Audio)?;
                return Ok(());
            }
            Err(TryRecvError::Empty) => {}
        }
        if let Some(request) = seeks.take() {
            core.seek(request.generation, request.position, events, positions);
            continue;
        }
        if core.drive(events, positions) {
            continue;
        }
        match commands.recv_timeout(WORKER_POLL) {
            Ok(command) => {
                if core
                    .command(command, events, positions)
                    .map_err(AppError::Audio)?
                {
                    return Ok(());
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                core.stop_active().map_err(AppError::Audio)?;
                return Ok(());
            }
        }
    }
}

fn emit(events: &Sender<AudioEvent>, event: AudioEvent) {
    let _ = events.send(event);
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::fs::File;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::mpsc::TryRecvError;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::{
        AudioCommand, AudioEvent, AudioFormat, AudioMetrics, AudioPosition, AudioRuntime, Backend,
        DecoderPoll, DecoderStream, OutputStream, PlaybackParts, PlaybackSettings, PositionLane,
        SeekLane, SeekRequest, WorkerCore, channel, sync_channel,
    };

    #[derive(Default)]
    struct FakeDecoder {
        script: VecDeque<DecoderPoll>,
        stopped: Arc<AtomicBool>,
    }

    impl DecoderStream for FakeDecoder {
        fn poll(&mut self) -> DecoderPoll {
            self.script.pop_front().unwrap_or(DecoderPoll::Pending)
        }

        fn shutdown(&mut self) -> Result<(), String> {
            self.stopped.store(true, Ordering::Release);
            Ok(())
        }
    }

    struct FakeOutput {
        calls: Arc<Mutex<Vec<&'static str>>>,
        drained: Arc<AtomicBool>,
        consumed_frames: Arc<AtomicU64>,
        gains: Arc<Mutex<Vec<(u8, bool)>>>,
    }

    impl OutputStream for FakeOutput {
        fn prepare(&mut self, _source: AudioFormat) -> Result<(), String> {
            self.calls.lock().expect("fake output log").push("prepare");
            self.consumed_frames.store(0, Ordering::Release);
            Ok(())
        }

        fn set_gain(&mut self, volume_percent: u8, muted: bool) {
            self.gains
                .lock()
                .expect("fake gain log")
                .push((volume_percent, muted));
        }

        fn write(&mut self, samples: &[f32]) -> Result<usize, String> {
            self.calls.lock().expect("fake output log").push("write");
            Ok(samples.len())
        }

        fn play(&mut self) -> Result<(), String> {
            self.calls.lock().expect("fake output log").push("play");
            Ok(())
        }

        fn pause(&mut self) -> Result<(), String> {
            self.calls.lock().expect("fake output log").push("pause");
            Ok(())
        }

        fn resume(&mut self) -> Result<(), String> {
            self.calls.lock().expect("fake output log").push("resume");
            Ok(())
        }

        fn stop(&mut self) -> Result<(), String> {
            self.calls.lock().expect("fake output log").push("stop");
            Ok(())
        }

        fn consumed_frames(&self) -> u64 {
            self.consumed_frames.load(Ordering::Acquire)
        }

        fn drained(&self) -> bool {
            self.drained.load(Ordering::Acquire)
        }

        fn failure(&self) -> Option<String> {
            None
        }
    }

    struct FakeBackend {
        scripts: VecDeque<VecDeque<DecoderPoll>>,
        decoder_stopped: Arc<AtomicBool>,
        calls: Arc<Mutex<Vec<&'static str>>>,
        drained: Arc<AtomicBool>,
        consumed_frames: Arc<AtomicU64>,
        gains: Arc<Mutex<Vec<(u8, bool)>>>,
        opened_positions: Arc<Mutex<Vec<Duration>>>,
    }

    impl Backend for FakeBackend {
        fn open(&mut self, _file: &File, position: Duration) -> Result<PlaybackParts, String> {
            self.opened_positions
                .lock()
                .expect("fake open log")
                .push(position);
            Ok((
                Box::new(FakeDecoder {
                    script: self.scripts.pop_front().unwrap_or_default(),
                    stopped: Arc::clone(&self.decoder_stopped),
                }),
                Box::new(FakeOutput {
                    calls: Arc::clone(&self.calls),
                    drained: Arc::clone(&self.drained),
                    consumed_frames: Arc::clone(&self.consumed_frames),
                    gains: Arc::clone(&self.gains),
                }),
            ))
        }
    }

    fn fixture_backend(script: VecDeque<DecoderPoll>) -> (FakeBackend, FakeFixture) {
        let fixture = FakeFixture {
            decoder_stopped: Arc::new(AtomicBool::new(false)),
            calls: Arc::new(Mutex::new(Vec::new())),
            drained: Arc::new(AtomicBool::new(false)),
            consumed_frames: Arc::new(AtomicU64::new(0)),
            gains: Arc::new(Mutex::new(Vec::new())),
            opened_positions: Arc::new(Mutex::new(Vec::new())),
        };
        (
            FakeBackend {
                scripts: VecDeque::from([script]),
                decoder_stopped: Arc::clone(&fixture.decoder_stopped),
                calls: Arc::clone(&fixture.calls),
                drained: Arc::clone(&fixture.drained),
                consumed_frames: Arc::clone(&fixture.consumed_frames),
                gains: Arc::clone(&fixture.gains),
                opened_positions: Arc::clone(&fixture.opened_positions),
            },
            fixture,
        )
    }

    struct FakeFixture {
        decoder_stopped: Arc<AtomicBool>,
        calls: Arc<Mutex<Vec<&'static str>>>,
        drained: Arc<AtomicBool>,
        consumed_frames: Arc<AtomicU64>,
        gains: Arc<Mutex<Vec<(u8, bool)>>>,
        opened_positions: Arc<Mutex<Vec<Duration>>>,
    }

    fn harmless_file() -> File {
        File::open("/dev/null").expect("open harmless fake media")
    }

    fn decoded(format: AudioFormat) -> super::DecodedInfo {
        decoded_at(format, Duration::ZERO)
    }

    fn decoded_at(format: AudioFormat, position: Duration) -> super::DecodedInfo {
        super::DecodedInfo {
            format,
            duration: Some(Duration::from_secs(1)),
            position,
        }
    }

    fn drive_steps<B: Backend>(
        core: &mut WorkerCore<B>,
        events: &std::sync::mpsc::Sender<AudioEvent>,
        positions: &PositionLane,
        steps: usize,
    ) {
        for _ in 0..steps {
            core.drive(events, positions);
        }
    }

    #[test]
    fn failed_replacement_stops_the_previous_generation_before_reporting_failure() {
        let (backend, fixture) = fixture_backend(VecDeque::new());
        let mut core = WorkerCore::new(backend);
        let (events, received) = channel();
        let positions = PositionLane::default();
        core.command(
            AudioCommand::Play {
                generation: 1,
                file: harmless_file(),
                position: Duration::ZERO,
                settings: PlaybackSettings::default(),
                paused: false,
            },
            &events,
            &positions,
        )
        .expect("previous track");

        core.command(
            AudioCommand::LoadFailed {
                generation: 2,
                message: "replacement disappeared".into(),
            },
            &events,
            &positions,
        )
        .expect("failed replacement is handled");

        assert_eq!(
            received.try_recv().expect("failure"),
            AudioEvent::Failed {
                generation: 2,
                message: "replacement disappeared".into(),
            }
        );
        assert!(fixture.decoder_stopped.load(Ordering::Acquire));
        assert_eq!(*fixture.calls.lock().expect("output calls"), ["stop"]);
        assert!(core.active.is_none());
        assert!(received.try_recv().is_err(), "no obsolete stopped event");
    }

    #[test]
    fn position_lane_keeps_only_the_latest_update() {
        let positions = PositionLane::default();
        positions.replace(AudioPosition {
            generation: 1,
            timeline_revision: 1,
            position: Duration::from_secs(1),
            duration: Some(Duration::from_secs(3)),
        });
        positions.replace(AudioPosition {
            generation: 1,
            timeline_revision: 2,
            position: Duration::from_secs(2),
            duration: Some(Duration::from_secs(3)),
        });

        assert_eq!(
            positions.take(),
            Some(AudioPosition {
                generation: 1,
                timeline_revision: 2,
                position: Duration::from_secs(2),
                duration: Some(Duration::from_secs(3)),
            })
        );
        assert_eq!(positions.take(), None);
    }

    #[test]
    fn repeated_seeks_bypass_the_reliable_command_backlog() {
        let (command_tx, commands) = sync_channel(2);
        let seeks = Arc::new(SeekLane::default());
        let runtime = AudioRuntime {
            commands: Some(command_tx),
            seeks: Some(Arc::clone(&seeks)),
            events: None,
            positions: None,
            spectrum: None,
            metrics: Arc::new(AudioMetrics::default()),
            worker: None,
        };

        runtime
            .send(AudioCommand::Seek {
                generation: 4,
                position: Duration::from_secs(15),
            })
            .expect("first seek target");
        runtime
            .send(AudioCommand::Seek {
                generation: 4,
                position: Duration::from_secs(110),
            })
            .expect("latest seek target");

        assert!(matches!(commands.try_recv(), Err(TryRecvError::Empty)));
        assert_eq!(
            seeks.take(),
            Some(SeekRequest {
                generation: 4,
                position: Duration::from_secs(110),
            })
        );
        assert_eq!(seeks.take(), None);

        seeks.close();
        assert!(
            runtime
                .send(AudioCommand::Seek {
                    generation: 4,
                    position: Duration::from_secs(115),
                })
                .is_err()
        );
    }

    #[test]
    fn initial_play_opens_the_decoder_at_the_requested_resume_position() {
        let format = AudioFormat {
            sample_rate: 48_000,
            channels: 2,
        };
        let position = Duration::from_millis(750);
        let (backend, fixture) = fixture_backend(VecDeque::from([
            DecoderPoll::Ready(decoded_at(format, position)),
            DecoderPoll::Samples(vec![0.1, -0.1]),
        ]));
        let mut core = WorkerCore::new(backend);
        let (events, received) = channel();
        let positions = PositionLane::default();

        core.command(
            AudioCommand::Play {
                generation: 6,
                file: harmless_file(),
                position,
                settings: PlaybackSettings::default(),
                paused: false,
            },
            &events,
            &positions,
        )
        .expect("start resumed playback");
        drive_steps(&mut core, &events, &positions, 4);

        assert_eq!(
            fixture
                .opened_positions
                .lock()
                .expect("open position log")
                .as_slice(),
            [position]
        );
        assert!(matches!(
            received.try_recv(),
            Ok(AudioEvent::Started {
                generation: 6,
                position: started,
                ..
            }) if started == position
        ));
    }

    #[test]
    fn initially_paused_playback_waits_for_resume_before_starting_output() {
        let format = AudioFormat {
            sample_rate: 48_000,
            channels: 2,
        };
        let (backend, fixture) = fixture_backend(VecDeque::from([
            DecoderPoll::Ready(decoded(format)),
            DecoderPoll::Samples(vec![0.1, -0.1]),
        ]));
        let mut core = WorkerCore::new(backend);
        let (events, received) = channel();
        let positions = PositionLane::default();

        core.command(
            AudioCommand::Play {
                generation: 8,
                file: harmless_file(),
                position: Duration::ZERO,
                settings: PlaybackSettings::default(),
                paused: true,
            },
            &events,
            &positions,
        )
        .expect("load paused playback");
        drive_steps(&mut core, &events, &positions, 4);

        assert!(matches!(
            received.try_recv(),
            Ok(AudioEvent::Started { generation: 8, .. })
        ));
        assert_eq!(
            fixture.calls.lock().expect("fake output calls").as_slice(),
            ["prepare", "write"]
        );

        core.command(AudioCommand::Resume { generation: 8 }, &events, &positions)
            .expect("resume paused playback");
        assert_eq!(
            received.try_recv().expect("resumed event"),
            AudioEvent::Resumed { generation: 8 }
        );
        assert_eq!(
            fixture.calls.lock().expect("fake output calls").as_slice(),
            ["prepare", "write", "resume"]
        );
    }

    #[test]
    fn fake_device_observes_ordered_play_pause_resume_and_stop() {
        let format = AudioFormat {
            sample_rate: 48_000,
            channels: 2,
        };
        let (backend, fixture) = fixture_backend(VecDeque::from([
            DecoderPoll::Ready(decoded(format)),
            DecoderPoll::Samples(vec![0.1, -0.1, 0.2, -0.2]),
        ]));
        let mut core = WorkerCore::new(backend);
        let (events, received) = channel();
        let positions = PositionLane::default();

        core.command(
            AudioCommand::Play {
                generation: 7,
                file: harmless_file(),
                position: Duration::ZERO,
                settings: PlaybackSettings::default(),
                paused: false,
            },
            &events,
            &positions,
        )
        .expect("start fake playback");
        assert!(core.drive(&events, &positions));
        assert!(core.drive(&events, &positions));
        assert!(core.drive(&events, &positions));
        assert_eq!(
            received.try_recv().expect("started event"),
            AudioEvent::Started {
                generation: 7,
                timeline_revision: 1,
                format,
                duration: Some(Duration::from_secs(1)),
                position: Duration::ZERO,
            }
        );

        core.command(AudioCommand::Pause { generation: 7 }, &events, &positions)
            .expect("pause fake playback");
        core.command(AudioCommand::Resume { generation: 7 }, &events, &positions)
            .expect("resume fake playback");
        core.command(AudioCommand::Stop { generation: 7 }, &events, &positions)
            .expect("stop fake playback");

        assert_eq!(
            fixture.calls.lock().expect("fake output calls").as_slice(),
            ["prepare", "write", "play", "pause", "resume", "stop"]
        );
        assert!(fixture.decoder_stopped.load(Ordering::Acquire));
        assert_eq!(
            received.try_iter().collect::<Vec<_>>(),
            [
                AudioEvent::Paused { generation: 7 },
                AudioEvent::Resumed { generation: 7 },
                AudioEvent::Stopped { generation: 7 }
            ]
        );
    }

    #[test]
    fn completion_waits_for_the_fake_device_to_drain() {
        let format = AudioFormat {
            sample_rate: 44_100,
            channels: 1,
        };
        let (backend, fixture) = fixture_backend(VecDeque::from([
            DecoderPoll::Ready(decoded(format)),
            DecoderPoll::Samples(vec![0.1, 0.2]),
            DecoderPoll::End,
        ]));
        let mut core = WorkerCore::new(backend);
        let (events, received) = channel();
        let positions = PositionLane::default();
        core.command(
            AudioCommand::Play {
                generation: 9,
                file: harmless_file(),
                position: Duration::ZERO,
                settings: PlaybackSettings::default(),
                paused: false,
            },
            &events,
            &positions,
        )
        .expect("start fake playback");
        for _ in 0..5 {
            core.drive(&events, &positions);
        }
        assert_eq!(
            received.try_recv().expect("started event"),
            AudioEvent::Started {
                generation: 9,
                timeline_revision: 1,
                format,
                duration: Some(Duration::from_secs(1)),
                position: Duration::ZERO,
            }
        );
        assert!(received.try_recv().is_err(), "finish must wait for drain");

        fixture.drained.store(true, Ordering::Release);
        assert!(core.drive(&events, &positions));
        assert_eq!(
            received.try_recv().expect("finished event"),
            AudioEvent::Finished { generation: 9 }
        );
        assert!(fixture.decoder_stopped.load(Ordering::Acquire));
    }

    #[test]
    fn gain_changes_reach_the_fake_output() {
        let (backend, fixture) = fixture_backend(VecDeque::new());
        let mut core = WorkerCore::new(backend);
        let (events, _received) = channel();
        let positions = PositionLane::default();
        core.command(
            AudioCommand::Play {
                generation: 10,
                file: harmless_file(),
                position: Duration::ZERO,
                settings: PlaybackSettings::default(),
                paused: false,
            },
            &events,
            &positions,
        )
        .expect("start fake playback");

        core.command(
            AudioCommand::SetGain {
                generation: 10,
                volume_percent: 65,
                muted: true,
            },
            &events,
            &positions,
        )
        .expect("set fake gain");

        assert_eq!(
            fixture.gains.lock().expect("gain log").last(),
            Some(&(65, true))
        );
    }

    #[test]
    fn pause_and_resume_survive_a_seek_decoder_restart() {
        let format = AudioFormat {
            sample_rate: 48_000,
            channels: 1,
        };
        let first = VecDeque::from([
            DecoderPoll::Ready(decoded(format)),
            DecoderPoll::Samples(vec![0.1; 512]),
        ]);
        let (mut backend, fixture) = fixture_backend(first);
        backend.scripts.push_back(VecDeque::from([
            DecoderPoll::Ready(decoded_at(format, Duration::from_millis(750))),
            DecoderPoll::Samples(vec![0.1; 512]),
        ]));
        let mut core = WorkerCore::new(backend);
        let (events, received) = channel();
        let positions = PositionLane::default();
        core.command(
            AudioCommand::Play {
                generation: 11,
                file: harmless_file(),
                position: Duration::ZERO,
                settings: PlaybackSettings::default(),
                paused: false,
            },
            &events,
            &positions,
        )
        .expect("start fake playback");
        drive_steps(&mut core, &events, &positions, 6);
        assert!(matches!(
            received.try_recv(),
            Ok(AudioEvent::Started { generation: 11, .. })
        ));

        core.command(
            AudioCommand::Seek {
                generation: 11,
                position: Duration::from_millis(750),
            },
            &events,
            &positions,
        )
        .expect("restart at seek target");
        core.command(AudioCommand::Pause { generation: 11 }, &events, &positions)
            .expect("pause during restart");
        assert_eq!(
            received.try_recv().expect("paused event"),
            AudioEvent::Paused { generation: 11 }
        );
        drive_steps(&mut core, &events, &positions, 6);
        assert!(matches!(
            received.try_recv(),
            Ok(AudioEvent::Seeked {
                generation: 11,
                position,
                ..
            }) if position == Duration::from_millis(750)
        ));
        assert_eq!(
            fixture.calls.lock().expect("fake output calls").as_slice(),
            ["prepare", "write", "play", "stop", "prepare", "write"]
        );

        core.command(AudioCommand::Resume { generation: 11 }, &events, &positions)
            .expect("resume after restart");
        assert_eq!(
            received.try_recv().expect("resumed event"),
            AudioEvent::Resumed { generation: 11 }
        );
        assert_eq!(
            fixture.calls.lock().expect("fake output calls").as_slice(),
            [
                "prepare", "write", "play", "stop", "prepare", "write", "resume"
            ]
        );
    }

    #[test]
    fn seek_restarts_at_the_app_selected_source_position() {
        let format = AudioFormat {
            sample_rate: 48_000,
            channels: 1,
        };
        let first = VecDeque::from([
            DecoderPoll::Ready(decoded(format)),
            DecoderPoll::Samples(vec![0.1; 512]),
        ]);
        let (mut backend, fixture) = fixture_backend(first);
        backend.scripts.push_back(VecDeque::from([
            DecoderPoll::Ready(decoded_at(format, Duration::from_millis(750))),
            DecoderPoll::Samples(vec![0.1; 16_384]),
            DecoderPoll::End,
        ]));
        let mut core = WorkerCore::new(backend);
        let (events, received) = channel();
        let positions = PositionLane::default();
        core.command(
            AudioCommand::Play {
                generation: 11,
                file: harmless_file(),
                position: Duration::ZERO,
                settings: PlaybackSettings::default(),
                paused: false,
            },
            &events,
            &positions,
        )
        .expect("start fake playback");
        drive_steps(&mut core, &events, &positions, 6);
        assert!(matches!(
            received.try_recv(),
            Ok(AudioEvent::Started { generation: 11, .. })
        ));

        fixture.consumed_frames.store(24_000, Ordering::Release);
        core.drive(&events, &positions);
        assert_eq!(
            positions.take().expect("coalesced position"),
            AudioPosition {
                generation: 11,
                timeline_revision: 1,
                position: Duration::from_millis(500),
                duration: Some(Duration::from_secs(1)),
            }
        );
        core.command(
            AudioCommand::Seek {
                generation: 11,
                position: Duration::from_millis(750),
            },
            &events,
            &positions,
        )
        .expect("seek fake playback");
        drive_steps(&mut core, &events, &positions, 6);
        assert_eq!(
            received.try_recv().expect("seek event"),
            AudioEvent::Seeked {
                generation: 11,
                timeline_revision: 2,
                position: Duration::from_millis(750),
            }
        );

        assert_eq!(
            fixture
                .opened_positions
                .lock()
                .expect("open log")
                .as_slice(),
            [Duration::ZERO, Duration::from_millis(750)]
        );
    }
}
