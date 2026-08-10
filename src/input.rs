// SPDX-License-Identifier: Apache-2.0

//! Deterministic terminal keybinding resolution.

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
    ToggleHelp,
    QueueRemove,
    QueueClear,
    QueueMoveUp,
    QueueMoveDown,
    PlayPause,
    Play,
    Pause,
    Stop,
    Next,
    Previous,
    SeekBackward,
    SeekForward,
    SeekRelative {
        forward: bool,
        distance: Duration,
    },
    SeekAbsolute {
        playback_generation: u64,
        queue_instance: u64,
        position: Duration,
    },
    VolumeDown,
    VolumeUp,
    SetVolume(u8),
    ToggleMute,
    ToggleShuffle,
    SetShuffle(bool),
    CycleRepeat,
    RepeatOff,
    RepeatAll,
    RepeatOne,
}

/// Resolves one normal-mode key without delaying terminal input.
#[must_use]
pub fn resolve(key: KeyEvent) -> Option<AppAction> {
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    let shifted = key.modifiers == KeyModifiers::SHIFT;
    match key.code {
        KeyCode::Char('q') if key.modifiers.is_empty() => Some(AppAction::Quit),
        KeyCode::Char('c') if control => Some(AppAction::Quit),
        KeyCode::Tab if shifted => Some(AppAction::FocusPrevious),
        KeyCode::BackTab => Some(AppAction::FocusPrevious),
        KeyCode::Tab if key.modifiers.is_empty() => Some(AppAction::FocusNext),
        KeyCode::Up | KeyCode::Char('k') if key.modifiers.is_empty() => {
            Some(AppAction::MovePrevious)
        }
        KeyCode::Down | KeyCode::Char('j') if key.modifiers.is_empty() => Some(AppAction::MoveNext),
        KeyCode::Home | KeyCode::Char('g') if key.modifiers.is_empty() => {
            Some(AppAction::MoveFirst)
        }
        KeyCode::End if key.modifiers.is_empty() => Some(AppAction::MoveLast),
        KeyCode::Char('G') if key.modifiers.is_empty() || shifted => Some(AppAction::MoveLast),
        KeyCode::Enter if key.modifiers.is_empty() => Some(AppAction::Activate),
        KeyCode::Char('/') if key.modifiers.is_empty() => Some(AppAction::SearchOpen),
        KeyCode::Char('?') if key.modifiers.is_empty() || shifted => Some(AppAction::ToggleHelp),
        KeyCode::Delete | KeyCode::Char('d') if key.modifiers.is_empty() => {
            Some(AppAction::QueueRemove)
        }
        KeyCode::Char('c') if key.modifiers.is_empty() => Some(AppAction::QueueClear),
        KeyCode::Char('K') if key.modifiers.is_empty() || shifted => Some(AppAction::QueueMoveUp),
        KeyCode::Char('J') if key.modifiers.is_empty() || shifted => Some(AppAction::QueueMoveDown),
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
        KeyCode::Char(' ') if key.modifiers.is_empty() => Some(AppAction::PlayPause),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::{AppAction, resolve};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn space_and_help_resolve_immediately() {
        assert_eq!(resolve(key(KeyCode::Char(' '))), Some(AppAction::PlayPause));
        assert_eq!(
            resolve(key(KeyCode::Char('?'))),
            Some(AppAction::ToggleHelp)
        );
    }

    #[test]
    fn focus_keys_resolve_to_one_direction() {
        assert_eq!(resolve(key(KeyCode::Tab)), Some(AppAction::FocusNext));
        assert_eq!(
            resolve(key(KeyCode::BackTab)),
            Some(AppAction::FocusPrevious)
        );
    }

    #[test]
    fn modified_keys_do_not_trigger_plain_actions() {
        assert_eq!(
            resolve(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT)),
            None
        );
    }

    #[test]
    fn browser_search_and_queue_keys_resolve_to_one_action() {
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
            assert_eq!(resolve(event), Some(expected));
        }
    }

    #[test]
    fn playback_control_keys_resolve_to_the_documented_steps() {
        let cases = [
            (key(KeyCode::Left), AppAction::SeekBackward),
            (key(KeyCode::Right), AppAction::SeekForward),
            (key(KeyCode::Char('-')), AppAction::VolumeDown),
            (key(KeyCode::Char('+')), AppAction::VolumeUp),
            (key(KeyCode::Char('m')), AppAction::ToggleMute),
            (key(KeyCode::Char('x')), AppAction::ToggleShuffle),
            (key(KeyCode::Char('r')), AppAction::CycleRepeat),
        ];
        for (event, expected) in cases {
            assert_eq!(resolve(event), Some(expected));
        }
    }
}
