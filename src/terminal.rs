// SPDX-License-Identifier: Apache-2.0

//! Terminal entry, event-loop ownership, and best-effort restoration.

use std::io::{self, Stdout, Write};
use std::os::fd::BorrowedFd;
use std::path::Path;
use std::time::Duration;

use crossterm::cursor::{Hide, Show};
use crossterm::execute;
use crossterm::terminal::{
    Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
    enable_raw_mode, size as terminal_size,
};
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::Cell;
use ratatui::layout::Rect;
use ratatui::{Terminal, TerminalOptions, Viewport};

use crate::app::{AppState, PlaybackIntent};
use crate::audio::{AudioCommand, AudioDiagnostics, AudioEvent, AudioRuntime};
use crate::config::{Config, TERMINAL_BUFFER_BYTES};
use crate::errors::{AppError, AppResult};
use crate::event::{AppEvent, EventSource};
use crate::model::ScanIndex;
use crate::mpris::{MprisProjection, MprisRuntime, REQUEST_CAPACITY};

const STATE_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);

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
    let initial_area = current_terminal_area()?;
    let mut app = AppState::new(config, index)?;
    let mut state_store = crate::state::StateStore::open(root, root_path)?;
    match state_store.read_session(&config.queue)? {
        crate::state::SessionLoad::Missing => {}
        crate::state::SessionLoad::Loaded(snapshot) => app.restore_session(&snapshot)?,
        crate::state::SessionLoad::Ignored(reason) => app.session_ignored(&reason),
    }
    let mut state_sync = StateSynchronizer::default();
    state_sync.sync(&app, &mut state_store, Duration::ZERO)?;
    let audio = AudioRuntime::start()?;
    let mpris = match MprisRuntime::start(MprisProjection::from_app(&app)) {
        Ok(runtime) => Some(runtime),
        Err(error) => {
            tracing::warn!(%error, "desktop controls unavailable");
            app.desktop_controls_unavailable(&error.to_string());
            None
        }
    };
    let session_result = run_terminal_session(
        root,
        initial_area,
        &mut app,
        &audio,
        mpris.as_ref(),
        &mut state_store,
        &mut state_sync,
    );
    let audio_result = audio.shutdown();
    app.session_stopped();
    let mpris_result = mpris.map_or(Ok(()), |runtime| {
        runtime
            .publish(MprisProjection::from_app(&app))
            .and_then(|()| runtime.shutdown())
    });
    finish_session(
        &mut app,
        &mut state_store,
        &mut state_sync,
        session_result,
        combine_results(audio_result, mpris_result),
    )
}

fn finish_session(
    app: &mut AppState,
    state_store: &mut crate::state::StateStore,
    state_sync: &mut StateSynchronizer,
    session_result: AppResult<()>,
    audio_result: AppResult<()>,
) -> AppResult<()> {
    app.session_stopped();
    let state_result = state_sync.force(app, state_store);
    combine_results(combine_results(session_result, audio_result), state_result)
}

