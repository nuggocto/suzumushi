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
    MovePrevious,
    MoveNext,
    MoveFirst,
    MoveLast,
    Activate,
    SearchOpen,
    QueueRemove,
    QueueClear,
    QueueMoveUp,
    QueueMoveDown,
    PlayPause,
    Stop,
    Next,
    Previous,
    SeekBackward,
    SeekForward,
    VolumeDown,
    VolumeUp,
    ToggleMute,
    ToggleShuffle,
    CycleRepeat,
    SpeedDown,
    SpeedUp,
    SpeedNormal,
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
                    KeyCode::Char(' ') => return Some(AppAction::PlayPause),
                    _ => {}
                }
            }
            return self.resolve_plain(key, now).or(Some(AppAction::PlayPause));
        }
        self.resolve_plain(key, now)
    }

    /// Fires a pending plain `Space` exactly at its deadline.
    pub fn tick(&mut self, now: Duration) -> Option<AppAction> {
        let started = self.leader_started?;
        if now >= started.saturating_add(self.leader_timeout) {
            self.leader_started = None;
            Some(AppAction::PlayPause)
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
        let shifted = key.modifiers == KeyModifiers::SHIFT;
        match key.code {
            KeyCode::Char('q') if !control => Some(AppAction::Quit),
            KeyCode::Char('c') if control => Some(AppAction::Quit),
            KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => {
                Some(AppAction::FocusPrevious)
            }
            KeyCode::BackTab => Some(AppAction::FocusPrevious),
            KeyCode::Tab => Some(AppAction::FocusNext),
            KeyCode::Up | KeyCode::Char('k') if key.modifiers.is_empty() => {
                Some(AppAction::MovePrevious)
            }
            KeyCode::Down | KeyCode::Char('j') if key.modifiers.is_empty() => {
                Some(AppAction::MoveNext)
            }
            KeyCode::Home | KeyCode::Char('g') if key.modifiers.is_empty() => {
                Some(AppAction::MoveFirst)
            }
            KeyCode::End if key.modifiers.is_empty() => Some(AppAction::MoveLast),
            KeyCode::Char('G') if key.modifiers.is_empty() || shifted => Some(AppAction::MoveLast),
            KeyCode::Enter if key.modifiers.is_empty() => Some(AppAction::Activate),
            KeyCode::Char('/') if key.modifiers.is_empty() => Some(AppAction::SearchOpen),
            KeyCode::Delete | KeyCode::Char('d') if key.modifiers.is_empty() => {
                Some(AppAction::QueueRemove)
            }
            KeyCode::Char('c') if key.modifiers.is_empty() => Some(AppAction::QueueClear),
            KeyCode::Char('K') if key.modifiers.is_empty() || shifted => {
                Some(AppAction::QueueMoveUp)
            }
            KeyCode::Char('J') if key.modifiers.is_empty() || shifted => {
                Some(AppAction::QueueMoveDown)
            }
            KeyCode::Char('s') if key.modifiers.is_empty() => Some(AppAction::Stop),
            KeyCode::Char('n') if key.modifiers.is_empty() => Some(AppAction::Next),
            KeyCode::Char('p') if key.modifiers.is_empty() => Some(AppAction::Previous),
            KeyCode::Left if key.modifiers.is_empty() => Some(AppAction::SeekBackward),
            KeyCode::Right if key.modifiers.is_empty() => Some(AppAction::SeekForward),
            KeyCode::Char('-') if key.modifiers.is_empty() => Some(AppAction::VolumeDown),
            KeyCode::Char('+' | '=') if key.modifiers.is_empty() || shifted => {
                Some(AppAction::VolumeUp)
            }
            KeyCode::Char('m') if key.modifiers.is_empty() => Some(AppAction::ToggleMute),
            KeyCode::Char('x') if key.modifiers.is_empty() => Some(AppAction::ToggleShuffle),
            KeyCode::Char('r') if key.modifiers.is_empty() => Some(AppAction::CycleRepeat),
            KeyCode::Char('[') if key.modifiers.is_empty() => Some(AppAction::SpeedDown),
            KeyCode::Char(']') if key.modifiers.is_empty() => Some(AppAction::SpeedUp),
            KeyCode::Char('0') if key.modifiers.is_empty() => Some(AppAction::SpeedNormal),
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
        assert_eq!(input.tick(timeout), Some(AppAction::PlayPause));

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
            Some(AppAction::PlayPause)
        );
    }

    #[test]
    fn double_space_resolves_to_one_action() {
        let timeout = Duration::from_millis(250);
        let mut input = InputState::new(timeout);
        assert!(input.key(key(KeyCode::Char(' ')), Duration::ZERO).is_none());
        assert_eq!(
            input.key(key(KeyCode::Char(' ')), Duration::from_millis(1)),
            Some(AppAction::PlayPause)
        );
        assert_eq!(input.tick(timeout + Duration::from_millis(1)), None);
    }

    #[test]
    fn browser_search_and_queue_keys_resolve_to_one_action() {
        let mut input = InputState::new(Duration::from_millis(250));
        let cases = [
            (key(KeyCode::Char('/')), AppAction::SearchOpen),
            (key(KeyCode::Down), AppAction::MoveNext),
            (key(KeyCode::Enter), AppAction::Activate),
            (key(KeyCode::Char('d')), AppAction::QueueRemove),
            (key(KeyCode::Char('c')), AppAction::QueueClear),
            (
                KeyEvent::new(KeyCode::Char('J'), KeyModifiers::SHIFT),
                AppAction::QueueMoveDown,
            ),
            (
                KeyEvent::new(KeyCode::Char('K'), KeyModifiers::SHIFT),
                AppAction::QueueMoveUp,
            ),
        ];
        for (event, expected) in cases {
            assert_eq!(input.key(event, Duration::ZERO), Some(expected));
        }
    }

    #[test]
    fn playback_control_keys_resolve_to_the_documented_steps() {
        let mut input = InputState::new(Duration::from_millis(250));
        let cases = [
            (key(KeyCode::Left), AppAction::SeekBackward),
            (key(KeyCode::Right), AppAction::SeekForward),
            (key(KeyCode::Char('-')), AppAction::VolumeDown),
            (key(KeyCode::Char('+')), AppAction::VolumeUp),
            (key(KeyCode::Char('m')), AppAction::ToggleMute),
            (key(KeyCode::Char('x')), AppAction::ToggleShuffle),
            (key(KeyCode::Char('r')), AppAction::CycleRepeat),
            (key(KeyCode::Char('[')), AppAction::SpeedDown),
            (key(KeyCode::Char(']')), AppAction::SpeedUp),
            (key(KeyCode::Char('0')), AppAction::SpeedNormal),
        ];
        for (event, expected) in cases {
            assert_eq!(input.key(event, Duration::ZERO), Some(expected));
        }
    }
}
