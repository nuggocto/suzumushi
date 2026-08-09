// SPDX-License-Identifier: Apache-2.0

//! Bounded terminal event polling with an injected monotonic timeline.

use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyEvent, KeyEventKind};

use crate::errors::{AppError, AppResult};
use crate::terminal_capabilities::ProbeResult;

const MAX_POLL: Duration = Duration::from_millis(100);

/// Events consumed by the app-owned terminal loop.
#[derive(Clone, Copy, Debug)]
pub enum AppEvent {
    Key(KeyEvent, Duration),
    Resize(u16, u16),
    Tick(Duration),
    Capability(ProbeResult),
}

/// Owns the monotonic origin used by leader-key deadlines.
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

    /// Waits only until the next periodic tick or pending leader deadline.
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal event stream cannot be polled or read.
    pub fn next(&self, leader_deadline: Option<Duration>) -> AppResult<AppEvent> {
        let now = self.elapsed();
        let deadline_wait =
            leader_deadline.map_or(MAX_POLL, |deadline| deadline.saturating_sub(now));
        let wait = MAX_POLL.min(deadline_wait);
        if !event::poll(wait)
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
            Event::Resize(width, height) => AppEvent::Resize(width, height),
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
