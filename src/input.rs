// SPDX-License-Identifier: Apache-2.0

//! Deterministic keybinding and leader-key resolution.

use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Actions currently understood by the terminal shell.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AppAction {
    Quit,
    FocusNext,
    FocusPrevious,
    PlayPausePlaceholder,
    PalettePlaceholder,
}

/// Input state owned by the app loop.
#[derive(Debug)]
pub struct InputState {
    leader_timeout: Duration,
    leader_started: Option<Duration>,
}

impl InputState {
    /// Creates one key resolver with the validated configured timeout.
    #[must_use]
    pub const fn new(leader_timeout: Duration) -> Self {
        Self {
            leader_timeout,
            leader_started: None,
        }
    }

    /// Resolves one key at an injected monotonic time.
    #[must_use]
    pub fn key(&mut self, key: KeyEvent, now: Duration) -> Option<AppAction> {
        if let Some(started) = self.leader_started.take() {
            if now < started.saturating_add(self.leader_timeout) && key.modifiers.is_empty() {
                match key.code {
                    KeyCode::Char('/') => return Some(AppAction::PalettePlaceholder),
                    KeyCode::Char(' ') => return Some(AppAction::PlayPausePlaceholder),
                    _ => {}
                }
            }
            return self
                .resolve_plain(key, now)
                .or(Some(AppAction::PlayPausePlaceholder));
        }
        self.resolve_plain(key, now)
    }

    /// Fires a pending plain `Space` exactly at its deadline.
    pub fn tick(&mut self, now: Duration) -> Option<AppAction> {
        let started = self.leader_started?;
        if now >= started.saturating_add(self.leader_timeout) {
            self.leader_started = None;
            Some(AppAction::PlayPausePlaceholder)
        } else {
            None
        }
    }

    /// Returns the current leader deadline for bounded event polling.
    #[must_use]
    pub fn deadline(&self) -> Option<Duration> {
        self.leader_started
            .map(|started| started.saturating_add(self.leader_timeout))
    }

    fn resolve_plain(&mut self, key: KeyEvent, now: Duration) -> Option<AppAction> {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('q') if !control => Some(AppAction::Quit),
            KeyCode::Char('c') if control => Some(AppAction::Quit),
            KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => {
                Some(AppAction::FocusPrevious)
            }
            KeyCode::BackTab => Some(AppAction::FocusPrevious),
            KeyCode::Tab => Some(AppAction::FocusNext),
            KeyCode::Char(' ') if key.modifiers.is_empty() => {
                self.leader_started = Some(now);
                None
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::{AppAction, InputState};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn space_fires_at_deadline_and_a_chord_wins_before_it() {
        let timeout = Duration::from_millis(250);
        let mut input = InputState::new(timeout);
        assert!(input.key(key(KeyCode::Char(' ')), Duration::ZERO).is_none());
        assert_eq!(
            input.tick(timeout.saturating_sub(Duration::from_millis(1))),
            None
        );
        assert_eq!(input.tick(timeout), Some(AppAction::PlayPausePlaceholder));

        assert!(
            input
                .key(key(KeyCode::Char(' ')), Duration::from_secs(1))
                .is_none()
        );
        assert_eq!(
            input.key(
                key(KeyCode::Char('/')),
                (Duration::from_secs(1) + timeout).saturating_sub(Duration::from_millis(1))
            ),
            Some(AppAction::PalettePlaceholder)
        );
        assert_eq!(input.tick(Duration::from_secs(2)), None);
    }

    #[test]
    fn focus_keys_resolve_to_one_direction() {
        let mut input = InputState::new(Duration::from_millis(250));
        assert_eq!(
            input.key(key(KeyCode::Tab), Duration::ZERO),
            Some(AppAction::FocusNext)
        );
        assert_eq!(
            input.key(key(KeyCode::BackTab), Duration::ZERO),
            Some(AppAction::FocusPrevious)
        );
    }

    #[test]
    fn invalid_and_modified_chords_resolve_once() {
        let mut input = InputState::new(Duration::from_millis(250));
        assert!(input.key(key(KeyCode::Char(' ')), Duration::ZERO).is_none());
        assert_eq!(
            input.key(key(KeyCode::Char('q')), Duration::from_millis(1)),
            Some(AppAction::Quit)
        );

        assert!(
            input
                .key(key(KeyCode::Char(' ')), Duration::from_secs(1))
                .is_none()
        );
        assert_eq!(
            input.key(
                KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT),
                Duration::from_secs(1) + Duration::from_millis(1)
            ),
            Some(AppAction::PlayPausePlaceholder)
        );
    }

    #[test]
    fn double_space_resolves_to_one_action() {
        let timeout = Duration::from_millis(250);
        let mut input = InputState::new(timeout);
        assert!(input.key(key(KeyCode::Char(' ')), Duration::ZERO).is_none());
        assert_eq!(
            input.key(key(KeyCode::Char(' ')), Duration::from_millis(1)),
            Some(AppAction::PlayPausePlaceholder)
        );
        assert_eq!(input.tick(timeout + Duration::from_millis(1)), None);
    }
}
