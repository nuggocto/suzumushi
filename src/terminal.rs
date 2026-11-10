// SPDX-License-Identifier: Apache-2.0

//! Terminal entry, event-loop ownership, and best-effort restoration.

use std::io::{self, Stdout, Write};
use std::os::fd::BorrowedFd;
use std::path::Path;
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossterm::cursor::{Hide, Show};
use crossterm::execute;
use crossterm::terminal::{
    Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::app::{AppState, PlaybackIntent};
use crate::audio::{AudioCommand, AudioDiagnostics, AudioRuntime};
use crate::config::Config;
use crate::errors::{AppError, AppResult};
use crate::event::{AppEvent, EventSource};
use crate::model::ScanIndex;
use crate::mpris::{MprisProjection, MprisRuntime, REQUEST_CAPACITY};

/// While the playback position moves, the resume checkpoint is refreshed at
/// most this often, so a crash loses at most this much of the position.
const SESSION_POSITION_INTERVAL: Duration = Duration::from_secs(5);

/// Runs one terminal session and restores terminal modes on every unwind path.
///
/// # Errors
///
/// Returns a terminal, drawing, or event-stream error.
pub fn run(
    root: BorrowedFd<'_>,
    root_path: &Path,
    config: &Config,
    index: ScanIndex,
) -> AppResult<()> {
    let mut app = AppState::new(config, index);
    let mut state_store = crate::state::StateStore::open(root, root_path)?;
    state_store.remove_retired()?;
    match state_store.read_session(&config.queue)? {
        crate::state::SessionLoad::Missing => {}
        crate::state::SessionLoad::Loaded(snapshot) => app.restore_session(&snapshot)?,
        crate::state::SessionLoad::Ignored(reason) => app.session_ignored(&reason),
    }
    let mut state_sync = StateSynchronizer::default();
    state_sync.sync(&app, &mut state_store, Duration::ZERO)?;
    let audio = AudioRuntime::start()?;
    let mut mpris = match MprisRuntime::start(MprisProjection::from_app(&app)) {
        Ok(runtime) => Some(runtime),
        Err(error) => {
            crate::logging::warn(format_args!("desktop controls unavailable: {error}"));
            app.desktop_controls_unavailable(&error.to_string());
            None
        }
    };
    let session_result = run_terminal_session(
        root,
        &mut app,
        &audio,
        mpris.as_mut(),
        &mut state_store,
        &mut state_sync,
    );
    let audio_result = audio.shutdown();
    app.session_stopped();
    let mpris_result = mpris.map_or(Ok(()), |mut runtime| {
        runtime.publish(MprisProjection::from_app(&app));
        runtime.shutdown()
    });
    finish_session(
        &mut app,
        &mut state_store,
        session_result,
        combine_results(audio_result, mpris_result),
    )
}

fn finish_session(
    app: &mut AppState,
    state_store: &mut crate::state::StateStore,
    session_result: AppResult<()>,
    audio_result: AppResult<()>,
) -> AppResult<()> {
    app.session_stopped();
    let state_result = write_session(app, state_store);
    combine_results(combine_results(session_result, audio_result), state_result)
}

fn run_terminal_session(
    root: BorrowedFd<'_>,
    app: &mut AppState,
    audio: &AudioRuntime,
    mut mpris: Option<&mut MprisRuntime>,
    state_store: &mut crate::state::StateStore,
    state_sync: &mut StateSynchronizer,
) -> AppResult<()> {
    let stdout = io::stdout();
    install_panic_restore();
    enable_raw_mode().map_err(|error| AppError::io("enable raw mode", "terminal", error))?;
    let mut guard = TerminalGuard::new(CrosstermControl { stdout });
    guard.enter()?;

    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))
        .map_err(|error| AppError::io("create terminal", "terminal", error))?;

    let events = EventSource::new();
    let mut stage = crate::ui::Stage::new(crate::ui::ColorDepth::from_environment());
    let mut event_time = Duration::ZERO;
    let mut audio_diagnostics = AudioDiagnosticsReporter::default();

    while !app.should_quit {
        drain_audio_events(app, root, audio)?;
        for line in audio_diagnostics.report(audio.diagnostics(), event_time) {
            crate::logging::warn(format_args!("{line}"));
        }
        drain_mpris_actions(app, root, audio, mpris.as_deref())?;
        publish_mpris(app, mpris.as_deref_mut());
        state_sync.sync(app, state_store, event_time)?;
        // A fullscreen viewport follows the terminal size: `draw` resizes and
        // clears its buffers whenever the size changed since the last frame.
        terminal
            .draw(|frame| crate::ui::render(frame, app, &mut stage, event_time))
            .map_err(|error| AppError::io("draw terminal", "terminal", error))?;
        let event = events.next(stage.frame_interval())?;
        event_time = event_time_for(event);
        if let Some(intent) = apply_event(app, event) {
            dispatch_playback(app, root, audio, intent)?;
        }
        publish_mpris(app, mpris.as_deref_mut());
        state_sync.sync(app, state_store, event_time)?;
    }

    terminal
        .show_cursor()
        .map_err(|error| AppError::io("show terminal cursor", "terminal", error))?;
    drop(terminal);
    guard.restore()
}

