// SPDX-License-Identifier: Apache-2.0

//! App-owned terminal state and session lifecycle.

use std::io;
use std::time::Duration;

use crate::config::Config;
use crate::errors::{AppError, AppResult};
use crate::input::{AppAction, InputState};
use crate::locks::{ActiveTuiLease, RootMutationLease};
use crate::paths::SelectedRoot;
use crate::terminal_capabilities::ImageProtocol;

const TUI_STARTUP_OPEN_FILES_PEAK: usize = 5;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Focus {
    Library,
    NowPlaying,
    Art,
    Queue,
}

impl Focus {
    fn next(self) -> Self {
        match self {
            Self::Library => Self::NowPlaying,
            Self::NowPlaying => Self::Art,
            Self::Art => Self::Queue,
            Self::Queue => Self::Library,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::Library => Self::Queue,
            Self::NowPlaying => Self::Library,
            Self::Art => Self::NowPlaying,
            Self::Queue => Self::Art,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ColorMode {
    Terminal,
    Mono,
}

impl ColorMode {
    #[must_use]
    pub fn resolve(theme: &str, no_color: bool, dumb_terminal: bool) -> Self {
        if theme == "mono" || no_color || dumb_terminal {
            Self::Mono
        } else {
            Self::Terminal
        }
    }
}

/// State owned exclusively by the event loop.
#[derive(Debug)]
pub struct AppState {
    pub focus: Focus,
    pub should_quit: bool,
    pub terminal_size: (u16, u16),
    pub image_protocol: ImageProtocol,
    pub color_mode: ColorMode,
    pub status_message: &'static str,
    pub input: InputState,
}

impl AppState {
    /// Builds the shell state from already validated configuration.
    #[must_use]
    pub fn new(config: &Config) -> Self {
        let focus = if config.default_view == "queue" {
            Focus::Queue
        } else {
            Focus::Library
        };
        let color_mode = ColorMode::resolve(
            &config.theme,
            std::env::var_os("NO_COLOR").is_some(),
            std::env::var_os("TERM").is_some_and(|value| value == "dumb"),
        );
        Self {
            focus,
            should_quit: false,
            terminal_size: (0, 0),
            image_protocol: ImageProtocol::Fallback,
            color_mode,
            status_message: "Ready",
            input: InputState::new(Duration::from_millis(config.input.leader_timeout_ms)),
        }
    }

    /// Applies one resolved action without sharing state across threads.
    pub fn apply(&mut self, action: AppAction) {
        match action {
            AppAction::Quit => self.should_quit = true,
            AppAction::FocusNext => self.focus = self.focus.next(),
            AppAction::FocusPrevious => self.focus = self.focus.previous(),
            AppAction::PlayPausePlaceholder => {
                self.status_message = "Playback is not available yet";
            }
            AppAction::PalettePlaceholder => {
                self.status_message = "The command palette is not available yet";
            }
        }
    }
}

/// Acquires the mutable-session leases, starts file logging, and owns the TUI.
///
/// # Errors
///
/// Returns a typed startup, logging, terminal, or event-loop error.
pub fn run(root: &SelectedRoot, config: &Config) -> AppResult<()> {
    reserve_startup_open_files(config.runtime.max_open_files)?;
    let _active_lease = ActiveTuiLease::acquire()?;
    let _root_lease = RootMutationLease::acquire_from(root.descriptor(), &root.path)?;
    let logging = crate::logging::initialize_from(root.descriptor(), &root.path, &config.logging)?;
    tracing::info!("terminal session starting");
    let result = crate::terminal::run(config);
    if result.is_ok() {
        tracing::info!("terminal session stopped cleanly");
    } else {
        tracing::error!("terminal session failed");
    }
    let logging = logging.finish();
    if logging.dropped_records > 0 {
        let mut stderr = io::stderr().lock();
        crate::logging::report_dropped_records(&mut stderr, logging.dropped_records)
            .map_err(|error| AppError::io("report dropped diagnostics", "terminal", error))?;
    }
    match (result, logging.writer_error()) {
        (Ok(()), None) => Ok(()),
        (Ok(()), Some(error)) | (Err(error), None) => Err(error),
        (Err(primary), Some(secondary)) => Err(AppError::Multiple {
            primary: Box::new(primary),
            secondary: Box::new(secondary),
        }),
    }
}

fn reserve_startup_open_files(limit: usize) -> AppResult<()> {
    if limit < TUI_STARTUP_OPEN_FILES_PEAK {
        return Err(AppError::InvalidConfig(format!(
            "runtime.max_open_files must be at least {TUI_STARTUP_OPEN_FILES_PEAK} for terminal startup"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{AppState, ColorMode, Focus, reserve_startup_open_files};
    use crate::config::Config;
    use crate::input::AppAction;

    #[test]
    fn focus_cycles_both_directions() {
        let mut app = AppState::new(&Config::default());
        assert_eq!(app.focus, Focus::Library);
        app.apply(AppAction::FocusNext);
        assert_eq!(app.focus, Focus::NowPlaying);
        app.apply(AppAction::FocusNext);
        assert_eq!(app.focus, Focus::Art);
        app.apply(AppAction::FocusNext);
        assert_eq!(app.focus, Focus::Queue);
        app.apply(AppAction::FocusNext);
        assert_eq!(app.focus, Focus::Library);
        app.apply(AppAction::FocusPrevious);
        assert_eq!(app.focus, Focus::Queue);
        app.apply(AppAction::FocusPrevious);
        assert_eq!(app.focus, Focus::Art);
        app.apply(AppAction::FocusPrevious);
        assert_eq!(app.focus, Focus::NowPlaying);
        app.apply(AppAction::FocusPrevious);
        assert_eq!(app.focus, Focus::Library);
    }

    #[test]
    fn mono_is_selected_without_relying_on_color_for_focus() {
        assert_eq!(
            ColorMode::resolve("terminal", false, false),
            ColorMode::Terminal
        );
        assert_eq!(ColorMode::resolve("mono", false, false), ColorMode::Mono);
        assert_eq!(ColorMode::resolve("terminal", true, false), ColorMode::Mono);
        assert_eq!(ColorMode::resolve("terminal", false, true), ColorMode::Mono);
    }

    #[test]
    fn terminal_open_file_reservation_accepts_the_exact_peak() {
        assert!(reserve_startup_open_files(5).is_ok());
        assert!(reserve_startup_open_files(4).is_err());
    }
}
