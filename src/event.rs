// SPDX-License-Identifier: Apache-2.0

//! Bounded terminal event polling with an injected monotonic timeline.

use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyEvent, KeyEventKind};

use crate::errors::{AppError, AppResult};
const MAX_POLL: Duration = Duration::from_millis(100);

/// Events consumed by the app-owned terminal loop.
#[derive(Clone, Copy, Debug)]
pub enum AppEvent {
    Key(KeyEvent, Duration),
    Resize(u16, u16, Duration),
    Tick(Duration),
}

/// Owns the monotonic origin used by input and animation events.
pub struct EventSource {
    started: Instant,
}

impl EventSource {
    #[must_use]
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
        }
    }

    /// Waits only until the next periodic redraw tick.
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal event stream cannot be polled or read.
    pub fn next(&self) -> AppResult<AppEvent> {
        if !event::poll(MAX_POLL)
            .map_err(|error| AppError::io("poll terminal input", "terminal", error))?
        {
            return Ok(AppEvent::Tick(self.elapsed()));
        }
        let event = event::read()
            .map_err(|error| AppError::io("read terminal input", "terminal", error))?;
        let now = self.elapsed();
        Ok(match event {
            Event::Key(key) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
                AppEvent::Key(key, now)
            }
            Event::Resize(width, height) => AppEvent::Resize(width, height, now),
            _ => AppEvent::Tick(now),
        })
    }

    fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }
}

impl Default for EventSource {
    fn default() -> Self {
        Self::new()
    }
}