fn drain_mpris_actions(
    app: &mut AppState,
    root: BorrowedFd<'_>,
    audio: &AudioRuntime,
    mpris: Option<&MprisRuntime>,
) -> AppResult<()> {
    let Some(mpris) = mpris else {
        return Ok(());
    };
    for _ in 0..REQUEST_CAPACITY {
        let Some(action) = mpris.try_action() else {
            break;
        };
        if let Some(intent) = app.apply(action) {
            dispatch_playback(app, root, audio, intent)?;
        }
    }
    Ok(())
}

fn publish_mpris(app: &AppState, mpris: Option<&mut MprisRuntime>) {
    if let Some(runtime) = mpris {
        runtime.publish(MprisProjection::from_app(app));
    }
}

/// Writes the resume checkpoint when it changes.
///
/// A different queue or current track is written at once. A moving position is
/// written at most every [`SESSION_POSITION_INTERVAL`]. A stopped or paused
/// session writes nothing.
#[derive(Default)]
struct StateSynchronizer {
    written: Option<(Option<crate::app::SessionIdentity>, Duration)>,
    last_write: Duration,
}

impl StateSynchronizer {
    fn sync(
        &mut self,
        app: &AppState,
        store: &mut crate::state::StateStore,
        now: Duration,
    ) -> AppResult<()> {
        let checkpoint = (app.session_identity(), app.session_position());
        let due = match self.written {
            None => true,
            Some((identity, _)) if identity != checkpoint.0 => true,
            Some(written) => {
                written != checkpoint
                    && now.saturating_sub(self.last_write) >= SESSION_POSITION_INTERVAL
            }
        };
        if due {
            write_session(app, store)?;
            self.written = Some(checkpoint);
            self.last_write = now;
        }
        Ok(())
    }
}

fn write_session(app: &AppState, store: &mut crate::state::StateStore) -> AppResult<()> {
    match app.session_snapshot() {
        Some(snapshot) => store.write_session(&snapshot),
        None => store.clear_session(),
    }
}

fn combine_results(primary: AppResult<()>, secondary: AppResult<()>) -> AppResult<()> {
    match (primary, secondary) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(primary), Err(secondary)) => Err(AppError::Multiple {
            primary: Box::new(primary),
            secondary: Box::new(secondary),
        }),
    }
}

const fn event_time_for(event: AppEvent) -> Duration {
    match event {
        AppEvent::Key(_, now) | AppEvent::Resize(_, _, now) | AppEvent::Tick(now) => now,
    }
}

fn apply_event(app: &mut AppState, event: AppEvent) -> Option<PlaybackIntent> {
    match event {
        AppEvent::Key(key, now) => app.key(key, now),
        AppEvent::Resize(_, _, now) | AppEvent::Tick(now) => {
            app.advance_time(now);
            None
        }
    }
}

fn drain_audio_events(
    app: &mut AppState,
    root: BorrowedFd<'_>,
    audio: &AudioRuntime,
) -> AppResult<()> {
    app.audio_spectrum(audio.spectrum());
    while let Some(position) = audio.try_position() {
        app.audio_position(position);
    }
    while let Some(event) = audio.try_event()? {
        if let Some(intent) = app.audio_event(event) {
            dispatch_playback(app, root, audio, intent)?;
        }
    }
    Ok(())
}

/// Rate limit for audio diagnostics. The event loop turns every `MAX_POLL`, so an
/// ongoing dropout would otherwise warn ten times a second and fill the log.
const DIAGNOSTICS_REPORT_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Default)]
struct AudioDiagnosticsReporter {
    reported: AudioDiagnostics,
    last_report: Option<Duration>,
    realtime_denial_logged: bool,
}