fn run_terminal_session(
    root: BorrowedFd<'_>,
    initial_area: Rect,
    app: &mut AppState,
    audio: &AudioRuntime,
    mpris: Option<&MprisRuntime>,
    state_store: &mut crate::state::StateStore,
    state_sync: &mut StateSynchronizer,
) -> AppResult<()> {
    let stdout = io::stdout();
    enable_raw_mode().map_err(|error| AppError::io("enable raw mode", "terminal", error))?;
    let mut guard = TerminalGuard::new(CrosstermControl { stdout });
    guard.enter()?;

    let mut terminal = create_terminal(initial_area)?;

    let events = EventSource::new();
    let mut event_time = Duration::ZERO;
    let mut audio_diagnostics = AudioDiagnosticsReporter::default();

    while !app.should_quit {
        drain_audio_events(app, root, audio)?;
        audio_diagnostics.report(audio.diagnostics(), event_time);
        drain_mpris_actions(app, root, audio, mpris)?;
        publish_mpris(app, mpris)?;
        state_sync.sync(app, state_store, event_time)?;
        terminal
            .draw(|frame| {
                app.terminal_size = (frame.area().width, frame.area().height);
                crate::ui::render(frame, app, event_time);
            })
            .map_err(|error| AppError::io("draw terminal", "terminal", error))?;
        let event = events.next()?;
        event_time = event_time_for(event);
        if let AppEvent::Resize(width, height, _) = event {
            let area = validate_terminal_area(width, height, TERMINAL_BUFFER_BYTES)?;
            drop(terminal);
            execute!(guard.output_mut(), Clear(ClearType::All))
                .map_err(|error| AppError::io("clear resized terminal", "terminal", error))?;
            terminal = create_terminal(area)?;
        }
        if let Some(intent) = apply_event(app, event) {
            dispatch_playback(app, root, audio, intent)?;
        }
        publish_mpris(app, mpris)?;
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

fn publish_mpris(app: &AppState, mpris: Option<&MprisRuntime>) -> AppResult<()> {
    mpris.map_or(Ok(()), |runtime| {
        runtime.publish(MprisProjection::from_app(app))
    })
}

#[derive(Default)]
struct StateSynchronizer {
    previous_now_playing: Option<crate::state::NowPlayingProjection>,
    last_now_playing_write: Option<Duration>,
    session_initialized: bool,
    previous_session: Option<crate::app::SessionIdentity>,
    last_session_write: Option<Duration>,
}

impl StateSynchronizer {
    fn sync(
        &mut self,
        app: &AppState,
        store: &mut crate::state::StateStore,
        now: Duration,
    ) -> AppResult<()> {
        let projection = crate::state::NowPlayingProjection::from_app(app);
        let changed = self.previous_now_playing.as_ref() != Some(&projection);
        let heartbeat_due = self
            .last_now_playing_write
            .is_none_or(|last| now.saturating_sub(last) >= STATE_HEARTBEAT_INTERVAL);
        if changed || heartbeat_due {
            store.write_now_playing(&projection)?;
            self.previous_now_playing = Some(projection);
            self.last_now_playing_write = Some(now);
        }

        let identity = app.session_identity();
        let session_changed = !self.session_initialized || self.previous_session != identity;
        let checkpoint_due = identity.is_some()
            && self
                .last_session_write
                .is_none_or(|last| now.saturating_sub(last) >= STATE_HEARTBEAT_INTERVAL);
        if session_changed || checkpoint_due {
            write_session(app, store)?;
            self.session_initialized = true;
            self.previous_session = identity;
            self.last_session_write = identity.map(|_| now);
        }
        Ok(())
    }

    fn force(&mut self, app: &AppState, store: &mut crate::state::StateStore) -> AppResult<()> {
        let projection = crate::state::NowPlayingProjection::from_app(app);
        let now_playing_result = store.write_now_playing(&projection);
        if now_playing_result.is_ok() {
            self.previous_now_playing = Some(projection);
        }
        let session_result = write_session(app, store);
        combine_results(now_playing_result, session_result)
    }
}

fn write_session(app: &AppState, store: &mut crate::state::StateStore) -> AppResult<()> {
    match app.session_snapshot()? {
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

fn current_terminal_area() -> AppResult<Rect> {
    let (width, height) = terminal_size()
        .map_err(|error| AppError::io("read terminal dimensions", "terminal", error))?;
    validate_terminal_area(width, height, TERMINAL_BUFFER_BYTES)
}

fn create_terminal(area: Rect) -> AppResult<Terminal<CrosstermBackend<Stdout>>> {
    Terminal::with_options(
        CrosstermBackend::new(io::stdout()),
        TerminalOptions {
            viewport: Viewport::Fixed(area),
        },
    )
    .map_err(|error| AppError::io("create terminal", "terminal", error))
}

fn validate_terminal_area(width: u16, height: u16, budget: usize) -> AppResult<Rect> {
    let bytes = usize::from(width)
        .checked_mul(usize::from(height))
        .and_then(|cells| cells.checked_mul(std::mem::size_of::<Cell>()))
        .and_then(|one_buffer| one_buffer.checked_mul(2))
        .ok_or_else(|| {
            AppError::io(
                "validate terminal dimensions",
                "terminal",
                io::Error::new(io::ErrorKind::InvalidInput, "terminal buffer size overflow"),
            )
        })?;
    if bytes > budget {
        return Err(AppError::io(
            "validate terminal dimensions",
            "terminal",
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "{width}x{height} requires {bytes} bytes for terminal buffers, above the {budget}-byte UI reservation"
                ),
            ),
        ));
    }
    Ok(Rect::new(0, 0, width, height))
}

fn apply_event(app: &mut AppState, event: AppEvent) -> Option<PlaybackIntent> {
    match event {
        AppEvent::Key(key, now) => app.key(key, now),
        AppEvent::Resize(width, height, now) => {
            app.advance_time(now);
            app.terminal_size = (width, height);
            None
        }
        AppEvent::Tick(now) => {
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
    while let Some(position) = audio.try_position()? {
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
/// ongoing dropout would otherwise warn ten times a second and rotate the bounded
/// log files clean of every other record.
const DIAGNOSTICS_REPORT_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Default)]
struct AudioDiagnosticsReporter {
    reported: AudioDiagnostics,
    last_report: Option<Duration>,
    realtime_denial_logged: bool,
}

impl AudioDiagnosticsReporter {
    /// Logs the counters that grew since the last report, at most once per
    /// [`DIAGNOSTICS_REPORT_INTERVAL`]. The interval only advances when something
    /// was actually logged, so a quiet counter cannot delay a real dropout report.
    fn report(&mut self, current: AudioDiagnostics, now: Duration) {
        if current == self.reported {
            return;
        }
        let due = self
            .last_report
            .is_none_or(|last| now.saturating_sub(last) >= DIAGNOSTICS_REPORT_INTERVAL);
        if !due {
            return;
        }
        let mut emitted = false;

        let underruns = current
            .underrun_samples
            .saturating_sub(self.reported.underrun_samples);
        if underruns != 0 {
            tracing::warn!(
                samples = underruns,
                total_samples = current.underrun_samples,
                "audio output ring underrun"
            );
            emitted = true;
        }
        let xruns = current.xruns.saturating_sub(self.reported.xruns);
        if xruns != 0 {
            tracing::warn!(
                count = xruns,
                total = current.xruns,
                "audio backend reported an xrun"
            );
            emitted = true;
        }
        // The backend reroutes the stream itself, so this is expected, not a fault.
        let device_changes = current
            .device_changes
            .saturating_sub(self.reported.device_changes);
        if device_changes != 0 {
            tracing::info!(
                count = device_changes,
                total = current.device_changes,
                "audio output was rerouted to a new default device"
            );
            // A new route can mean a different backend, where a refusal is no longer
            // the harmless redundant one described below. Let it be said again.
            self.realtime_denial_logged = false;
            emitted = true;
        }
        // Real-time promotion is a property of the audio path rather than an incident:
        // it either succeeds when a stream starts or it does not. PipeWire promotes its
        // own data thread and then refuses CPAL's redundant attempt on every callback,
        // so this is said once per path and the counter accrues in silence.
        let denials = current
            .realtime_denied
            .saturating_sub(self.reported.realtime_denied);
        if !self.realtime_denial_logged && denials != 0 {
            tracing::warn!(
                total = current.realtime_denied,
                "audio thread real-time promotion was refused; the backend may already \
                 schedule its own audio thread"
            );
            self.realtime_denial_logged = true;
            emitted = true;
        }

        self.reported = current;
        if emitted {
            self.last_report = Some(now);
        }
    }
}

fn dispatch_playback(
    app: &mut AppState,
    root: BorrowedFd<'_>,
    audio: &AudioRuntime,
    intent: PlaybackIntent,
) -> AppResult<()> {
    let command = match intent {
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
                Err(error) => {
                    let _ = app.audio_event(AudioEvent::Failed {
                        generation,
                        message: error.to_string(),
                    });
                    return Ok(());
                }
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
    };
    audio.send(command)
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

    fn output_mut(&mut self) -> &mut C::Output {
        self.control.output_mut()
    }

    fn enter(&mut self) -> AppResult<()> {
        // A partial escape write still needs the matching best-effort cleanup.
        self.alternate = true;
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
        STATE_HEARTBEAT_INTERVAL, StateSynchronizer, TerminalControl, TerminalGuard, apply_event,
        finish_session, validate_terminal_area,
    };
    use crate::app::{AppState, PlaybackStatus};
    use crate::config::Config;
    use crate::errors::AppError;
    use crate::event::AppEvent;
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
    fn a_persistent_real_time_denial_is_reported_only_once() {
        let mut reporter = AudioDiagnosticsReporter::default();
        let mut counters = AudioDiagnostics {
            realtime_denied: 50,
            ..AudioDiagnostics::default()
        };

        reporter.report(counters, Duration::ZERO);
        assert!(reporter.realtime_denial_logged, "the first denial is said");
        assert_eq!(reporter.last_report, Some(Duration::ZERO));

        // PipeWire refuses CPAL's redundant promotion on every callback, so the
        // counter keeps climbing. That must stay silent, and it must not consume the
        // report interval, or a real dropout would be left waiting behind it.
        counters.realtime_denied = 5_000;
        reporter.report(counters, DIAGNOSTICS_REPORT_INTERVAL * 2);

        assert_eq!(reporter.last_report, Some(Duration::ZERO));
        assert_eq!(reporter.reported.realtime_denied, 5_000);
    }

    #[test]
    fn a_rerouted_device_lets_a_real_time_denial_be_said_again() {
        let mut reporter = AudioDiagnosticsReporter::default();
        let mut counters = AudioDiagnostics {
            realtime_denied: 10,
            ..AudioDiagnostics::default()
        };
        reporter.report(counters, Duration::ZERO);
        assert!(reporter.realtime_denial_logged);

        // The route moved, so the next refusal may come from a different backend
        // where it is a real cause of dropouts rather than a redundant attempt.
        counters.device_changes = 1;
        reporter.report(counters, DIAGNOSTICS_REPORT_INTERVAL);
        assert!(!reporter.realtime_denial_logged, "the latch is re-armed");

        counters.realtime_denied = 11;
        reporter.report(counters, DIAGNOSTICS_REPORT_INTERVAL * 2);
        assert!(reporter.realtime_denial_logged, "a new refusal is said");
    }

    #[test]
    fn a_dropout_is_reported_even_after_a_silent_denial_flood() {
        let mut reporter = AudioDiagnosticsReporter::default();
        let mut counters = AudioDiagnostics {
            realtime_denied: 1,
            ..AudioDiagnostics::default()
        };
        reporter.report(counters, Duration::ZERO);
        counters.realtime_denied = 9_000;
        reporter.report(counters, DIAGNOSTICS_REPORT_INTERVAL);

        counters.underrun_samples = 128;
        reporter.report(counters, DIAGNOSTICS_REPORT_INTERVAL);

        assert_eq!(reporter.reported.underrun_samples, 128);
        assert_eq!(reporter.last_report, Some(DIAGNOSTICS_REPORT_INTERVAL));
    }

    #[test]
    fn app_events_record_resize() {
        let mut app =
            AppState::new(&Config::default(), empty_index()).expect("app state reservation");
        apply_event(&mut app, AppEvent::Resize(120, 32, Duration::ZERO));
        assert_eq!(app.terminal_size, (120, 32));
        assert!(!app.should_quit);
    }

    #[test]
    fn periodic_ticks_clear_expired_information_notices() {
        let mut app =
            AppState::new(&Config::default(), empty_index()).expect("app state reservation");
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
        let mut app =
            AppState::new(&Config::default(), empty_index()).expect("app state reservation");
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

    #[test]
    fn unchanged_state_is_refreshed_on_the_bounded_heartbeat() {
        let (_root, _root_file, mut writer) = state_writer();
        let app = AppState::new(&Config::default(), empty_index()).expect("app state");
        let mut sync = StateSynchronizer::default();

        sync.sync(&app, &mut writer, Duration::ZERO)
            .expect("initial state");
        sync.sync(
            &app,
            &mut writer,
            STATE_HEARTBEAT_INTERVAL.saturating_sub(Duration::from_millis(1)),
        )
        .expect("state before heartbeat");
        assert_eq!(sync.last_now_playing_write, Some(Duration::ZERO));

        sync.sync(&app, &mut writer, STATE_HEARTBEAT_INTERVAL)
            .expect("heartbeat state");
        assert_eq!(sync.last_now_playing_write, Some(STATE_HEARTBEAT_INTERVAL));
    }

    #[test]
    fn event_loop_errors_still_publish_stopped_state() {
        let (root, _root_file, mut writer) = state_writer();
        let mut app = AppState::new(&Config::default(), empty_index()).expect("app state");
        app.playback_status = PlaybackStatus::Playing;
        let mut sync = StateSynchronizer::default();
        sync.sync(&app, &mut writer, Duration::ZERO)
            .expect("active state");

        let error = finish_session(
            &mut app,
            &mut writer,
            &mut sync,
            Err(AppError::Resource("event loop fixture".into())),
            Ok(()),
        )
        .expect_err("original session error remains visible");

        assert!(error.to_string().contains("event loop fixture"));
        let state: serde_json::Value = serde_json::from_slice(
            &fs::read(root.path().join("state/now-playing.json")).expect("final state"),
        )
        .expect("valid final state");
        assert_eq!(state["status"], "stopped");
    }

    #[test]
    fn terminal_buffers_accept_the_exact_budget_and_refuse_one_byte_less() {
        let two_cells = std::mem::size_of::<ratatui::buffer::Cell>() * 2;
        assert!(validate_terminal_area(1, 1, two_cells).is_ok());
        assert!(validate_terminal_area(1, 1, two_cells - 1).is_err());
        assert!(validate_terminal_area(u16::MAX, u16::MAX, two_cells).is_err());
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
