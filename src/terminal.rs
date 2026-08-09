// SPDX-License-Identifier: Apache-2.0

//! Terminal entry, event-loop ownership, and best-effort restoration.

use std::io::{self, Stdout, Write};

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

use crate::app::AppState;
use crate::config::{Config, UI_STATE_SCRATCH_BYTES};
use crate::errors::{AppError, AppResult};
use crate::event::{AppEvent, EventSource};

/// Runs one terminal session and restores terminal modes on every unwind path.
///
/// # Errors
///
/// Returns a terminal, capability-probe, drawing, or event-stream error.
pub fn run(config: &Config) -> AppResult<()> {
    let initial_area = current_terminal_area()?;
    let stdout = io::stdout();
    enable_raw_mode().map_err(|error| AppError::io("enable raw mode", "terminal", error))?;
    let mut guard = TerminalGuard::new(CrosstermControl { stdout });
    guard.enter()?;

    let mut terminal = create_terminal(initial_area)?;

    let probe = crate::terminal_capabilities::query(config.artwork.terminal_images)?;
    let mut app = AppState::new(config);
    apply_event(&mut app, AppEvent::Capability(probe));
    let events = EventSource::new();

    while !app.should_quit {
        terminal
            .draw(|frame| {
                app.terminal_size = (frame.area().width, frame.area().height);
                crate::ui::render(frame, &app);
            })
            .map_err(|error| AppError::io("draw terminal", "terminal", error))?;
        let leader_deadline = app.input.deadline();
        let event = events.next(leader_deadline)?;
        if let AppEvent::Resize(width, height) = event {
            let area = validate_terminal_area(width, height, UI_STATE_SCRATCH_BYTES)?;
            drop(terminal);
            execute!(guard.output_mut(), Clear(ClearType::All))
                .map_err(|error| AppError::io("clear resized terminal", "terminal", error))?;
            terminal = create_terminal(area)?;
        }
        apply_event(&mut app, event);
    }

    terminal
        .show_cursor()
        .map_err(|error| AppError::io("show terminal cursor", "terminal", error))?;
    drop(terminal);
    guard.restore()
}

fn current_terminal_area() -> AppResult<Rect> {
    let (width, height) = terminal_size()
        .map_err(|error| AppError::io("read terminal dimensions", "terminal", error))?;
    validate_terminal_area(width, height, UI_STATE_SCRATCH_BYTES)
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

fn apply_event(app: &mut AppState, event: AppEvent) {
    match event {
        AppEvent::Key(key, now) => {
            if let Some(action) = app.input.key(key, now) {
                app.apply(action);
            }
        }
        AppEvent::Resize(width, height) => app.terminal_size = (width, height),
        AppEvent::Tick(now) => {
            if let Some(action) = app.input.tick(now) {
                app.apply(action);
            }
        }
        AppEvent::Capability(probe) => {
            app.image_protocol = probe.protocol;
            app.should_quit = probe.cancelled;
        }
    }
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
    use std::io::{self, Write};
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::rc::Rc;

    use super::{TerminalControl, TerminalGuard, apply_event, validate_terminal_area};
    use crate::app::AppState;
    use crate::config::Config;
    use crate::event::AppEvent;
    use crate::terminal_capabilities::{ImageProtocol, ProbeResult};

    #[test]
    fn app_events_record_resize_and_no_reply_fallback() {
        let mut app = AppState::new(&Config::default());
        apply_event(&mut app, AppEvent::Resize(120, 32));
        apply_event(
            &mut app,
            AppEvent::Capability(ProbeResult {
                protocol: ImageProtocol::Fallback,
                timed_out: true,
                cancelled: false,
            }),
        );
        assert_eq!(app.terminal_size, (120, 32));
        assert_eq!(app.image_protocol, ImageProtocol::Fallback);
        assert!(!app.should_quit);
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
