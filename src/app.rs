// SPDX-License-Identifier: Apache-2.0

//! App-owned library, search, queue, terminal state, and session lifecycle.

use std::ffi::OsStr;
use std::io;
use std::mem::size_of;
use std::os::unix::ffi::OsStrExt;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::audio::{AudioFormat, AudioSpectrum, PlaybackSettings};
use crate::config::{Config, UI_STATE_SCRATCH_BYTES};
use crate::display::{bounded_text, terminal_safe};
use crate::errors::{AppError, AppResult};
use crate::input::AppAction;
use crate::locks::{ActiveTuiLease, RootWriterLease};
use crate::model::{ScanIndex, TrackEntry};
use crate::paths::SelectedRoot;

mod browser;
mod playback;
mod queue;

use playback::PlaybackState;

pub(crate) use queue::QueueItem;
use queue::{QueueError, QueueState};

pub(crate) use browser::{BrowserRow, normal_component};
use browser::{BrowserState, browser_reservation, search_reservation_bytes};

const TUI_STARTUP_OPEN_FILES_PEAK: usize = 6;
const INFORMATION_NOTICE_LIFETIME: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Focus {
    Library,
    Player,
    Queue,
}

impl Focus {
    fn next(self) -> Self {
        match self {
            Self::Library => Self::Player,
            Self::Player => Self::Queue,
            Self::Queue => Self::Library,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::Library => Self::Queue,
            Self::Player => Self::Library,
            Self::Queue => Self::Player,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ColorMode {
    Terminal,
    Mono,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StatusKind {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Overlay {
    None,
    Help,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlaybackStatus {
    Stopped,
    Loading,
    Playing,
    Paused,
    Error,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RepeatMode {
    Off,
    All,
    One,
}

impl RepeatMode {
    fn next(self) -> Self {
        match self {
            Self::Off => Self::All,
            Self::All => Self::One,
            Self::One => Self::Off,
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::All => "queue",
            Self::One => "one",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum PlaybackIntent {
    Load {
        generation: u64,
        item: QueueItem,
        position: Duration,
        settings: PlaybackSettings,
        paused: bool,
    },
    Pause {
        generation: u64,
    },
    Resume {
        generation: u64,
    },
    Stop {
        generation: u64,
    },
    SetGain {
        generation: u64,
        volume_percent: u8,
        muted: bool,
    },
    Seek {
        generation: u64,
        position: Duration,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SessionIdentity {
    queue_generation: u64,
    current_index: usize,
    current_instance_id: u64,
}

/// State owned exclusively by the event loop.
#[derive(Debug)]
pub struct AppState {
    pub focus: Focus,
    pub should_quit: bool,
    overlay: Overlay,
    pub terminal_size: (u16, u16),
    pub color_mode: ColorMode,
    pub status_message: String,
    pub(crate) status_kind: StatusKind,
    event_time: Duration,
    status_expires_at: Option<Duration>,
    pub index: ScanIndex,
    browser: BrowserState,
    queue: QueueState,
    pub volume_percent: u8,
    pub muted: bool,
    pub repeat: RepeatMode,
    status_text_max_bytes: usize,
    playback: PlaybackState,
}

impl AppState {
    /// Builds app-owned browser and queue state from one scan request.
    ///
    /// # Errors
    ///
    /// Returns an error when a validated bounded reservation cannot be made.
    pub fn new(config: &Config, index: ScanIndex) -> AppResult<Self> {
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

        let (browser_capacity, browser_bytes) = browser_reservation(&index)?;
        let search_bytes = search_reservation_bytes(config)?;
        let shuffle_bytes = config
            .queue
            .max_items
            .checked_mul(size_of::<u64>())
            .ok_or_else(|| AppError::Resource("shuffle reservation overflow".into()))?;
        let queued_asset_bytes = index
            .assets
            .len()
            .checked_mul(size_of::<u8>())
            .ok_or_else(|| AppError::Resource("queued asset reservation overflow".into()))?;
        if browser_bytes
            .saturating_add(search_bytes)
            .saturating_add(shuffle_bytes)
            .saturating_add(queued_asset_bytes)
            > UI_STATE_SCRATCH_BYTES
        {
            return Err(AppError::InvalidConfig(
                "library browser, search, queue identity, and shuffle reservations exceed the UI/state scratch budget"
                    .into(),
            ));
        }

        let browser = BrowserState::new(&index, config, browser_capacity)?;
        let queue = QueueState::new(config, index.assets.len())?;

        let (status_message, status_kind) = if index.complete {
            (
                format!(
                    "Ready: {} tracks, {} playlists",
                    index.entries.len(),
                    index.playlists.len()
                ),
                StatusKind::Info,
            )
        } else {
            (
                format!(
                    "Scan incomplete: {} tracks, {} warnings; run diagnose for details",
                    index.entries.len(),
                    index.warnings.len()
                ),
                StatusKind::Warning,
            )
        };

        Ok(Self {
            focus,
            should_quit: false,
            overlay: Overlay::None,
            terminal_size: (0, 0),
            color_mode,
            status_message,
            status_kind,
            event_time: Duration::ZERO,
            status_expires_at: None,
            index,
            browser,
            queue,
            volume_percent: 100,
            muted: false,
            repeat: RepeatMode::Off,
            status_text_max_bytes: config.runtime.status_text_max_bytes,
            playback: PlaybackState::new(),
        })
    }

    #[must_use]
    pub const fn playback_status(&self) -> PlaybackStatus {
        self.playback.status
    }

    #[must_use]
    pub const fn playback_generation(&self) -> u64 {
        self.playback.generation
    }

    #[cfg(test)]
    pub(crate) fn set_playback_status(&mut self, status: PlaybackStatus) {
        self.playback.status = status;
    }

    pub(crate) fn queue(&self) -> &[QueueItem] {
        self.queue.items()
    }

    pub(crate) const fn queue_selection(&self) -> usize {
        self.queue.selection
    }

    #[must_use]
    pub const fn shuffle_enabled(&self) -> bool {
        self.queue.shuffle
    }

    pub(crate) fn search(&self) -> &browser::SearchState {
        &self.browser.search
    }

    pub(crate) const fn library_selection(&self) -> usize {
        self.browser.selection
    }

    /// Resolves a key against the active search mode or normal key map.
    pub(crate) fn key(&mut self, key: KeyEvent, now: Duration) -> Option<PlaybackIntent> {
        self.advance_time(now);
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.should_quit = true;
            return None;
        }
        if self.overlay == Overlay::Help {
            match key.code {
                KeyCode::Char('q') if key.modifiers.is_empty() => self.should_quit = true,
                KeyCode::Esc | KeyCode::Char('?') => {
                    self.overlay = Overlay::None;
                    self.set_status("Help closed");
                }
                _ => {}
            }
            return None;
        }
        if self.browser.search.active {
            self.search_key(key)
        } else if let Some(action) = crate::input::resolve(key) {
            self.apply(action)
        } else {
            None
        }
    }

    /// Applies one resolved non-modal action without sharing state across threads.
    pub(crate) fn apply(&mut self, action: AppAction) -> Option<PlaybackIntent> {
        match action {
            AppAction::Quit => self.should_quit = true,
            AppAction::FocusNext => self.focus = self.focus.next(),
            AppAction::FocusPrevious => self.focus = self.focus.previous(),
            AppAction::MovePrevious => self.move_selection(false),
            AppAction::MoveNext => self.move_selection(true),
            AppAction::MoveFirst => self.move_to_edge(false),
            AppAction::MoveLast => self.move_to_edge(true),
            AppAction::Activate => return self.activate(),
            AppAction::SearchOpen => self.open_search(),
            AppAction::ToggleHelp => {
                self.overlay = if self.overlay == Overlay::Help {
                    Overlay::None
                } else {
                    Overlay::Help
                };
                self.set_status(if self.overlay == Overlay::Help {
                    "Help opened; press ? or Esc to close"
                } else {
                    "Help closed"
                });
            }
            AppAction::QueueRemove => self.remove_queue_item(),
            AppAction::QueueClear => self.clear_queue(),
            AppAction::QueueMoveUp => self.move_queue_item(false),
            AppAction::QueueMoveDown => self.move_queue_item(true),
            AppAction::PlayPause => return self.play_pause(),
            AppAction::Play => return self.play(),
            AppAction::Pause => return self.pause(),
            AppAction::Stop => return self.stop_playback(),
            AppAction::Next => return self.next_track(),
            AppAction::Previous => return self.previous_track(),
            AppAction::SeekBackward => return self.seek(false),
            AppAction::SeekForward => return self.seek(true),
            AppAction::SeekRelative { forward, distance } => {
                return self.seek_relative(forward, distance);
            }
            AppAction::SeekAbsolute {
                playback_generation,
                queue_instance,
                position,
            } => {
                return self.seek_absolute(playback_generation, queue_instance, position);
            }
            AppAction::VolumeDown => return self.change_volume(false),
            AppAction::VolumeUp => return self.change_volume(true),
            AppAction::SetVolume(volume_percent) => return self.set_volume(volume_percent),
            AppAction::ToggleMute => return self.toggle_mute(),
            AppAction::ToggleShuffle => self.toggle_shuffle(),
            AppAction::SetShuffle(enabled) => self.set_shuffle(enabled),
            AppAction::CycleRepeat => self.cycle_repeat(),
            AppAction::RepeatOff => self.set_repeat(RepeatMode::Off),
            AppAction::RepeatAll => self.set_repeat(RepeatMode::All),
            AppAction::RepeatOne => self.set_repeat(RepeatMode::One),
        }
        None
    }

    #[must_use]
    pub(crate) fn library_row_count(&self) -> usize {
        self.browser.row_count()
    }

    #[must_use]
    pub(crate) fn library_row(&self, position: usize) -> Option<BrowserRow> {
        self.browser.row(position)
    }

    #[must_use]
    pub(crate) fn entry(&self, entry_index: usize) -> Option<&TrackEntry> {
        self.index.entries.get(entry_index)
    }

    #[must_use]
    pub(crate) fn help_visible(&self) -> bool {
        self.overlay == Overlay::Help
    }

    #[must_use]
    pub(crate) fn entry_title(&self, entry_index: usize) -> String {
        self.entry_title_bounded(entry_index, self.status_text_max_bytes)
    }

    #[must_use]
    pub(crate) fn entry_title_bounded(&self, entry_index: usize, max_bytes: usize) -> String {
        let Some(entry) = self.entry(entry_index) else {
            return "Missing track".into();
        };
        self.index
            .asset_for_entry(entry)
            .and_then(|asset| asset.tags.title.as_deref())
            .map_or_else(
                || bounded_text(&entry.search.filename, max_bytes),
                |title| terminal_safe(title.as_bytes(), max_bytes),
            )
    }

    #[must_use]
    pub(crate) fn queue_item_title(&self, item: QueueItem, max_bytes: usize) -> String {
        if item.scan_generation != self.index.generation
            || self
                .index
                .entries
                .get(item.entry_index)
                .is_none_or(|entry| entry.id != item.entry_id)
        {
            "Stale track".into()
        } else {
            self.entry_title_bounded(item.entry_index, max_bytes)
        }
    }

    #[must_use]
    pub(crate) fn player_title(&self, max_bytes: usize) -> String {
        self.playback.current.map_or_else(
            || "Nothing playing".into(),
            |item| self.queue_item_title(item, max_bytes),
        )
    }

    #[must_use]
    pub(crate) fn player_creator(&self, max_bytes: usize) -> String {
        let Some(item) = self.playback.current else {
            return String::new();
        };
        self.media_for_item(item)
            .and_then(|(_, asset)| {
                asset
                    .tags
                    .artist
                    .as_deref()
                    .or(asset.tags.album_artist.as_deref())
            })
            .map_or_else(String::new, |creator| {
                terminal_safe(creator.as_bytes(), max_bytes)
            })
    }

    #[must_use]
    pub(crate) fn state_identity(&self, max_bytes: usize) -> (String, String) {
        let Some(item) = self.playback.current else {
            return ("Nothing playing".into(), String::new());
        };
        let Some((entry, asset)) = self.media_for_item(item) else {
            return ("Stale track".into(), String::new());
        };
        let title = asset.tags.title.as_deref().map_or_else(
            || {
                let filename = entry
                    .display_path
                    .file_stem()
                    .map_or(&[][..], OsStr::as_bytes);
                bounded_text(&String::from_utf8_lossy(filename), max_bytes)
            },
            |title| bounded_text(title, max_bytes),
        );
        let creator = asset
            .tags
            .artist
            .as_deref()
            .or(asset.tags.album_artist.as_deref())
            .map_or_else(String::new, |creator| bounded_text(creator, max_bytes));
        (title, creator)
    }

    #[must_use]
    pub(crate) fn playback_format(&self) -> Option<AudioFormat> {
        self.playback.format
    }

    #[must_use]
    pub(crate) const fn playback_position(&self) -> Duration {
        self.playback.position
    }

    #[must_use]
    pub(crate) const fn playback_duration(&self) -> Option<Duration> {
        self.playback.duration
    }

    #[must_use]
    pub(crate) const fn audio_spectrum_levels(&self) -> [u8; crate::audio::SPECTRUM_BANDS] {
        self.playback.spectrum.levels()
    }

    pub(crate) fn audio_spectrum(&mut self, spectrum: AudioSpectrum) {
        self.playback.spectrum = spectrum;
    }

    #[must_use]
    pub(crate) fn queue_position(&self) -> Option<usize> {
        self.current_queue_index().map(|index| index + 1)
    }

    #[must_use]
    pub(crate) fn current_track_token(&self) -> Option<(u64, u64)> {
        self.playback
            .current
            .map(|item| (self.playback.generation, item.instance_id))
    }

    #[must_use]
    pub(crate) fn can_go_next(&self) -> bool {
        self.next_queue_index(self.repeat == RepeatMode::All)
            .is_some()
    }

    #[must_use]
    pub(crate) const fn seek_revision(&self) -> u64 {
        self.playback.seek_revision
    }

    pub(crate) fn session_stopped(&mut self) {
        self.playback.status = PlaybackStatus::Stopped;
        self.playback.seek_target = None;
        self.playback.format = None;
    }

    #[must_use]
    pub(crate) fn session_identity(&self) -> Option<SessionIdentity> {
        if self.queue.items().is_empty() {
            return None;
        }
        let current_index = self
            .current_queue_index()
            .unwrap_or_else(|| self.queue.selection.min(self.queue.items().len() - 1));
        Some(SessionIdentity {
            queue_generation: self.queue.generation(),
            current_index,
            current_instance_id: self.queue.items()[current_index].instance_id,
        })
    }

    pub(crate) fn session_snapshot(&self) -> AppResult<Option<crate::state::SessionSnapshot>> {
        let Some(identity) = self.session_identity() else {
            return Ok(None);
        };
        let mut queue_entry_ids = Vec::new();
        reserve_exact(
            &mut queue_entry_ids,
            self.queue.items().len(),
            "resume queue",
        )?;
        queue_entry_ids.extend(self.queue.items().iter().map(|item| item.entry_id.0));
        let position = self
            .current_queue_index()
            .filter(|index| *index == identity.current_index)
            .map_or(Duration::ZERO, |_| self.playback.resume_position());
        Ok(Some(crate::state::SessionSnapshot::new(
            queue_entry_ids,
            identity.current_index,
            u64::try_from(position.as_millis()).unwrap_or(u64::MAX),
        )))
    }

    pub(crate) fn restore_session(
        &mut self,
        snapshot: &crate::state::SessionSnapshot,
    ) -> AppResult<()> {
        let Some(restored) = self.queue.restore(&self.index, snapshot)? else {
            self.set_warning("Saved session has no tracks in the current library");
            return Ok(());
        };
        let current_index = restored.current_index;
        let current = self.queue.items()[current_index];
        self.playback
            .restore(current, current_index, restored.position);
        let skipped = restored.skipped;
        if skipped == 0 {
            self.set_persistent_status("Session restored; Space resumes");
        } else {
            let track_label = if skipped == 1 { "track" } else { "tracks" };
            self.set_warning(&format!(
                "Session restored; {skipped} unavailable or duplicate {track_label} skipped; Space resumes"
            ));
        }
        Ok(())
    }

    pub(crate) fn session_ignored(&mut self, reason: &str) {
        let reason = terminal_safe(reason.as_bytes(), self.status_text_max_bytes);
        self.set_warning(&format!("Saved session ignored: {reason}"));
    }

    pub(crate) fn desktop_controls_unavailable(&mut self, reason: &str) {
        let reason = terminal_safe(reason.as_bytes(), self.status_text_max_bytes);
        self.set_warning(&format!("Desktop controls unavailable: {reason}"));
    }

    pub(crate) fn media_for_item(
        &self,
        item: QueueItem,
    ) -> Option<(&TrackEntry, &crate::model::MediaAsset)> {
        if item.scan_generation != self.index.generation {
            return None;
        }
        let entry = self.index.entries.get(item.entry_index)?;
        if entry.id != item.entry_id {
            return None;
        }
        Some((entry, self.index.asset_for_entry(entry)?))
    }

    // Keep query mutation in the action body; match guards only classify keys.
    #[allow(clippy::collapsible_match)]
    fn search_key(&mut self, key: KeyEvent) -> Option<PlaybackIntent> {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => {
                self.close_search();
                self.set_status("Search closed");
            }
            KeyCode::Enter => return self.activate_search_result(),
            KeyCode::Up => self.move_library_selection(false),
            KeyCode::Down => self.move_library_selection(true),
            KeyCode::Char('p') if control => self.move_library_selection(false),
            KeyCode::Char('n') if control => self.move_library_selection(true),
            KeyCode::Char('u') if control => {
                self.browser.clear_query(&self.index);
            }
            KeyCode::Backspace => {
                self.browser.pop_query(&self.index);
            }
            KeyCode::Char(character)
                if !character.is_control()
                    && (key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT) =>
            {
                if !self.browser.push_query(character, &self.index) {
                    self.set_warning("Search query limit reached");
                }
            }
            _ => {}
        }
        None
    }

    fn open_search(&mut self) {
        self.browser.open_search();
        self.focus = Focus::Library;
        self.set_status("Type to search; Esc closes");
    }

    fn close_search(&mut self) {
        self.browser.close_search();
    }

    fn activate_search_result(&mut self) -> Option<PlaybackIntent> {
        let selected = self.selected_entry_index();
        self.close_search();
        if let Some(entry_index) = selected {
            self.queue_entry(entry_index)
        } else {
            self.set_status("No search result selected");
            None
        }
    }

    fn move_selection(&mut self, next: bool) {
        match self.focus {
            Focus::Library => self.move_library_selection(next),
            Focus::Queue => self.queue.move_selection(next),
            Focus::Player => {}
        }
    }

    fn move_library_selection(&mut self, next: bool) {
        let row_count = self.library_row_count();
        move_index(&mut self.browser.selection, row_count, next);
    }

    fn move_to_edge(&mut self, last: bool) {
        match self.focus {
            Focus::Library => {
                self.browser.selection = if last {
                    self.library_row_count().saturating_sub(1)
                } else {
                    0
                };
            }
            Focus::Queue => {
                self.queue.selection = if last {
                    self.queue.items().len().saturating_sub(1)
                } else {
                    0
                };
            }
            Focus::Player => {}
        }
    }

    fn activate(&mut self) -> Option<PlaybackIntent> {
        if self.focus != Focus::Library {
            return self.play_pause();
        }
        match self.library_row(self.browser.selection) {
            Some(BrowserRow::Track { entry_index, .. }) => self.queue_entry(entry_index),
            Some(BrowserRow::Playlist { playlist_index }) => self
                .normal_browser_position(self.browser.selection)
                .and_then(|position| self.queue_playlist(playlist_index, position)),
            Some(BrowserRow::Folder { .. }) => {
                self.toggle_selected_folder();
                None
            }
            Some(BrowserRow::LibraryRoot | BrowserRow::PlaylistsRoot) | None => {
                self.set_status("Select a track or playlist to add it to the queue");
                None
            }
        }
    }

    fn selected_entry_index(&self) -> Option<usize> {
        self.browser.selected_entry_index()
    }

    fn normal_browser_position(&self, visible_position: usize) -> Option<usize> {
        self.browser.normal_position(visible_position)
    }

    fn toggle_selected_folder(&mut self) {
        if let Some(collapsed) = self.browser.toggle_selected_folder() {
            self.set_status(if collapsed {
                "Folder collapsed"
            } else {
                "Folder expanded"
            });
        }
    }

    fn toggle_shuffle(&mut self) {
        self.set_shuffle(!self.queue.shuffle);
    }

    fn set_shuffle(&mut self, enabled: bool) {
        if self.queue.shuffle == enabled {
            return;
        }
        self.queue.shuffle = enabled;
        self.refresh_shuffle_order();
        self.set_status(if self.queue.shuffle {
            "Shuffle: on"
        } else {
            "Shuffle: off"
        });
    }

    fn cycle_repeat(&mut self) {
        self.set_repeat(self.repeat.next());
    }

    fn set_repeat(&mut self, repeat: RepeatMode) {
        if self.repeat == repeat {
            return;
        }
        self.repeat = repeat;
        self.set_status(&format!("Repeat: {}", self.repeat.label()));
    }

    fn refresh_shuffle_order(&mut self) {
        let current = self
            .worker_has_track()
            .then_some(self.playback.current)
            .flatten();
        self.queue.refresh_shuffle_order(current);
    }

    fn queue_entry(&mut self, entry_index: usize) -> Option<PlaybackIntent> {
        let appended = match self.queue.append(&self.index, std::iter::once(entry_index)) {
            Ok(appended) => appended,
            Err(error) => {
                self.queue_error(error, "The selected track is no longer available");
                return None;
            }
        };
        let title = self.entry_title(entry_index);
        self.set_status(&format!("Queued: {title}"));
        self.start_appended_queue(appended.start)
    }

    fn queue_playlist(
        &mut self,
        playlist_index: usize,
        browser_position: usize,
    ) -> Option<PlaybackIntent> {
        let Some(playlist) = self.index.playlists.get(playlist_index) else {
            self.set_warning("The selected playlist is no longer available");
            return None;
        };
        let name = playlist.name.clone();
        let entries = self.browser.playlist_entries(browser_position);
        let appended = match self.queue.append(&self.index, entries) {
            Ok(appended) => appended,
            Err(error) => {
                self.queue_error(error, "The playlist contains a stale track");
                return None;
            }
        };
        let count = appended.len();
        if count == 0 {
            self.set_status("Playlist is empty");
            return None;
        }
        let track_label = if count == 1 { "track" } else { "tracks" };
        self.set_status(&format!("Queued playlist: {name} ({count} {track_label})"));
        self.start_appended_queue(appended.start)
    }

    fn start_appended_queue(&mut self, start: usize) -> Option<PlaybackIntent> {
        if self.worker_has_track() {
            return None;
        }
        self.queue.selection = start;
        self.start_new_shuffle_round(start)
    }

    fn queue_error(&mut self, error: QueueError, unavailable: &str) {
        match error {
            QueueError::Unavailable => self.set_warning(unavailable),
            QueueError::Duplicate => self.set_status("Audio already in Queue"),
            QueueError::Limit => self.set_warning("Queue limit reached"),
            QueueError::GenerationExhausted => self.set_error("Queue generation exhausted"),
        }
    }

    fn remove_queue_item(&mut self) {
        if self.focus != Focus::Queue {
            return;
        }
        let removed_before_current = self.current_queue_index().is_none()
            && self.queue.selection < self.playback.position_hint;
        match self.queue.remove_selected(&self.index) {
            Ok(Some(removed)) => {
                if removed_before_current {
                    self.playback.position_hint -= 1;
                }
                self.reconcile_playback_position();
                let title = self.entry_title(removed.entry_index);
                self.set_status(&format!("Removed: {title}"));
            }
            Ok(None) => {}
            Err(error) => self.queue_error(error, "The selected track is no longer available"),
        }
    }

    fn clear_queue(&mut self) {
        if self.focus != Focus::Queue {
            return;
        }
        match self.queue.clear() {
            Ok(true) => {
                self.reconcile_playback_position();
                self.set_status("Queue cleared");
            }
            Ok(false) => {}
            Err(error) => self.queue_error(error, "The queue is no longer available"),
        }
    }

    fn move_queue_item(&mut self, down: bool) {
        if self.focus != Focus::Queue {
            return;
        }
        match self.queue.move_selected(down) {
            Ok(true) => {
                self.reconcile_playback_position();
                self.set_status("Queue order changed");
            }
            Ok(false) => {}
            Err(error) => self.queue_error(error, "The queue is no longer available"),
        }
    }

    fn set_status(&mut self, message: &str) {
        self.set_status_kind(StatusKind::Info, message, Some(INFORMATION_NOTICE_LIFETIME));
    }

    fn set_persistent_status(&mut self, message: &str) {
        self.set_status_kind(StatusKind::Info, message, None);
    }

    fn set_warning(&mut self, message: &str) {
        self.set_status_kind(StatusKind::Warning, message, None);
    }

    fn set_error(&mut self, message: &str) {
        self.set_status_kind(StatusKind::Error, message, None);
    }

    fn set_status_kind(&mut self, kind: StatusKind, message: &str, lifetime: Option<Duration>) {
        self.status_kind = kind;
        self.status_expires_at = lifetime.map(|duration| self.event_time.saturating_add(duration));
        self.status_message = bounded_text(message, self.status_text_max_bytes);
    }

    pub(crate) fn advance_time(&mut self, now: Duration) {
        self.event_time = self.event_time.max(now);
        if self
            .status_expires_at
            .is_some_and(|deadline| self.event_time >= deadline)
        {
            self.status_message.clear();
            self.status_kind = StatusKind::Info;
            self.status_expires_at = None;
        }
    }
}

fn reserve_exact<T>(items: &mut Vec<T>, capacity: usize, name: &str) -> AppResult<()> {
    items
        .try_reserve_exact(capacity)
        .map_err(|error| AppError::Resource(format!("cannot reserve {name}: {error}")))
}

fn move_index(selection: &mut usize, len: usize, next: bool) {
    if len == 0 {
        *selection = 0;
    } else if next {
        *selection = selection.saturating_add(1).min(len - 1);
    } else {
        *selection = selection.saturating_sub(1);
    }
}

/// Acquires the mutable-session leases, scans once, starts file logging, and owns the TUI.
///
/// # Errors
///
/// Returns a typed startup, scanning, logging, terminal, or event-loop error.
pub fn run(root: &SelectedRoot, config: &Config) -> AppResult<()> {
    reserve_startup_open_files(config.runtime.max_open_files)?;
    let _active_lease = ActiveTuiLease::acquire()?;
    let _root_lease = RootWriterLease::acquire_from(root.descriptor(), &root.path)?;
    let logging = crate::logging::initialize_from(root.descriptor(), &root.path, &config.logging)?;
    tracing::info!("terminal session starting");
    let result = crate::scan::scan_for_session(root.descriptor(), &root.path, config)
        .and_then(|index| crate::terminal::run(root.descriptor(), &root.path, config, index));
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
mod tests;
