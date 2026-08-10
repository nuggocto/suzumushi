// SPDX-License-Identifier: Apache-2.0

//! Owned decoding, output, and playback worker.

mod decoder;
mod file;
mod output;

use std::fs::File;
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::errors::{AppError, AppResult};

pub(crate) use file::open_verified_media;

/// Runs the isolated decoder entry point selected by the hidden CLI command.
#[must_use]
pub fn decoder_helper_main() -> i32 {
    decoder::helper_main()
}

/// Exercises the bounded decoder adapter for fuzzing.
pub fn fuzz_decode(input: &[u8]) {
    decoder::fuzz_decode(input);
}

const COMMAND_CAPACITY: usize = 32;
const EVENT_CAPACITY: usize = 64;
const WORKER_POLL: Duration = Duration::from_millis(5);

/// One decoded PCM format accepted by the current pipeline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioFormat {
    pub sample_rate: u32,
    pub channels: u16,
}

/// Reliable commands sent by the app-owned terminal loop.
#[derive(Debug)]
pub enum AudioCommand {
    Play { generation: u64, file: File },
    Pause { generation: u64 },
    Resume { generation: u64 },
    Stop { generation: u64 },
    Shutdown,
}

/// Reliable state changes returned to the app-owned terminal loop.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AudioEvent {
    Started {
        generation: u64,
        format: AudioFormat,
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
        let worker = thread::Builder::new()
            .name("suzumushi-audio".into())
            .spawn(move || worker_main(&command_rx, &event_tx, ProductionBackend))
            .map_err(|error| AppError::Audio(format!("cannot start worker: {error}")))?;
        Ok(Self {
            commands: Some(command_tx),
            events: Some(event_rx),
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
        let Some(worker) = self.worker.take() else {
            return Ok(());
        };
        worker
            .join()
            .map_err(|_| AppError::Audio("worker panicked during shutdown".into()))?
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
    Ready(AudioFormat),
    Samples(Vec<f32>),
    End,
    Failed(String),
}

trait DecoderStream: Send {
    fn poll(&mut self) -> DecoderPoll;
    fn shutdown(&mut self) -> Result<(), String>;
}

trait OutputStream: Send {
    fn prepare(&mut self, source: AudioFormat) -> Result<(), String>;
    fn write(&mut self, samples: &[f32]) -> Result<usize, String>;
    fn play(&mut self) -> Result<(), String>;
    fn pause(&mut self) -> Result<(), String>;
    fn resume(&mut self) -> Result<(), String>;
    fn stop(&mut self) -> Result<(), String>;
    fn drained(&self) -> bool;
    fn failure(&self) -> Option<String>;
}

type PlaybackParts = (Box<dyn DecoderStream>, Box<dyn OutputStream>);

trait Backend: Send {
    fn open(&mut self, file: File) -> Result<PlaybackParts, String>;
}

struct ProductionBackend;

impl Backend for ProductionBackend {
    fn open(&mut self, file: File) -> Result<PlaybackParts, String> {
        Ok((
            Box::new(decoder::HelperDecoder::start(&file)?),
            Box::new(output::CpalOutput::new()),
        ))
    }
}

struct ActivePlayback {
    generation: u64,
    decoder: Box<dyn DecoderStream>,
    output: Box<dyn OutputStream>,
    format: Option<AudioFormat>,
    pending: Option<(Vec<f32>, usize)>,
    started: bool,
    paused: bool,
    ended: bool,
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
    ) -> Result<bool, String> {
        match command {
            AudioCommand::Play { generation, file } => {
                if let Err(error) = self.stop_active() {
                    emit(
                        events,
                        AudioEvent::Failed {
                            generation,
                            message: error,
                        },
                    );
                    return Ok(false);
                }
                match self.backend.open(file) {
                    Ok((decoder, output)) => {
                        self.active = Some(ActivePlayback {
                            generation,
                            decoder,
                            output,
                            format: None,
                            pending: None,
                            started: false,
                            paused: false,
                            ended: false,
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
            AudioCommand::Pause { generation } => {
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
            }
            AudioCommand::Resume { generation } => {
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
            AudioCommand::Stop { generation } => {
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
            AudioCommand::Shutdown => {
                self.stop_active()?;
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn drive(&mut self, events: &SyncSender<AudioEvent>) -> bool {
        let Some(active) = self.active.as_ref() else {
            return false;
        };
        let generation = active.generation;
        if self.output_failed(events, generation) {
            return true;
        }

        let active = self
            .active
            .as_mut()
            .expect("failure handling retains successful playback");

        if let Some((samples, offset)) = active.pending.as_mut() {
            match active.output.write(&samples[*offset..]) {
                Ok(written) if written > 0 => {
                    *offset += written;
                    if !active.started {
                        if let Err(message) = active.output.play() {
                            self.fail(events, generation, &message);
                            return true;
                        }
                        active.started = true;
                        emit(
                            events,
                            AudioEvent::Started {
                                generation,
                                format: active
                                    .format
                                    .expect("samples require a prepared output format"),
                            },
                        );
                    }
                    if *offset == samples.len() {
                        active.pending = None;
                    }
                    return true;
                }
                Ok(_) => {}
                Err(message) => {
                    self.fail(events, generation, &message);
                    return true;
                }
            }
        }

        if active.pending.is_none() && !active.ended {
            match active.decoder.poll() {
                DecoderPoll::Pending => {}
                DecoderPoll::Ready(format) => {
                    if active.format.is_some() {
                        self.fail(events, generation, "decoder sent two format headers");
                    } else if let Err(message) = active.output.prepare(format) {
                        self.fail(events, generation, &message);
                    } else {
                        active.format = Some(format);
                    }
                    return true;
                }
                DecoderPoll::Samples(samples) => {
                    if active.format.is_none() {
                        self.fail(events, generation, "decoder sent samples before its format");
                    } else if samples.is_empty() {
                        self.fail(events, generation, "decoder sent an empty PCM block");
                    } else {
                        active.pending = Some((samples, 0));
                    }
                    return true;
                }
                DecoderPoll::End => {
                    active.ended = true;
                    return true;
                }
                DecoderPoll::Failed(message) => {
                    self.fail(events, generation, &message);
                    return true;
                }
            }
        }

        if active.ended && active.pending.is_none() {
            if !active.started {
                self.fail(
                    events,
                    generation,
                    "track contained no decodable audio samples",
                );
                return true;
            }
            if active.output.drained() {
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
                return true;
            }
        }
        false
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
        match (output_error, decoder_error) {
            (None, None) => Ok(()),
            (Some(error), None) | (None, Some(error)) => Err(error),
            (Some(left), Some(right)) => Err(format!("{left}; {right}")),
        }
    }
}

fn worker_main<B: Backend>(
    commands: &Receiver<AudioCommand>,
    events: &SyncSender<AudioEvent>,
    backend: B,
) -> AppResult<()> {
    let mut core = WorkerCore::new(backend);
    loop {
        match commands.try_recv() {
            Ok(command) => {
                if core.command(command, events).map_err(AppError::Audio)? {
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
        if core.drive(events) {
            continue;
        }
        match commands.recv_timeout(WORKER_POLL) {
            Ok(command) => {
                if core.command(command, events).map_err(AppError::Audio)? {
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
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use super::{
        AudioCommand, AudioEvent, AudioFormat, Backend, DecoderPoll, DecoderStream, OutputStream,
        PlaybackParts, WorkerCore, sync_channel,
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
    }

    impl OutputStream for FakeOutput {
        fn prepare(&mut self, _source: AudioFormat) -> Result<(), String> {
            self.calls.lock().expect("fake output log").push("prepare");
            Ok(())
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

        fn drained(&self) -> bool {
            self.drained.load(Ordering::Acquire)
        }

        fn failure(&self) -> Option<String> {
            None
        }
    }

    struct FakeBackend {
        script: VecDeque<DecoderPoll>,
        decoder_stopped: Arc<AtomicBool>,
        calls: Arc<Mutex<Vec<&'static str>>>,
        drained: Arc<AtomicBool>,
    }

    impl Backend for FakeBackend {
        fn open(&mut self, _file: File) -> Result<PlaybackParts, String> {
            Ok((
                Box::new(FakeDecoder {
                    script: std::mem::take(&mut self.script),
                    stopped: Arc::clone(&self.decoder_stopped),
                }),
                Box::new(FakeOutput {
                    calls: Arc::clone(&self.calls),
                    drained: Arc::clone(&self.drained),
                }),
            ))
        }
    }

    fn fixture_backend(script: VecDeque<DecoderPoll>) -> (FakeBackend, FakeFixture) {
        let fixture = FakeFixture {
            decoder_stopped: Arc::new(AtomicBool::new(false)),
            calls: Arc::new(Mutex::new(Vec::new())),
            drained: Arc::new(AtomicBool::new(false)),
        };
        (
            FakeBackend {
                script,
                decoder_stopped: Arc::clone(&fixture.decoder_stopped),
                calls: Arc::clone(&fixture.calls),
                drained: Arc::clone(&fixture.drained),
            },
            fixture,
        )
    }

    struct FakeFixture {
        decoder_stopped: Arc<AtomicBool>,
        calls: Arc<Mutex<Vec<&'static str>>>,
        drained: Arc<AtomicBool>,
    }

    fn harmless_file() -> File {
        File::open("/dev/null").expect("open harmless fake media")
    }

    #[test]
    fn fake_device_observes_ordered_play_pause_resume_and_stop() {
        let format = AudioFormat {
            sample_rate: 48_000,
            channels: 2,
        };
        let (backend, fixture) = fixture_backend(VecDeque::from([
            DecoderPoll::Ready(format),
            DecoderPoll::Samples(vec![0.1, -0.1, 0.2, -0.2]),
        ]));
        let mut core = WorkerCore::new(backend);
        let (events, received) = sync_channel(8);

        core.command(
            AudioCommand::Play {
                generation: 7,
                file: harmless_file(),
            },
            &events,
        )
        .expect("start fake playback");
        assert!(core.drive(&events));
        assert!(core.drive(&events));
        assert!(core.drive(&events));
        assert_eq!(
            received.try_recv().expect("started event"),
            AudioEvent::Started {
                generation: 7,
                format
            }
        );

        core.command(AudioCommand::Pause { generation: 7 }, &events)
            .expect("pause fake playback");
        core.command(AudioCommand::Resume { generation: 7 }, &events)
            .expect("resume fake playback");
        core.command(AudioCommand::Stop { generation: 7 }, &events)
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
            DecoderPoll::Ready(format),
            DecoderPoll::Samples(vec![0.1, 0.2]),
            DecoderPoll::End,
        ]));
        let mut core = WorkerCore::new(backend);
        let (events, received) = sync_channel(8);
        core.command(
            AudioCommand::Play {
                generation: 9,
                file: harmless_file(),
            },
            &events,
        )
        .expect("start fake playback");
        for _ in 0..5 {
            core.drive(&events);
        }
        assert_eq!(
            received.try_recv().expect("started event"),
            AudioEvent::Started {
                generation: 9,
                format
            }
        );
        assert!(received.try_recv().is_err(), "finish must wait for drain");

        fixture.drained.store(true, Ordering::Release);
        assert!(core.drive(&events));
        assert_eq!(
            received.try_recv().expect("finished event"),
            AudioEvent::Finished { generation: 9 }
        );
        assert!(fixture.decoder_stopped.load(Ordering::Acquire));
    }
}
