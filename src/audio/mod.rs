// SPDX-License-Identifier: Apache-2.0

//! Owned decoding, output, and playback worker.

mod decoder;
mod file;
mod output;
mod stretch;

use std::fs::File;
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::errors::{AppError, AppResult};

pub(crate) use file::open_verified_media;

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
const EVENT_CAPACITY: usize = 64;
const WORKER_POLL: Duration = Duration::from_millis(5);
const POSITION_INTERVAL: Duration = Duration::from_millis(250);

/// One decoded PCM format accepted by the current pipeline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioFormat {
    pub sample_rate: u32,
    pub channels: u16,
}

/// One validated pitch-preserving playback speed in quarter steps.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlaybackSpeed(u8);

impl PlaybackSpeed {
    pub const HALF: Self = Self(2);
    pub const NORMAL: Self = Self(4);
    pub const DOUBLE: Self = Self(8);

    #[must_use]
    pub fn slower(self) -> Self {
        Self(self.0.saturating_sub(1).max(Self::HALF.0))
    }

    #[must_use]
    pub fn faster(self) -> Self {
        Self(self.0.saturating_add(1).min(Self::DOUBLE.0))
    }

    #[must_use]
    pub fn percent(self) -> u16 {
        u16::from(self.0) * 25
    }

    #[must_use]
    pub fn multiplier(self) -> f32 {
        f32::from(self.0) * 0.25
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self.0 {
            2 => "0.5x",
            3 => "0.75x",
            4 => "1.0x",
            5 => "1.25x",
            6 => "1.5x",
            7 => "1.75x",
            8 => "2.0x",
            _ => unreachable!("playback speed is constructed only within its fixed bounds"),
        }
    }
}

impl Default for PlaybackSpeed {
    fn default() -> Self {
        Self::NORMAL
    }
}

/// App-owned playback settings applied to each newly opened track.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlaybackSettings {
    pub volume_percent: u8,
    pub muted: bool,
    pub speed: PlaybackSpeed,
}