impl AudioDiagnosticsReporter {
    /// Returns a log line for each counter that grew since the last report, at
    /// most once per [`DIAGNOSTICS_REPORT_INTERVAL`]. The interval only advances
    /// when a line is returned, so a quiet counter cannot delay a real dropout.
    fn report(&mut self, current: AudioDiagnostics, now: Duration) -> Vec<String> {
        let mut lines = Vec::new();
        if current == self.reported {
            return lines;
        }
        let due = self
            .last_report
            .is_none_or(|last| now.saturating_sub(last) >= DIAGNOSTICS_REPORT_INTERVAL);
        if !due {
            return lines;
        }

        let underruns = current
            .underrun_samples
            .saturating_sub(self.reported.underrun_samples);
        if underruns != 0 {
            lines.push(format!(
                "audio output ring underrun: {underruns} samples ({} total)",
                current.underrun_samples
            ));
        }
        let xruns = current.xruns.saturating_sub(self.reported.xruns);
        if xruns != 0 {
            lines.push(format!(
                "audio backend reported {xruns} xruns ({} total)",
                current.xruns
            ));
        }
        // The backend reroutes the stream itself, so this is expected, not a fault.
        let device_changes = current
            .device_changes
            .saturating_sub(self.reported.device_changes);
        if device_changes != 0 {
            lines.push(format!(
                "audio output was rerouted to a new default device ({} total)",
                current.device_changes
            ));
            // A new route can mean a different backend, where a refusal is no longer
            // the harmless redundant one described below. Let it be said again.
            self.realtime_denial_logged = false;
        }
        // Real-time promotion is a property of the audio path rather than an incident:
        // it either succeeds when a stream starts or it does not. PipeWire promotes its
        // own data thread and then refuses CPAL's redundant attempt on every callback,
        // so this is said once per path and the counter accrues in silence.
        let denials = current
            .realtime_denied
            .saturating_sub(self.reported.realtime_denied);
        if !self.realtime_denial_logged && denials != 0 {
            lines.push(format!(
                "audio thread real-time promotion was refused ({} total); the backend may \
                 already schedule its own audio thread",
                current.realtime_denied
            ));
            self.realtime_denial_logged = true;
        }

        self.reported = current;
        if !lines.is_empty() {
            self.last_report = Some(now);
        }
        lines
    }
}

fn dispatch_playback(
    app: &mut AppState,
    root: BorrowedFd<'_>,
    audio: &AudioRuntime,
    intent: PlaybackIntent,
) -> AppResult<()> {
    audio.send(playback_command(app, root, intent))
}

fn playback_command(app: &AppState, root: BorrowedFd<'_>, intent: PlaybackIntent) -> AudioCommand {
    match intent {
        PlaybackIntent::Load {
            generation,
            item,
            position,
            settings,
            paused,
        } => {
            let opened = app
                .media_for_item(item)
                .ok_or_else(|| AppError::Audio("queued track became stale; rescan required".into()))
                .and_then(|(entry, asset)| crate::audio::open_verified_media(root, entry, asset));
            match opened {
                Ok(file) => AudioCommand::Play {
                    generation,
                    file,
                    position,
                    settings,
                    paused,
                },
                Err(error) => AudioCommand::LoadFailed {
                    generation,
                    message: error.to_string(),
                },
            }
        }
        PlaybackIntent::Pause { generation } => AudioCommand::Pause { generation },
        PlaybackIntent::Resume { generation } => AudioCommand::Resume { generation },
        PlaybackIntent::Stop { generation } => AudioCommand::Stop { generation },
        PlaybackIntent::SetGain {
            generation,
            volume_percent,
            muted,
        } => AudioCommand::SetGain {
            generation,
            volume_percent,
            muted,
        },
        PlaybackIntent::Seek {
            generation,
            position,
        } => AudioCommand::Seek {
            generation,
            position,
        },
    }
}

/// Set while the terminal is in raw mode on the alternate screen.
static TERMINAL_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Release builds abort on panic, so `TerminalGuard` never drops. This hook
/// restores the terminal first, then lets the default hook print the message
/// on the normal screen where it stays readable.
fn install_panic_restore() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let report = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if TERMINAL_ACTIVE.swap(false, Ordering::AcqRel) {
                let _ = execute!(io::stdout(), Show, LeaveAlternateScreen);
                let _ = disable_raw_mode();
            }
            report(info);
        }));
    });
}

trait TerminalControl {
    type Output: Write;