impl Default for PlaybackSettings {
    fn default() -> Self {
        Self {
            volume_percent: 100,
            muted: false,
            speed: PlaybackSpeed::NORMAL,
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
        settings: PlaybackSettings,
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
    SetSpeed {
        generation: u64,
        speed: PlaybackSpeed,
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
    SpeedChanged {
        generation: u64,
        timeline_revision: u64,
        speed: PlaybackSpeed,
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
    events: Option<Receiver<AudioEvent>>,
    positions: Option<Arc<PositionLane>>,
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
        let (event_tx, event_rx) = sync_channel(EVENT_CAPACITY);
        let positions = Arc::new(PositionLane::default());
        let worker_positions = Arc::clone(&positions);
        let worker = thread::Builder::new()
            .name("suzumushi-audio".into())
            .spawn(move || {
                worker_main(&command_rx, &event_tx, &worker_positions, ProductionBackend)
            })
            .map_err(|error| AppError::Audio(format!("cannot start worker: {error}")))?;
        Ok(Self {
            commands: Some(command_tx),
            events: Some(event_rx),
            positions: Some(positions),
            worker: Some(worker),
        })
    }

    /// Sends one reliable playback command.
    ///
    /// # Errors
    ///
    /// Returns an audio error when the worker has stopped.
    pub fn send(&self, command: AudioCommand) -> AppResult<()> {
        self.commands
            .as_ref()
            .ok_or_else(|| AppError::Audio("worker is already stopped".into()))?
            .send(command)
            .map_err(|_| AppError::Audio("worker command lane disconnected".into()))
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

    /// Cancels active playback and awaits every owned thread and helper.
    ///
    /// # Errors
    ///
    /// Returns the worker's cleanup failure or panic.
    pub fn shutdown(mut self) -> AppResult<()> {
        self.stop()
    }

    fn stop(&mut self) -> AppResult<()> {
        if let Some(commands) = self.commands.take() {
            let _ = commands.try_send(AudioCommand::Shutdown);
            drop(commands);
        }
        self.events.take();
        self.positions.take();
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

struct ProductionBackend;

impl Backend for ProductionBackend {
    fn open(&mut self, file: &File, position: Duration) -> Result<PlaybackParts, String> {
        Ok((
            Box::new(decoder::HelperDecoder::start(file, position)?),
            Box::new(output::CpalOutput::new()),
        ))
    }
}

#[derive(Clone, Copy, Debug)]
enum RestartNotice {
    Seek,
    Speed,
}

struct ActivePlayback {
    generation: u64,
    timeline_revision: u64,
    file: File,
    decoder: Box<dyn DecoderStream>,
    output: Box<dyn OutputStream>,
    info: Option<DecodedInfo>,
    settings: PlaybackSettings,
    tempo: Option<stretch::TempoProcessor>,
    pending: Option<(Vec<f32>, usize)>,
    started: bool,
    paused: bool,
    ended: bool,
    base_position: Duration,
    last_reported_position: Duration,
    restart_notice: Option<RestartNotice>,
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
        events: &SyncSender<AudioEvent>,
        positions: &PositionLane,
    ) -> Result<bool, String> {
        match command {
            AudioCommand::Play {
                generation,
                file,
                settings,
            } => self.play(generation, file, settings, events),
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
            AudioCommand::SetSpeed { generation, speed } => {
                self.set_speed(generation, speed, events);
            }
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
        mut settings: PlaybackSettings,
        events: &SyncSender<AudioEvent>,
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
        match self.backend.open(&file, Duration::ZERO) {
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
                    tempo: None,
                    pending: None,
                    started: false,
                    paused: false,
                    ended: false,
                    base_position: Duration::ZERO,
                    last_reported_position: Duration::ZERO,
                    restart_notice: None,
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

    fn pause(
        &mut self,
        generation: u64,
        events: &SyncSender<AudioEvent>,
        positions: &PositionLane,
    ) {
        if let Some(active) = self.matching_active(generation)
            && active.started
            && !active.paused
        {
            match active.output.pause() {
                Ok(()) => {
                    active.paused = true;
                    emit(events, AudioEvent::Paused { generation });
                }
                Err(message) => self.fail(events, generation, &message),
            }
        }
        self.publish_position(positions, true);
    }

    fn resume(&mut self, generation: u64, events: &SyncSender<AudioEvent>) {
        if let Some(active) = self.matching_active(generation)
            && active.started
            && active.paused
        {
            match active.output.resume() {
                Ok(()) => {
                    active.paused = false;
                    emit(events, AudioEvent::Resumed { generation });
                }
                Err(message) => self.fail(events, generation, &message),
            }
        }
    }

    fn stop(&mut self, generation: u64, events: &SyncSender<AudioEvent>) {
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
        events: &SyncSender<AudioEvent>,
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
            && let Err(message) =
                self.restart_active(generation, position, None, RestartNotice::Seek)
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

    fn set_speed(
        &mut self,
        generation: u64,
        speed: PlaybackSpeed,
        events: &SyncSender<AudioEvent>,
    ) {
        let Some(active) = self
            .active
            .as_ref()
            .filter(|active| active.generation == generation)
        else {
            return;
        };
        if active.settings.speed == speed {
            return;
        }
        if active.info.is_none() {
            let position = active.base_position;
            let timeline_revision = active.timeline_revision;
            self.active
                .as_mut()
                .expect("matching loading playback is retained")
                .settings
                .speed = speed;
            emit(
                events,
                AudioEvent::SpeedChanged {
                    generation,
                    timeline_revision,
                    speed,
                    position,
                },
            );
        } else {
            let position = self.current_position().unwrap_or(Duration::ZERO);
            if let Err(message) =
                self.restart_active(generation, position, Some(speed), RestartNotice::Speed)
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
    }

    fn drive(&mut self, events: &SyncSender<AudioEvent>, positions: &PositionLane) -> bool {
        let Some(active) = self.active.as_ref() else {
            return false;
        };
        let generation = active.generation;
        if self.output_failed(events, generation) {
            return true;
        }

        self.publish_position(positions, false);
        self.write_pending(events, generation)
            || self.pull_tempo(events, generation)
            || self.poll_decoder(events, generation)
            || self.finish_drained(events, positions, generation)
    }

    fn write_pending(&mut self, events: &SyncSender<AudioEvent>, generation: u64) -> bool {
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

    fn pull_tempo(&mut self, events: &SyncSender<AudioEvent>, generation: u64) -> bool {
        let active = self.active.as_mut().expect("active playback is retained");
        if active.pending.is_some() {
            return false;
        }
        let Some(tempo) = active.tempo.as_mut() else {
            return false;
        };
        match tempo.pull() {
            Ok(Some(samples)) => {
                active.pending = Some((samples, 0));
                true
            }
            Ok(None) => false,
            Err(message) => {
                self.fail(events, generation, &message);
                true
            }
        }
    }

    fn poll_decoder(&mut self, events: &SyncSender<AudioEvent>, generation: u64) -> bool {
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
                if let Some(tempo) = active.tempo.as_mut()
                    && let Err(message) = tempo.finish()
                {
                    self.fail(events, generation, &message);
                }
                true
            }
            DecoderPoll::Failed(message) => {
                self.fail(events, generation, &message);
                true
            }
        }
    }

    fn decoder_ready(
        &mut self,
        events: &SyncSender<AudioEvent>,
        generation: u64,
        info: DecodedInfo,
    ) {
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
            active.tempo = if active.settings.speed == PlaybackSpeed::NORMAL {
                None
            } else {
                match stretch::TempoProcessor::new(info.format, active.settings.speed) {
                    Ok(tempo) => Some(tempo),
                    Err(message) => {
                        self.fail(events, generation, &message);
                        return;
                    }
                }
            };
            active.info = Some(info);
        }
    }

    fn decoder_samples(
        &mut self,
        events: &SyncSender<AudioEvent>,
        generation: u64,
        samples: Vec<f32>,
    ) {
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
        } else if let Some(tempo) = active.tempo.as_mut() {
            if let Err(message) = tempo.push(&samples) {
                self.fail(events, generation, &message);
            }
        } else {
            active.pending = Some((samples, 0));
        }
    }

    fn finish_drained(
        &mut self,
        events: &SyncSender<AudioEvent>,
        positions: &PositionLane,
        generation: u64,
    ) -> bool {
        let active = self.active.as_ref().expect("active playback is retained");
        let tempo_drained = active
            .tempo
            .as_ref()
            .is_none_or(stretch::TempoProcessor::drained);
        if !active.ended || active.pending.is_some() || !tempo_drained {
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

    fn restart_active(
        &mut self,
        generation: u64,
        position: Duration,
        speed: Option<PlaybackSpeed>,
        notice: RestartNotice,
    ) -> Result<(), String> {
        let Some(mut old) = self.active.take() else {
            return Ok(());
        };
        if old.generation != generation {
            self.active = Some(old);
            return Ok(());
        }
        let output_error = old.output.stop().err();
        let decoder_error = old.decoder.shutdown().err();
        if let Some(speed) = speed {
            old.settings.speed = speed;
        }
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
            tempo: None,
            pending: None,
            started: false,
            paused,
            ended: false,
            base_position: position,
            last_reported_position: position,
            restart_notice: Some(notice),
        });
        Ok(())
    }

    fn current_position(&self) -> Option<Duration> {
        self.active.as_ref().map(position_for)
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

    fn output_failed(&mut self, events: &SyncSender<AudioEvent>, generation: u64) -> bool {
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

    fn fail(&mut self, events: &SyncSender<AudioEvent>, generation: u64, message: &str) {
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

fn emit_started(active: &mut ActivePlayback, events: &SyncSender<AudioEvent>) {
    let info = active
        .info
        .expect("samples require prepared decoder information");
    let generation = active.generation;
    let timeline_revision = active.timeline_revision;
    let event = match active.restart_notice.take() {
        None => AudioEvent::Started {
            generation,
            timeline_revision,
            format: info.format,
            duration: info.duration,
            position: info.position,
        },
        Some(RestartNotice::Seek) => AudioEvent::Seeked {
            generation,
            timeline_revision,
            position: info.position,
        },
        Some(RestartNotice::Speed) => AudioEvent::SpeedChanged {
            generation,
            timeline_revision,
            speed: active.settings.speed,
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
    let speed_percent = u128::from(active.settings.speed.percent());
    let denominator = u128::from(info.format.sample_rate).saturating_mul(100);
    let nanos = frames
        .saturating_mul(1_000_000_000)
        .saturating_mul(speed_percent)
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
    events: &SyncSender<AudioEvent>,
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

fn emit(events: &SyncSender<AudioEvent>, event: AudioEvent) {
    let _ = events.send(event);
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::fs::File;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::{
        AudioCommand, AudioEvent, AudioFormat, AudioPosition, Backend, DecoderPoll, DecoderStream,
        OutputStream, PlaybackParts, PlaybackSettings, PlaybackSpeed, PositionLane, WorkerCore,
        sync_channel,
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
        events: &std::sync::mpsc::SyncSender<AudioEvent>,
        positions: &PositionLane,
        steps: usize,
    ) {
        for _ in 0..steps {
            core.drive(events, positions);
        }
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
    fn playback_speed_steps_clamp_at_half_and_double() {
        let mut speed = PlaybackSpeed::NORMAL;
        for _ in 0..10 {
            speed = speed.slower();
        }
        assert_eq!(speed, PlaybackSpeed::HALF);
        assert_eq!(speed.label(), "0.5x");
        for _ in 0..10 {
            speed = speed.faster();
        }
        assert_eq!(speed, PlaybackSpeed::DOUBLE);
        assert_eq!(speed.label(), "2.0x");
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
        let (events, received) = sync_channel(8);
        let positions = PositionLane::default();

        core.command(
            AudioCommand::Play {
                generation: 7,
                file: harmless_file(),
                settings: PlaybackSettings::default(),
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
        let (events, received) = sync_channel(8);
        let positions = PositionLane::default();
        core.command(
            AudioCommand::Play {
                generation: 9,
                file: harmless_file(),
                settings: PlaybackSettings::default(),
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
        let (events, _received) = sync_channel(4);
        let positions = PositionLane::default();
        core.command(
            AudioCommand::Play {
                generation: 10,
                file: harmless_file(),
                settings: PlaybackSettings::default(),
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
    fn controls_restart_at_the_app_selected_source_position() {
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
        backend.scripts.push_back(VecDeque::from([
            DecoderPoll::Ready(decoded_at(format, Duration::from_millis(750))),
            DecoderPoll::Samples(vec![0.1; 16_384]),
            DecoderPoll::End,
        ]));
        let mut core = WorkerCore::new(backend);
        let (events, received) = sync_channel(16);
        let positions = PositionLane::default();
        core.command(
            AudioCommand::Play {
                generation: 11,
                file: harmless_file(),
                settings: PlaybackSettings::default(),
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

        core.command(
            AudioCommand::SetSpeed {
                generation: 11,
                speed: PlaybackSpeed::DOUBLE,
            },
            &events,
            &positions,
        )
        .expect("change fake speed");
        drive_steps(&mut core, &events, &positions, 6);
        assert_eq!(
            received.try_recv().expect("speed event"),
            AudioEvent::SpeedChanged {
                generation: 11,
                timeline_revision: 3,
                speed: PlaybackSpeed::DOUBLE,
                position: Duration::from_millis(750),
            }
        );
        assert_eq!(
            fixture
                .opened_positions
                .lock()
                .expect("open log")
                .as_slice(),
            [
                Duration::ZERO,
                Duration::from_millis(750),
                Duration::from_millis(750),
            ]
        );
    }
}