    fn output_mut(&mut self) -> &mut Self::Output;
    fn disable_raw(&mut self) -> io::Result<()>;
}

struct CrosstermControl {
    stdout: Stdout,
}

impl TerminalControl for CrosstermControl {
    type Output = Stdout;

    fn output_mut(&mut self) -> &mut Self::Output {
        &mut self.stdout
    }

    fn disable_raw(&mut self) -> io::Result<()> {
        disable_raw_mode()
    }
}

struct TerminalGuard<C: TerminalControl> {
    control: C,
    alternate: bool,
    raw: bool,
}

impl<C: TerminalControl> TerminalGuard<C> {
    fn new(control: C) -> Self {
        Self {
            control,
            alternate: false,
            raw: true,
        }
    }

    fn enter(&mut self) -> AppResult<()> {
        // A partial escape write still needs the matching best-effort cleanup.
        self.alternate = true;
        TERMINAL_ACTIVE.store(true, Ordering::Release);
        execute!(
            self.control.output_mut(),
            EnterAlternateScreen,
            Clear(ClearType::All),
            Hide
        )
        .map_err(|error| AppError::io("enter alternate screen", "terminal", error))?;
        Ok(())
    }

    fn restore(&mut self) -> AppResult<()> {
        TERMINAL_ACTIVE.store(false, Ordering::Release);
        let mut first_error = None;
        if self.alternate {
            if let Err(error) = execute!(self.control.output_mut(), Show, LeaveAlternateScreen) {
                first_error = Some(error);
            } else {
                self.alternate = false;
            }
        }
        if self.raw {
            if let Err(error) = self.control.disable_raw() {
                first_error.get_or_insert(error);
            } else {
                self.raw = false;
            }
        }
        if let Err(error) = self.control.output_mut().flush() {
            first_error.get_or_insert(error);
        }
        first_error.map_or(Ok(()), |error| {
            Err(AppError::io("restore terminal", "terminal", error))
        })
    }
}

impl<C: TerminalControl> Drop for TerminalGuard<C> {
    fn drop(&mut self) {
        TERMINAL_ACTIVE.store(false, Ordering::Release);
        if self.alternate {
            let _ = execute!(self.control.output_mut(), Show, LeaveAlternateScreen);
        }
        if self.raw {
            let _ = self.control.disable_raw();
        }
        let _ = self.control.output_mut().flush();
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::fs;
    use std::io::{self, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::rc::Rc;
    use std::time::Duration;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use rustix::fd::AsFd;

    use super::{
        AudioDiagnostics, AudioDiagnosticsReporter, DIAGNOSTICS_REPORT_INTERVAL,
        SESSION_POSITION_INTERVAL, StateSynchronizer, TerminalControl, TerminalGuard, apply_event,
        finish_session,
    };
    use crate::app::AppState;
    use crate::config::Config;
    use crate::errors::AppError;
    use crate::event::AppEvent;
    use crate::input::AppAction;
    use crate::model::{ScanCounters, ScanIndex};

    fn empty_index() -> ScanIndex {
        ScanIndex {
            generation: 1,
            complete: true,
            assets: Vec::new(),
            entries: Vec::new(),
            playlists: Vec::new(),
            warnings: Vec::new(),
            counters: ScanCounters::default(),
        }
    }

    #[test]
    fn stale_queued_media_becomes_a_load_failure_command() {
        let app = AppState::new(&Config::default(), empty_index());
        let root = tempfile::tempfile().expect("unused root descriptor");
        let command = super::playback_command(
            &app,
            root.as_fd(),
            crate::app::PlaybackIntent::Load {
                generation: 2,
                item: crate::app::QueueItem {
                    instance_id: 1,
                    entry_index: 0,
                    entry_id: crate::model::TrackEntryId(1),
                    scan_generation: 1,
                },
                position: Duration::ZERO,
                settings: crate::audio::PlaybackSettings::default(),
                paused: false,
            },
        );
        assert!(
            matches!(command, crate::audio::AudioCommand::LoadFailed { generation: 2, message }
            if message.contains("queued track became stale"))
        );
    }

    #[test]
    fn a_persistent_real_time_denial_is_reported_only_once() {
        let mut reporter = AudioDiagnosticsReporter::default();
        let mut counters = AudioDiagnostics {
            realtime_denied: 50,
            ..AudioDiagnostics::default()
        };

        let first = reporter.report(counters, Duration::ZERO);
        assert_eq!(first.len(), 1);
        assert!(first[0].contains("real-time promotion was refused (50 total)"));

        // PipeWire refuses CPAL's redundant promotion on every callback, so the
        // counter keeps climbing. That must stay silent, and it must not consume the
        // report interval, or a real dropout would be left waiting behind it.
        counters.realtime_denied = 5_000;
        assert_eq!(
            reporter.report(counters, DIAGNOSTICS_REPORT_INTERVAL * 2),
            Vec::<String>::new()
        );
        counters.underrun_samples = 64;
        let dropout = reporter.report(counters, DIAGNOSTICS_REPORT_INTERVAL * 2);
        assert_eq!(
            dropout,
            ["audio output ring underrun: 64 samples (64 total)"]
        );
    }

    #[test]
    fn a_rerouted_device_lets_a_real_time_denial_be_said_again() {
        let mut reporter = AudioDiagnosticsReporter::default();
        let mut counters = AudioDiagnostics {
            realtime_denied: 10,
            ..AudioDiagnostics::default()
        };
        assert_eq!(reporter.report(counters, Duration::ZERO).len(), 1);

        // The route moved, so the next refusal may come from a different backend
        // where it is a real cause of dropouts rather than a redundant attempt.
        counters.device_changes = 1;
        counters.realtime_denied = 11;
        let lines = reporter.report(counters, DIAGNOSTICS_REPORT_INTERVAL);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].contains("rerouted"));
        assert!(lines[1].contains("real-time promotion was refused (11 total)"));
    }

    #[test]
    fn dropouts_inside_the_interval_wait_for_the_next_report() {
        let mut reporter = AudioDiagnosticsReporter::default();
        let mut counters = AudioDiagnostics {
            underrun_samples: 128,
            ..AudioDiagnostics::default()
        };
        assert_eq!(reporter.report(counters, Duration::ZERO).len(), 1);

        counters.underrun_samples = 256;
        assert_eq!(
            reporter.report(counters, DIAGNOSTICS_REPORT_INTERVAL / 2),
            Vec::<String>::new()
        );
        counters.underrun_samples = 384;
        assert_eq!(
            reporter.report(counters, DIAGNOSTICS_REPORT_INTERVAL),
            ["audio output ring underrun: 256 samples (384 total)"]
        );
    }

    #[test]
    fn periodic_ticks_clear_expired_information_notices() {
        let mut app = AppState::new(&Config::default(), empty_index());
        apply_event(
            &mut app,
            AppEvent::Key(
                KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
                Duration::from_secs(10),
            ),
        );

        apply_event(&mut app, AppEvent::Tick(Duration::from_millis(12_999)));
        assert_eq!(
            app.status_message, "Shuffle: on",
            "the notice remains readable before its deadline"
        );

        apply_event(
            &mut app,
            AppEvent::Key(
                KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
                Duration::from_millis(12_999),
            ),
        );
        apply_event(&mut app, AppEvent::Tick(Duration::from_secs(13)));
        assert_eq!(
            app.status_message, "Shuffle: off",
            "a newer notice replaces the old deadline"
        );

        apply_event(&mut app, AppEvent::Tick(Duration::from_millis(15_999)));
        assert!(
            app.status_message.is_empty(),
            "the periodic event clears the newest notice at its deadline"
        );
    }

    #[test]
    fn periodic_ticks_preserve_warnings() {
        let mut app = AppState::new(&Config::default(), empty_index());
        app.desktop_controls_unavailable("fixture unavailable");

        apply_event(&mut app, AppEvent::Tick(Duration::from_secs(30)));

        assert_eq!(
            app.status_message,
            "Desktop controls unavailable: fixture unavailable"
        );
    }

    fn state_writer() -> (tempfile::TempDir, std::fs::File, crate::state::StateStore) {
        let root = tempfile::tempdir().expect("temporary root");
        fs::create_dir(root.path().join("state")).expect("state directory");
        fs::set_permissions(root.path().join("state"), fs::Permissions::from_mode(0o700))
            .expect("private state permissions");
        let root_file = std::fs::File::open(root.path()).expect("open root");
        let writer =
            crate::state::StateStore::open(root_file.as_fd(), root.path()).expect("state writer");
        (root, root_file, writer)
    }

    fn one_track_index() -> ScanIndex {
        use crate::model::{
            FileIdentity, MediaAsset, SearchFields, TrackEntry, TrackEntryId, TrackEntrySource,
            TrackTags,
        };
        ScanIndex {
            assets: vec![MediaAsset {
                tags: TrackTags::default(),
                file_identity: FileIdentity {
                    device: 1,
                    inode: 1,
                    size: 1,
                    modified_seconds: 1,
                    modified_nanoseconds: 0,
                },
            }],
            entries: vec![TrackEntry {
                id: TrackEntryId(7),
                asset_index: 0,
                display_path: std::path::PathBuf::from("library/night.wav"),
                source: TrackEntrySource::LibraryFile,
                search: SearchFields {
                    filename: "night".into(),
                    relative_path: "library/night.wav".into(),
                },
            }],
            ..empty_index()
        }
    }

    fn modified(path: &std::path::Path) -> Option<std::time::SystemTime> {
        fs::metadata(path)
            .ok()
            .map(|metadata| metadata.modified().expect("modification time"))
    }

    #[test]
    fn the_session_checkpoint_is_written_only_when_it_changes() {
        let (root, _root_file, mut writer) = state_writer();
        let session = root.path().join("state/session.json");
        let mut app = AppState::new(&Config::default(), one_track_index());
        let mut sync = StateSynchronizer::default();

        sync.sync(&app, &mut writer, Duration::ZERO)
            .expect("first sync");
        assert!(!session.exists(), "an empty queue has no checkpoint");

        app.apply(AppAction::Activate)
            .expect("queue and start the track");
        sync.sync(&app, &mut writer, Duration::from_millis(100))
            .expect("queue change");
        let saved: serde_json::Value =
            serde_json::from_slice(&fs::read(&session).expect("checkpoint")).expect("JSON");
        assert_eq!(saved["queue_entry_ids"], serde_json::json!([7]));

        // Nothing changed, so a long idle stretch must not touch the file.
        fs::remove_file(&session).expect("remove checkpoint to observe writes");
        sync.sync(&app, &mut writer, SESSION_POSITION_INTERVAL * 10)
            .expect("idle sync");
        assert!(
            modified(&session).is_none(),
            "an unchanged checkpoint is not rewritten"
        );
    }

    #[test]
    fn event_loop_errors_still_write_the_final_checkpoint() {
        let (root, _root_file, mut writer) = state_writer();
        let mut app = AppState::new(&Config::default(), one_track_index());
        app.apply(AppAction::Activate)
            .expect("queue and start the track");
        let error = finish_session(
            &mut app,
            &mut writer,
            Err(AppError::Resource("event loop fixture".into())),
            Ok(()),
        )
        .expect_err("original session error remains visible");

        assert!(error.to_string().contains("event loop fixture"));
        let saved: serde_json::Value = serde_json::from_slice(
            &fs::read(root.path().join("state/session.json")).expect("final checkpoint"),
        )
        .expect("valid final checkpoint");
        assert_eq!(saved["current_index"], 0);
    }

    #[test]
    fn terminal_guard_restores_modes_during_unwinding() {
        #[derive(Default)]
        struct State {
            bytes: Vec<u8>,
            raw_disabled: bool,
        }

        struct SharedOutput(Rc<RefCell<State>>);

        impl Write for SharedOutput {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.0.borrow_mut().bytes.extend_from_slice(bytes);
                Ok(bytes.len())
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        struct FakeControl {
            output: SharedOutput,
            state: Rc<RefCell<State>>,
        }

        impl TerminalControl for FakeControl {
            type Output = SharedOutput;

            fn output_mut(&mut self) -> &mut Self::Output {
                &mut self.output
            }

            fn disable_raw(&mut self) -> io::Result<()> {
                self.state.borrow_mut().raw_disabled = true;
                Ok(())
            }
        }

        let state = Rc::new(RefCell::new(State::default()));
        let unwind = catch_unwind(AssertUnwindSafe({
            let state = Rc::clone(&state);
            move || {
                let control = FakeControl {
                    output: SharedOutput(Rc::clone(&state)),
                    state,
                };
                let mut guard = TerminalGuard::new(control);
                guard.enter().expect("enter fake terminal");
                panic!("fixture unwind");
            }
        }));

        assert!(unwind.is_err());
        let state = state.borrow();
        assert!(state.raw_disabled);
        assert!(state.bytes.windows(8).any(|bytes| bytes == b"\x1b[?1049h"));
        assert!(state.bytes.windows(8).any(|bytes| bytes == b"\x1b[?1049l"));
        assert!(state.bytes.windows(6).any(|bytes| bytes == b"\x1b[?25h"));
    }
}
