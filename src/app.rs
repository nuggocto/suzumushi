// SPDX-License-Identifier: Apache-2.0

//! App-owned library, search, queue, terminal state, and session lifecycle.

use std::ffi::OsStr;
use std::io;
use std::mem::size_of;
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::audio::{AudioEvent, AudioFormat, AudioPosition, PlaybackSettings};
use crate::config::{Config, UI_STATE_SCRATCH_BYTES};
use crate::display::{bounded_text, terminal_safe};
use crate::errors::{AppError, AppResult};
use crate::input::AppAction;
use crate::locks::{ActiveTuiLease, RootWriterLease};
use crate::model::{PlaylistId, ScanIndex, TrackEntry, TrackEntryId, TrackEntrySource};
use crate::paths::SelectedRoot;

const TUI_STARTUP_OPEN_FILES_PEAK: usize = 6;

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

/// One lightweight row in the normal library browser.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BrowserRow {
    LibraryRoot,
    PlaylistsRoot,
    Folder {
        entry_index: usize,
        component_index: usize,
        indent: usize,
    },
    Playlist {
        playlist_index: usize,
    },
    Track {
        entry_index: usize,
        indent: usize,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct QueueItem {
    pub instance_id: u64,
    pub entry_index: usize,
    pub entry_id: TrackEntryId,
    pub scan_generation: u64,
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

#[derive(Debug)]
struct PlaybackState {
    current: Option<QueueItem>,
    position_hint: usize,
    timeline_revision: u64,
    seek_target: Option<Duration>,
    format: Option<AudioFormat>,
    position: Duration,
    duration: Option<Duration>,
    start_paused: bool,
}

impl PlaybackState {
    const fn new() -> Self {
        Self {
            current: None,
            position_hint: 0,
            timeline_revision: 0,
            seek_target: None,
            format: None,
            position: Duration::ZERO,
            duration: None,
            start_paused: false,
        }
    }
}

#[derive(Debug)]
pub(crate) struct SearchState {
    pub active: bool,
    pub query: String,
    pub results: Vec<usize>,
    prefix_table: Vec<usize>,
    saved_selection: usize,
    max_results: usize,
    max_query_bytes: usize,
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
    pub index: ScanIndex,
    pub(crate) browser_rows: Vec<BrowserRow>,
    pub(crate) library_selection: usize,
    pub(crate) search: SearchState,
    pub(crate) queue: Vec<QueueItem>,
    pub(crate) queue_selection: usize,
    pub queue_generation: u64,
    pub playback_generation: u64,
    pub playback_status: PlaybackStatus,
    pub volume_percent: u8,
    pub muted: bool,
    pub shuffle: bool,
    pub repeat: RepeatMode,
    queue_max_items: usize,
    queue_max_bytes: usize,
    status_text_max_bytes: usize,
    next_queue_item_id: u64,
    shuffle_order: Vec<u64>,
    shuffle_cursor: Option<usize>,
    shuffle_seed: u64,
    seek_revision: u64,
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

        let browser_capacity = index
            .counters
            .encountered_entries
            .checked_add(index.playlists.len())
            .and_then(|value| value.checked_add(2))
            .ok_or_else(|| AppError::Resource("library browser reservation overflow".into()))?;
        let browser_bytes = browser_capacity
            .checked_mul(size_of::<BrowserRow>())
            .ok_or_else(|| {
                AppError::Resource("library browser byte reservation overflow".into())
            })?;
        let search_bytes = search_reservation_bytes(config)?;
        let shuffle_bytes = config
            .queue
            .max_items
            .checked_mul(size_of::<u64>())
            .ok_or_else(|| AppError::Resource("shuffle reservation overflow".into()))?;
        if browser_bytes
            .saturating_add(search_bytes)
            .saturating_add(shuffle_bytes)
            > UI_STATE_SCRATCH_BYTES
        {
            return Err(AppError::InvalidConfig(
                "library browser, search, and shuffle reservations exceed the UI/state scratch budget"
                    .into(),
            ));
        }

        let mut browser_rows = Vec::new();
        reserve_exact(&mut browser_rows, browser_capacity, "library browser")?;
        build_browser_rows(&index, &mut browser_rows, browser_capacity)?;
        let library_selection = browser_rows
            .iter()
            .position(|row| matches!(row, BrowserRow::Track { .. }))
            .unwrap_or(0);

        let search = allocate_search(config, library_selection)?;
        let (queue, shuffle_order) = allocate_queue(config)?;
        let shuffle_seed = random_seed();

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
            index,
            browser_rows,
            library_selection,
            search,
            queue,
            queue_selection: 0,
            queue_generation: 0,
            playback_generation: 0,
            playback_status: PlaybackStatus::Stopped,
            volume_percent: 100,
            muted: false,
            shuffle: false,
            repeat: RepeatMode::Off,
            queue_max_items: config.queue.max_items,
            queue_max_bytes: config.queue.max_bytes,
            status_text_max_bytes: config.runtime.status_text_max_bytes,
            next_queue_item_id: 1,
            shuffle_order,
            shuffle_cursor: None,
            shuffle_seed,
            seek_revision: 0,
            playback: PlaybackState::new(),
        })
    }

    /// Resolves a key against the active search mode or normal key map.
    pub(crate) fn key(&mut self, key: KeyEvent, _now: Duration) -> Option<PlaybackIntent> {
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
        if self.search.active {
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
        if self.search.active && !self.search.query.is_empty() {
            self.search.results.len()
        } else {
            self.browser_rows.len()
        }
    }

    #[must_use]
    pub(crate) fn library_row(&self, position: usize) -> Option<BrowserRow> {
        if self.search.active && !self.search.query.is_empty() {
            self.search
                .results
                .get(position)
                .copied()
                .map(|entry_index| BrowserRow::Track {
                    entry_index,
                    indent: 0,
                })
        } else {
            self.browser_rows.get(position).copied()
        }
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
    pub(crate) fn queue_position(&self) -> Option<usize> {
        self.current_queue_index().map(|index| index + 1)
    }

    #[must_use]
    pub(crate) fn current_track_token(&self) -> Option<(u64, u64)> {
        self.playback
            .current
            .map(|item| (self.playback_generation, item.instance_id))
    }

    #[must_use]
    pub(crate) fn can_go_next(&self) -> bool {
        self.next_queue_index(self.repeat == RepeatMode::All)
            .is_some()
    }

    #[must_use]
    pub(crate) const fn seek_revision(&self) -> u64 {
        self.seek_revision
    }

    pub(crate) fn audio_position(&mut self, update: AudioPosition) {
        if update.generation != self.playback_generation
            || self.playback.current.is_none()
            || matches!(
                self.playback_status,
                PlaybackStatus::Stopped | PlaybackStatus::Error
            )
            || self.playback.seek_target.is_some()
            || update.timeline_revision < self.playback.timeline_revision
        {
            return;
        }
        self.playback.timeline_revision = update.timeline_revision;
        self.playback.position = update.position;
        if update.duration.is_some() {
            self.playback.duration = update.duration;
        }
    }

    pub(crate) fn session_stopped(&mut self) {
        self.playback_status = PlaybackStatus::Stopped;
        self.playback.seek_target = None;
        self.playback.format = None;
    }

    #[must_use]
    pub(crate) fn session_identity(&self) -> Option<SessionIdentity> {
        if self.queue.is_empty() {
            return None;
        }
        let current_index = self
            .current_queue_index()
            .unwrap_or_else(|| self.queue_selection.min(self.queue.len() - 1));
        Some(SessionIdentity {
            queue_generation: self.queue_generation,
            current_index,
            current_instance_id: self.queue[current_index].instance_id,
        })
    }

    pub(crate) fn session_snapshot(&self) -> AppResult<Option<crate::state::SessionSnapshot>> {
        let Some(identity) = self.session_identity() else {
            return Ok(None);
        };
        let mut queue_entry_ids = Vec::new();
        reserve_exact(&mut queue_entry_ids, self.queue.len(), "resume queue")?;
        queue_entry_ids.extend(self.queue.iter().map(|item| item.entry_id.0));
        let position = self
            .current_queue_index()
            .filter(|index| *index == identity.current_index)
            .map_or(Duration::ZERO, |_| self.playback.position);
        let position = self
            .playback
            .duration
            .filter(|duration| position >= *duration)
            .map_or(position, |_| Duration::ZERO);
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
        let mut wanted = Vec::new();
        reserve_exact(
            &mut wanted,
            snapshot.queue_entry_ids.len(),
            "resume entry lookup",
        )?;
        wanted.extend(snapshot.queue_entry_ids.iter().copied().map(TrackEntryId));
        wanted.sort_unstable();
        wanted.dedup();

        let mut available = Vec::new();
        reserve_exact(&mut available, wanted.len(), "resume entry map")?;
        for (entry_index, entry) in self.index.entries.iter().enumerate() {
            if wanted.binary_search(&entry.id).is_ok() {
                available.push((entry.id, entry_index));
            }
        }
        available.sort_unstable_by_key(|(id, _)| *id);
        available.dedup_by_key(|(id, _)| *id);

        let mut restored_current = None;
        for (saved_index, entry_id) in snapshot.queue_entry_ids.iter().copied().enumerate() {
            let entry_id = TrackEntryId(entry_id);
            let Ok(position) = available.binary_search_by_key(&entry_id, |(id, _)| *id) else {
                continue;
            };
            let entry_index = available[position].1;
            let queue_index = self.queue.len();
            self.queue.push(QueueItem {
                instance_id: self.next_queue_item_id,
                entry_index,
                entry_id,
                scan_generation: self.index.generation,
            });
            self.next_queue_item_id = self
                .next_queue_item_id
                .checked_add(1)
                .ok_or_else(|| AppError::Resource("resume queue item identity exhausted".into()))?;
            if saved_index == snapshot.current_index {
                restored_current = Some(queue_index);
            }
        }

        if self.queue.is_empty() {
            self.set_warning("Saved session has no tracks in the current library");
            return Ok(());
        }

        self.queue_generation = 1;
        let current_index = restored_current.unwrap_or(0);
        let position = if restored_current.is_some() {
            Duration::from_millis(snapshot.position_ms)
        } else {
            Duration::ZERO
        };
        let current = self.queue[current_index];
        self.queue_selection = current_index;
        self.playback.current = Some(current);
        self.playback.position_hint = current_index;
        self.playback.position = position;
        self.playback.duration = None;
        self.playback.format = None;
        self.playback.timeline_revision = 0;
        self.playback.seek_target = None;
        self.playback_status = PlaybackStatus::Stopped;
        let missing = snapshot
            .queue_entry_ids
            .len()
            .saturating_sub(self.queue.len());
        if missing == 0 {
            self.set_status("Session restored; Space resumes");
        } else {
            let track_label = if missing == 1 { "track" } else { "tracks" };
            self.set_warning(&format!(
                "Session restored; {missing} missing {track_label} skipped; Space resumes"
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

    pub(crate) fn audio_event(&mut self, event: AudioEvent) -> Option<PlaybackIntent> {
        let generation = match &event {
            AudioEvent::Started { generation, .. }
            | AudioEvent::Paused { generation }
            | AudioEvent::Resumed { generation }
            | AudioEvent::Stopped { generation }
            | AudioEvent::Seeked { generation, .. }
            | AudioEvent::Finished { generation }
            | AudioEvent::Failed { generation, .. } => *generation,
        };
        if generation != self.playback_generation || self.playback.current.is_none() {
            return None;
        }
        match event {
            AudioEvent::Started {
                timeline_revision,
                format,
                duration,
                position,
                ..
            } => {
                let status = if self.playback.start_paused {
                    PlaybackStatus::Paused
                } else {
                    PlaybackStatus::Playing
                };
                self.set_playback_status(status);
                self.playback.seek_target = None;
                self.playback.format = Some(format);
                self.playback.duration = duration;
                self.apply_event_position(timeline_revision, position);
                self.set_status(if status == PlaybackStatus::Paused {
                    "Paused"
                } else {
                    "Playing"
                });
                None
            }
            AudioEvent::Paused { .. } => {
                self.set_playback_status(PlaybackStatus::Paused);
                self.set_status("Paused");
                None
            }
            AudioEvent::Resumed { .. } => {
                self.set_playback_status(PlaybackStatus::Playing);
                self.set_status("Playing");
                None
            }
            AudioEvent::Stopped { .. } => {
                self.set_playback_status(PlaybackStatus::Stopped);
                self.playback.seek_target = None;
                self.playback.format = None;
                self.playback.position = Duration::ZERO;
                self.set_status("Stopped");
                None
            }
            AudioEvent::Seeked {
                timeline_revision,
                position,
                ..
            } => {
                if self
                    .playback
                    .seek_target
                    .is_some_and(|target| target != position)
                {
                    self.playback.timeline_revision =
                        self.playback.timeline_revision.max(timeline_revision);
                } else {
                    self.playback.seek_target = None;
                    self.apply_event_position(timeline_revision, position);
                    self.seek_revision = self.seek_revision.saturating_add(1);
                    self.set_status("Seeked");
                }
                None
            }
            AudioEvent::Finished { .. } => {
                self.playback.seek_target = None;
                self.advance_after_finish()
            }
            AudioEvent::Failed { message, .. } => {
                self.set_playback_status(PlaybackStatus::Error);
                self.playback.seek_target = None;
                self.playback.format = None;
                let message = terminal_safe(message.as_bytes(), self.status_text_max_bytes);
                self.set_error(&format!("Playback failed: {message}"));
                None
            }
        }
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
                self.search.query.clear();
                self.rebuild_search();
            }
            KeyCode::Backspace => {
                self.search.query.pop();
                self.rebuild_search();
            }
            KeyCode::Char(character)
                if !character.is_control()
                    && (key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT) =>
            {
                let next_bytes = self.search.query.len().saturating_add(character.len_utf8());
                if next_bytes > self.search.max_query_bytes {
                    self.set_warning("Search query limit reached");
                } else {
                    self.search.query.push(character);
                    self.rebuild_search();
                }
            }
            _ => {}
        }
        None
    }

    fn open_search(&mut self) {
        self.search.saved_selection = self.library_selection;
        self.search.active = true;
        self.search.query.clear();
        self.search.results.clear();
        self.library_selection = 0;
        self.focus = Focus::Library;
        self.set_status("Type to search; Esc closes");
    }

    fn close_search(&mut self) {
        self.search.active = false;
        self.search.query.clear();
        self.search.results.clear();
        self.library_selection = self
            .search
            .saved_selection
            .min(self.browser_rows.len().saturating_sub(1));
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

    fn rebuild_search(&mut self) {
        self.library_selection = 0;
        let SearchState {
            query,
            results,
            prefix_table,
            max_results,
            ..
        } = &mut self.search;
        results.clear();
        if query.is_empty() {
            return;
        }
        let needle = query.as_bytes();
        rebuild_ascii_case_prefix(needle, prefix_table);
        for (entry_index, entry) in self.index.entries.iter().enumerate() {
            let tags = self.index.asset_for_entry(entry).map(|asset| &asset.tags);
            if entry_matches(entry, tags, needle, prefix_table) {
                results.push(entry_index);
                if results.len() == *max_results {
                    break;
                }
            }
        }
    }

    fn move_selection(&mut self, next: bool) {
        match self.focus {
            Focus::Library => self.move_library_selection(next),
            Focus::Queue => move_index(&mut self.queue_selection, self.queue.len(), next),
            Focus::Player => {}
        }
    }

    fn move_library_selection(&mut self, next: bool) {
        let row_count = self.library_row_count();
        move_index(&mut self.library_selection, row_count, next);
    }

    fn move_to_edge(&mut self, last: bool) {
        match self.focus {
            Focus::Library => {
                self.library_selection = if last {
                    self.library_row_count().saturating_sub(1)
                } else {
                    0
                };
            }
            Focus::Queue => {
                self.queue_selection = if last {
                    self.queue.len().saturating_sub(1)
                } else {
                    0
                };
            }
            Focus::Player => {}
        }
    }

    fn activate(&mut self) -> Option<PlaybackIntent> {
        if self.focus != Focus::Library {
            return None;
        }
        match self.library_row(self.library_selection) {
            Some(BrowserRow::Track { entry_index, .. }) => self.queue_entry(entry_index),
            Some(BrowserRow::Playlist { playlist_index }) => {
                self.queue_playlist(playlist_index, self.library_selection)
            }
            Some(BrowserRow::Folder { .. }) => {
                self.set_status("Folder tracks are shown below");
                None
            }
            Some(BrowserRow::LibraryRoot | BrowserRow::PlaylistsRoot) | None => {
                self.set_status("Select a track or playlist to add it to the queue");
                None
            }
        }
    }

    fn selected_entry_index(&self) -> Option<usize> {
        match self.library_row(self.library_selection)? {
            BrowserRow::Track { entry_index, .. } => Some(entry_index),
            BrowserRow::LibraryRoot
            | BrowserRow::PlaylistsRoot
            | BrowserRow::Folder { .. }
            | BrowserRow::Playlist { .. } => None,
        }
    }

    fn play_pause(&mut self) -> Option<PlaybackIntent> {
        match self.playback_status {
            PlaybackStatus::Playing => Some(PlaybackIntent::Pause {
                generation: self.playback_generation,
            }),
            PlaybackStatus::Paused => Some(PlaybackIntent::Resume {
                generation: self.playback_generation,
            }),
            PlaybackStatus::Loading => {
                self.set_status("Track is still loading");
                None
            }
            PlaybackStatus::Stopped | PlaybackStatus::Error => {
                if self.queue.is_empty() {
                    self.set_status("Queue a track before starting playback");
                    None
                } else {
                    let index = self.queue_selection.min(self.queue.len() - 1);
                    let position = self
                        .current_queue_index()
                        .filter(|current| *current == index)
                        .map_or(Duration::ZERO, |_| self.playback.position);
                    self.start_new_shuffle_round_at(index, position)
                }
            }
        }
    }

    fn play(&mut self) -> Option<PlaybackIntent> {
        match self.playback_status {
            PlaybackStatus::Paused => Some(PlaybackIntent::Resume {
                generation: self.playback_generation,
            }),
            PlaybackStatus::Stopped | PlaybackStatus::Error => self.play_pause(),
            PlaybackStatus::Loading | PlaybackStatus::Playing => None,
        }
    }

    fn pause(&self) -> Option<PlaybackIntent> {
        (self.playback_status == PlaybackStatus::Playing).then_some(PlaybackIntent::Pause {
            generation: self.playback_generation,
        })
    }

    fn stop_playback(&mut self) -> Option<PlaybackIntent> {
        if self.playback.current.is_none()
            || matches!(self.playback_status, PlaybackStatus::Stopped)
        {
            self.set_status("Nothing is playing");
            return None;
        }
        if self.playback_status == PlaybackStatus::Error {
            self.set_playback_status(PlaybackStatus::Stopped);
            self.playback.seek_target = None;
            self.playback.format = None;
            self.set_status("Stopped");
            return None;
        }
        Some(PlaybackIntent::Stop {
            generation: self.playback_generation,
        })
    }

    fn next_track(&mut self) -> Option<PlaybackIntent> {
        if self.queue.is_empty() {
            return self.stop_playback();
        }
        if let Some(next) = self.next_queue_index(self.repeat == RepeatMode::All) {
            self.queue_selection = next;
            match self.playback_status {
                PlaybackStatus::Paused => self.start_queue_index_paused(next),
                PlaybackStatus::Stopped | PlaybackStatus::Error => {
                    self.select_stopped_queue_index(next);
                    None
                }
                PlaybackStatus::Loading | PlaybackStatus::Playing => self.start_queue_index(next),
            }
        } else if matches!(
            self.playback_status,
            PlaybackStatus::Loading | PlaybackStatus::Playing | PlaybackStatus::Paused
        ) {
            self.set_status("End of queue");
            Some(PlaybackIntent::Stop {
                generation: self.playback_generation,
            })
        } else {
            self.set_playback_status(PlaybackStatus::Stopped);
            self.playback.format = None;
            self.set_status("End of queue");
            None
        }
    }

    fn previous_track(&mut self) -> Option<PlaybackIntent> {
        if self.queue.is_empty() {
            return self.stop_playback();
        }
        let current = self.current_queue_index().unwrap_or(self.queue_selection);
        let previous = self
            .previous_queue_index(self.repeat == RepeatMode::All)
            .unwrap_or(current)
            .min(self.queue.len() - 1);
        self.queue_selection = previous;
        match self.playback_status {
            PlaybackStatus::Paused => self.start_queue_index_paused(previous),
            PlaybackStatus::Stopped | PlaybackStatus::Error => {
                self.select_stopped_queue_index(previous);
                None
            }
            PlaybackStatus::Loading | PlaybackStatus::Playing => self.start_queue_index(previous),
        }
    }

    fn advance_after_finish(&mut self) -> Option<PlaybackIntent> {
        if self.repeat == RepeatMode::One
            && let Some(current) = self.current_queue_index()
        {
            return self.start_queue_index(current);
        }
        if let Some(next) = self.next_queue_index(self.repeat == RepeatMode::All) {
            self.queue_selection = next;
            self.start_queue_index(next)
        } else {
            self.set_playback_status(PlaybackStatus::Stopped);
            self.playback.format = None;
            if let Some(duration) = self.playback.duration {
                self.playback.position = duration;
            }
            self.set_status("Queue finished");
            None
        }
    }

    fn start_queue_index(&mut self, index: usize) -> Option<PlaybackIntent> {
        self.start_queue_index_at(index, Duration::ZERO)
    }

    fn start_queue_index_paused(&mut self, index: usize) -> Option<PlaybackIntent> {
        self.start_queue_index_with_state(index, Duration::ZERO, true)
    }

    fn start_queue_index_at(&mut self, index: usize, position: Duration) -> Option<PlaybackIntent> {
        self.start_queue_index_with_state(index, position, false)
    }

    fn start_queue_index_with_state(
        &mut self,
        index: usize,
        position: Duration,
        paused: bool,
    ) -> Option<PlaybackIntent> {
        let item = self.queue.get(index).copied()?;
        let Some(generation) = self.playback_generation.checked_add(1) else {
            self.set_error("Playback generation exhausted");
            return None;
        };
        self.playback_generation = generation;
        self.playback.current = Some(item);
        self.playback.position_hint = index;
        if self.shuffle {
            self.shuffle_cursor = self
                .shuffle_order
                .iter()
                .position(|candidate| *candidate == item.instance_id);
        }
        self.playback.timeline_revision = 0;
        self.playback.seek_target = None;
        self.playback.format = None;
        self.playback.position = position;
        self.playback.duration = None;
        self.playback.start_paused = paused;
        self.set_playback_status(PlaybackStatus::Loading);
        let title = self.queue_item_title(item, self.status_text_max_bytes);
        self.set_status(&format!("Loading: {title}"));
        Some(PlaybackIntent::Load {
            generation,
            item,
            position,
            settings: self.playback_settings(),
            paused,
        })
    }

    fn select_stopped_queue_index(&mut self, index: usize) {
        let Some(item) = self.queue.get(index).copied() else {
            return;
        };
        if self.playback.current != Some(item) {
            let Some(generation) = self.playback_generation.checked_add(1) else {
                self.set_error("Playback generation exhausted");
                return;
            };
            self.playback_generation = generation;
            self.playback.current = Some(item);
        }
        self.playback.position_hint = index;
        if self.shuffle {
            self.shuffle_cursor = self
                .shuffle_order
                .iter()
                .position(|candidate| *candidate == item.instance_id);
        }
        self.playback.timeline_revision = 0;
        self.playback.seek_target = None;
        self.playback.format = None;
        self.playback.position = Duration::ZERO;
        self.playback.duration = None;
        self.playback.start_paused = false;
        self.set_playback_status(PlaybackStatus::Stopped);
        let title = self.queue_item_title(item, self.status_text_max_bytes);
        self.set_status(&format!("Selected: {title}"));
    }

    fn start_new_shuffle_round(&mut self, index: usize) -> Option<PlaybackIntent> {
        self.start_new_shuffle_round_at(index, Duration::ZERO)
    }

    fn start_new_shuffle_round_at(
        &mut self,
        index: usize,
        position: Duration,
    ) -> Option<PlaybackIntent> {
        let intent = self.start_queue_index_at(index, position);
        if intent.is_some() {
            self.refresh_shuffle_order();
        }
        intent
    }

    fn current_queue_index(&self) -> Option<usize> {
        let current = self.playback.current?;
        if self
            .queue
            .get(self.playback.position_hint)
            .is_some_and(|item| item.instance_id == current.instance_id)
        {
            return Some(self.playback.position_hint);
        }
        self.queue
            .iter()
            .position(|item| item.instance_id == current.instance_id)
    }

    fn apply_event_position(&mut self, timeline_revision: u64, position: Duration) {
        if timeline_revision > self.playback.timeline_revision {
            self.playback.timeline_revision = timeline_revision;
            self.playback.position = position;
        }
    }

    fn next_queue_index(&self, wrap: bool) -> Option<usize> {
        if self.queue.is_empty() {
            return None;
        }
        if self.shuffle {
            let current = self.playback.current.map(|item| item.instance_id);
            let position = current.and_then(|id| {
                self.shuffle_order
                    .iter()
                    .position(|candidate| *candidate == id)
            });
            let position = position.or(self.shuffle_cursor);
            let next = position.map_or(0, |position| position.saturating_add(1));
            let order_index = if next < self.shuffle_order.len() {
                next
            } else if wrap {
                0
            } else {
                return None;
            };
            let instance_id = *self.shuffle_order.get(order_index)?;
            return self
                .queue
                .iter()
                .position(|item| item.instance_id == instance_id);
        }
        let next = self.current_queue_index().map_or_else(
            || self.playback.position_hint.min(self.queue.len()),
            |index| index.saturating_add(1),
        );
        if next < self.queue.len() {
            Some(next)
        } else if wrap {
            Some(0)
        } else {
            None
        }
    }

    fn previous_queue_index(&self, wrap: bool) -> Option<usize> {
        if self.queue.is_empty() {
            return None;
        }
        if self.shuffle {
            let current = self.playback.current.map(|item| item.instance_id)?;
            let position = self
                .shuffle_order
                .iter()
                .position(|candidate| *candidate == current);
            let order_index = if let Some(position) = position {
                if let Some(previous) = position.checked_sub(1) {
                    previous
                } else if wrap {
                    self.shuffle_order.len().checked_sub(1)?
                } else {
                    return None;
                }
            } else if let Some(previous) = self.shuffle_cursor {
                previous
            } else if wrap {
                self.shuffle_order.len().checked_sub(1)?
            } else {
                return None;
            };
            let instance_id = self.shuffle_order[order_index];
            return self
                .queue
                .iter()
                .position(|item| item.instance_id == instance_id);
        }
        let current = self.current_queue_index().unwrap_or(self.queue_selection);
        current
            .checked_sub(1)
            .or_else(|| wrap.then(|| self.queue.len() - 1))
    }

    const fn playback_settings(&self) -> PlaybackSettings {
        PlaybackSettings {
            volume_percent: self.volume_percent,
            muted: self.muted,
        }
    }

    fn change_volume(&mut self, increase: bool) -> Option<PlaybackIntent> {
        self.volume_percent = if increase {
            self.volume_percent.saturating_add(5).min(100)
        } else {
            self.volume_percent.saturating_sub(5)
        };
        self.set_status(&format!("Volume: {}%", self.volume_percent));
        self.gain_intent()
    }

    fn set_volume(&mut self, volume_percent: u8) -> Option<PlaybackIntent> {
        let volume_percent = volume_percent.min(100);
        if self.volume_percent == volume_percent && !self.muted {
            return None;
        }
        self.volume_percent = volume_percent;
        self.muted = false;
        self.set_status(&format!("Volume: {}%", self.volume_percent));
        self.gain_intent()
    }

    fn toggle_mute(&mut self) -> Option<PlaybackIntent> {
        self.muted = !self.muted;
        self.set_status(if self.muted { "Muted" } else { "Unmuted" });
        self.gain_intent()
    }

    fn gain_intent(&self) -> Option<PlaybackIntent> {
        self.playback.current.filter(|_| self.worker_has_track())?;
        Some(PlaybackIntent::SetGain {
            generation: self.playback_generation,
            volume_percent: self.volume_percent,
            muted: self.muted,
        })
    }

    fn seek(&mut self, forward: bool) -> Option<PlaybackIntent> {
        self.seek_relative(forward, Duration::from_secs(5))
    }

    fn seek_relative(&mut self, forward: bool, distance: Duration) -> Option<PlaybackIntent> {
        if !matches!(
            self.playback_status,
            PlaybackStatus::Playing | PlaybackStatus::Paused
        ) {
            self.set_status("Nothing seekable is playing");
            return None;
        }
        let current = self.playback.seek_target.unwrap_or(self.playback.position);
        let position = if forward {
            current.saturating_add(distance)
        } else {
            current.saturating_sub(distance)
        };
        if forward
            && self
                .playback
                .duration
                .is_some_and(|duration| position > duration)
        {
            return self.next_track();
        }
        let seconds = distance.as_secs();
        let position = self
            .playback
            .duration
            .map_or(position, |duration| position.min(duration));
        self.playback.seek_target = Some(position);
        self.playback.position = position;
        let status = if forward {
            format!("Seek forward {seconds} seconds")
        } else {
            format!("Seek backward {seconds} seconds")
        };
        self.set_status(&status);
        Some(PlaybackIntent::Seek {
            generation: self.playback_generation,
            position,
        })
    }

    fn seek_absolute(
        &mut self,
        playback_generation: u64,
        queue_instance: u64,
        position: Duration,
    ) -> Option<PlaybackIntent> {
        if self.current_track_token() != Some((playback_generation, queue_instance)) {
            return None;
        }
        if !matches!(
            self.playback_status,
            PlaybackStatus::Playing | PlaybackStatus::Paused
        ) {
            return None;
        }
        if self
            .playback
            .duration
            .is_some_and(|duration| position > duration)
        {
            return None;
        }
        self.playback.seek_target = Some(position);
        self.playback.position = position;
        self.set_status("Seeked");
        Some(PlaybackIntent::Seek {
            generation: self.playback_generation,
            position,
        })
    }

    fn worker_has_track(&self) -> bool {
        self.playback.current.is_some()
            && matches!(
                self.playback_status,
                PlaybackStatus::Loading | PlaybackStatus::Playing | PlaybackStatus::Paused
            )
    }

    fn toggle_shuffle(&mut self) {
        self.set_shuffle(!self.shuffle);
    }

    fn set_shuffle(&mut self, enabled: bool) {
        if self.shuffle == enabled {
            return;
        }
        self.shuffle = enabled;
        self.refresh_shuffle_order();
        self.set_status(if self.shuffle {
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
        self.shuffle_order.clear();
        self.shuffle_cursor = None;
        if !self.shuffle {
            return;
        }
        self.shuffle_order
            .extend(self.queue.iter().map(|item| item.instance_id));
        for right in (1..self.shuffle_order.len()).rev() {
            let left = usize::try_from(self.next_random() % (right as u64 + 1))
                .expect("shuffle index is bounded by usize");
            self.shuffle_order.swap(left, right);
        }
        if let Some(current) = current
            && let Some(position) = self
                .shuffle_order
                .iter()
                .position(|candidate| *candidate == current.instance_id)
        {
            self.shuffle_order.swap(0, position);
            self.shuffle_cursor = Some(0);
        }
    }

    fn extend_shuffle_order(&mut self, queue_start: usize) {
        if !self.shuffle {
            return;
        }
        let suffix_start = self
            .shuffle_cursor
            .map_or(0, |cursor| cursor.saturating_add(1))
            .min(self.shuffle_order.len());
        self.shuffle_order.extend(
            self.queue[queue_start.min(self.queue.len())..]
                .iter()
                .map(|item| item.instance_id),
        );
        for right in (suffix_start.saturating_add(1)..self.shuffle_order.len()).rev() {
            let width = right - suffix_start + 1;
            let offset = usize::try_from(self.next_random() % width as u64)
                .expect("shuffle suffix index is bounded by usize");
            self.shuffle_order.swap(suffix_start + offset, right);
        }
    }

    fn remove_from_shuffle_order(&mut self, instance_id: u64) {
        if !self.shuffle {
            return;
        }
        let Some(position) = self
            .shuffle_order
            .iter()
            .position(|candidate| *candidate == instance_id)
        else {
            return;
        };
        self.shuffle_order.remove(position);
        if let Some(cursor) = self.shuffle_cursor
            && position <= cursor
        {
            self.shuffle_cursor = cursor.checked_sub(1);
        }
    }

    fn next_random(&mut self) -> u64 {
        let mut value = self.shuffle_seed;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.shuffle_seed = value;
        value
    }

    fn reconcile_playback_position(&mut self) {
        if let Some(index) = self.current_queue_index() {
            self.playback.position_hint = index;
        } else {
            self.playback.position_hint = self.playback.position_hint.min(self.queue.len());
        }
    }

    fn set_playback_status(&mut self, status: PlaybackStatus) {
        self.playback_status = status;
    }

    fn queue_entry(&mut self, entry_index: usize) -> Option<PlaybackIntent> {
        let Some(entry_id) = self.index.entries.get(entry_index).map(|entry| entry.id) else {
            self.set_warning("The selected track is no longer available");
            return None;
        };
        if !self.can_append_queue(1) {
            return None;
        }
        let start_now = !self.worker_has_track();
        let queue_index = self.queue.len();
        let title = self.entry_title(entry_index);
        self.queue.push(QueueItem {
            instance_id: self.next_queue_item_id,
            entry_index,
            entry_id,
            scan_generation: self.index.generation,
        });
        self.next_queue_item_id += 1;
        self.queue_generation += 1;
        self.set_status(&format!("Queued: {title}"));
        if start_now {
            self.queue_selection = queue_index;
            self.start_new_shuffle_round(queue_index)
        } else {
            self.extend_shuffle_order(queue_index);
            None
        }
    }

    fn queue_playlist(
        &mut self,
        playlist_index: usize,
        browser_position: usize,
    ) -> Option<PlaybackIntent> {
        let Some(playlist_name) = self
            .index
            .playlists
            .get(playlist_index)
            .map(|playlist| playlist.name.clone())
        else {
            self.set_warning("The selected playlist is no longer available");
            return None;
        };
        let start = browser_position
            .saturating_add(1)
            .min(self.browser_rows.len());
        let end = self.browser_rows[start..]
            .iter()
            .position(|row| matches!(row, BrowserRow::Playlist { .. }))
            .map_or(self.browser_rows.len(), |offset| start + offset);
        let track_count = self.browser_rows[start..end]
            .iter()
            .filter(|row| matches!(row, BrowserRow::Track { .. }))
            .count();
        if track_count == 0 {
            self.set_status("Playlist is empty");
            return None;
        }
        if self.browser_rows[start..end].iter().any(|row| {
            matches!(
                row,
                BrowserRow::Track { entry_index, .. }
                    if self.index.entries.get(*entry_index).is_none()
            )
        }) {
            self.set_warning("The playlist contains a stale track");
            return None;
        }
        if !self.can_append_queue(track_count) {
            return None;
        }
        let start_now = !self.worker_has_track();
        let first_index = self.queue.len();
        for position in start..end {
            if let BrowserRow::Track { entry_index, .. } = self.browser_rows[position] {
                let entry = &self.index.entries[entry_index];
                self.queue.push(QueueItem {
                    instance_id: self.next_queue_item_id,
                    entry_index,
                    entry_id: entry.id,
                    scan_generation: self.index.generation,
                });
                self.next_queue_item_id += 1;
            }
        }
        self.queue_generation += 1;
        let track_label = if track_count == 1 { "track" } else { "tracks" };
        self.set_status(&format!(
            "Queued playlist: {playlist_name} ({track_count} {track_label})"
        ));
        if start_now {
            self.queue_selection = first_index;
            self.start_new_shuffle_round(first_index)
        } else {
            self.extend_shuffle_order(first_index);
            None
        }
    }

    fn remove_queue_item(&mut self) {
        if self.focus != Focus::Queue || self.queue.is_empty() {
            return;
        }
        if !self.can_advance_queue_generation() {
            return;
        }
        let removed = self.queue[self.queue_selection];
        let title = self.entry_title(removed.entry_index);
        self.queue.remove(self.queue_selection);
        self.remove_from_shuffle_order(removed.instance_id);
        self.queue_selection = self.queue_selection.min(self.queue.len().saturating_sub(1));
        self.reconcile_playback_position();
        self.queue_generation += 1;
        self.set_status(&format!("Removed: {title}"));
    }

    fn clear_queue(&mut self) {
        if self.focus != Focus::Queue || self.queue.is_empty() {
            return;
        }
        if !self.can_advance_queue_generation() {
            return;
        }
        self.queue.clear();
        self.shuffle_order.clear();
        self.shuffle_cursor = None;
        self.queue_selection = 0;
        self.reconcile_playback_position();
        self.queue_generation += 1;
        self.set_status("Queue cleared");
    }

    fn move_queue_item(&mut self, down: bool) {
        if self.focus != Focus::Queue || self.queue.len() < 2 {
            return;
        }
        let destination = if down {
            self.queue_selection.checked_add(1)
        } else {
            self.queue_selection.checked_sub(1)
        };
        let Some(destination) = destination.filter(|index| *index < self.queue.len()) else {
            return;
        };
        if !self.can_advance_queue_generation() {
            return;
        }
        self.queue.swap(self.queue_selection, destination);
        self.queue_selection = destination;
        self.reconcile_playback_position();
        self.queue_generation += 1;
        self.set_status("Queue order changed");
    }

    fn can_advance_queue_generation(&mut self) -> bool {
        if self.queue_generation == u64::MAX {
            self.set_error("Queue generation exhausted");
            false
        } else {
            true
        }
    }

    fn can_append_queue(&mut self, additional_items: usize) -> bool {
        let next_items = self.queue.len().checked_add(additional_items);
        let next_bytes = next_items.and_then(|items| items.checked_mul(size_of::<QueueItem>()));
        let queue_ids_fit = u64::try_from(additional_items)
            .ok()
            .and_then(|additional| self.next_queue_item_id.checked_add(additional))
            .is_some();
        if next_items.is_none_or(|items| items > self.queue_max_items)
            || next_bytes.is_none_or(|bytes| bytes > self.queue_max_bytes)
            || !queue_ids_fit
        {
            self.set_warning("Queue limit reached");
            return false;
        }
        self.can_advance_queue_generation()
    }

    fn set_status(&mut self, message: &str) {
        self.set_status_kind(StatusKind::Info, message);
    }

    fn set_warning(&mut self, message: &str) {
        self.set_status_kind(StatusKind::Warning, message);
    }

    fn set_error(&mut self, message: &str) {
        self.set_status_kind(StatusKind::Error, message);
    }

    fn set_status_kind(&mut self, kind: StatusKind, message: &str) {
        self.status_kind = kind;
        self.status_message.clear();
        for character in message.chars() {
            if self
                .status_message
                .len()
                .saturating_add(character.len_utf8())
                > self.status_text_max_bytes
            {
                break;
            }
            self.status_message.push(character);
        }
    }
}

fn reserve_exact<T>(items: &mut Vec<T>, capacity: usize, name: &str) -> AppResult<()> {
    items
        .try_reserve_exact(capacity)
        .map_err(|error| AppError::Resource(format!("cannot reserve {name}: {error}")))
}

fn allocate_search(config: &Config, saved_selection: usize) -> AppResult<SearchState> {
    let mut query = String::new();
    query
        .try_reserve_exact(config.search.max_query_bytes)
        .map_err(|error| AppError::Resource(format!("cannot reserve search query: {error}")))?;
    let mut results = Vec::new();
    reserve_exact(&mut results, config.search.max_results, "search results")?;
    let mut prefix_table = Vec::new();
    reserve_exact(
        &mut prefix_table,
        config.search.max_query_bytes,
        "search prefix table",
    )?;
    Ok(SearchState {
        active: false,
        query,
        results,
        prefix_table,
        saved_selection,
        max_results: config.search.max_results,
        max_query_bytes: config.search.max_query_bytes,
    })
}

fn allocate_queue(config: &Config) -> AppResult<(Vec<QueueItem>, Vec<u64>)> {
    let mut queue = Vec::new();
    reserve_exact(&mut queue, config.queue.max_items, "queue")?;
    let queue_bytes = queue
        .capacity()
        .checked_mul(size_of::<QueueItem>())
        .ok_or_else(|| AppError::Resource("queue byte reservation overflow".into()))?;
    if queue_bytes > config.queue.max_bytes {
        return Err(AppError::InvalidConfig(
            "reserved queue capacity exceeds queue.max_bytes".into(),
        ));
    }
    let mut shuffle_order = Vec::new();
    reserve_exact(&mut shuffle_order, config.queue.max_items, "shuffle order")?;
    Ok((queue, shuffle_order))
}

fn random_seed() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .to_le_bytes();
    let mut low = [0_u8; size_of::<u64>()];
    low.copy_from_slice(&nanos[..size_of::<u64>()]);
    let seed = u64::from_le_bytes(low) ^ u64::from(std::process::id());
    if seed == 0 {
        0x9e37_79b9_7f4a_7c15
    } else {
        seed
    }
}

fn search_reservation_bytes(config: &Config) -> AppResult<usize> {
    config
        .search
        .max_results
        .checked_mul(size_of::<usize>())
        .and_then(|value| value.checked_add(config.search.max_query_bytes))
        .and_then(|value| {
            config
                .search
                .max_query_bytes
                .checked_mul(size_of::<usize>())
                .and_then(|prefix_bytes| value.checked_add(prefix_bytes))
        })
        .ok_or_else(|| AppError::Resource("search reservation overflow".into()))
}

fn entry_matches(
    entry: &TrackEntry,
    tags: Option<&crate::model::TrackTags>,
    needle: &[u8],
    prefix_table: &[usize],
) -> bool {
    contains_ascii_case_insensitive(entry.search.filename.as_bytes(), needle, prefix_table).found
        || contains_ascii_case_insensitive(
            entry.search.relative_path.as_bytes(),
            needle,
            prefix_table,
        )
        .found
        || tags.is_some_and(|tags| {
            [&tags.artist, &tags.album_artist, &tags.album, &tags.title]
                .into_iter()
                .flatten()
                .any(|field| {
                    contains_ascii_case_insensitive(field.as_bytes(), needle, prefix_table).found
                })
        })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SearchMatch {
    found: bool,
    #[cfg(test)]
    comparisons: usize,
}

fn rebuild_ascii_case_prefix(needle: &[u8], prefix_table: &mut Vec<usize>) {
    prefix_table.clear();
    prefix_table.resize(needle.len(), 0);
    let mut matched = 0;
    for index in 1..needle.len() {
        while matched > 0 && !needle[index].eq_ignore_ascii_case(&needle[matched]) {
            matched = prefix_table[matched - 1];
        }
        if needle[index].eq_ignore_ascii_case(&needle[matched]) {
            matched += 1;
        }
        prefix_table[index] = matched;
    }
}

fn contains_ascii_case_insensitive(
    haystack: &[u8],
    needle: &[u8],
    prefix_table: &[usize],
) -> SearchMatch {
    debug_assert_eq!(needle.len(), prefix_table.len());
    #[cfg(test)]
    let mut comparisons = 0;
    if needle.is_empty() || needle.len() > haystack.len() {
        return SearchMatch {
            found: false,
            #[cfg(test)]
            comparisons,
        };
    }

    let mut matched = 0;
    for &byte in haystack {
        loop {
            #[cfg(test)]
            {
                comparisons += 1;
            }
            if byte.eq_ignore_ascii_case(&needle[matched]) {
                matched += 1;
                if matched == needle.len() {
                    return SearchMatch {
                        found: true,
                        #[cfg(test)]
                        comparisons,
                    };
                }
                break;
            }
            if matched == 0 {
                break;
            }
            matched = prefix_table[matched - 1];
        }
    }

    SearchMatch {
        found: false,
        #[cfg(test)]
        comparisons,
    }
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

fn build_browser_rows(
    index: &ScanIndex,
    rows: &mut Vec<BrowserRow>,
    row_limit: usize,
) -> AppResult<()> {
    push_browser_row(rows, BrowserRow::LibraryRoot, row_limit)?;
    let mut previous = None;
    let mut entry_cursor = 0;
    while let Some(entry) = index.entries.get(entry_cursor) {
        if matches!(
            &entry.source,
            TrackEntrySource::LibraryFile { .. } | TrackEntrySource::LibrarySymlink { .. }
        ) {
            append_tree_entry(index, rows, entry_cursor, 1, 1, &mut previous, row_limit)?;
            entry_cursor += 1;
        } else {
            break;
        }
    }

    push_browser_row(rows, BrowserRow::PlaylistsRoot, row_limit)?;
    for (playlist_index, playlist) in index.playlists.iter().enumerate() {
        push_browser_row(rows, BrowserRow::Playlist { playlist_index }, row_limit)?;
        let mut previous = None;
        while index
            .entries
            .get(entry_cursor)
            .is_some_and(|entry| entry_playlist(entry) == Some(playlist.id))
        {
            append_tree_entry(index, rows, entry_cursor, 2, 2, &mut previous, row_limit)?;
            entry_cursor += 1;
        }
    }
    Ok(())
}

fn append_tree_entry(
    index: &ScanIndex,
    rows: &mut Vec<BrowserRow>,
    entry_index: usize,
    skipped_components: usize,
    base_indent: usize,
    previous: &mut Option<(usize, usize)>,
    row_limit: usize,
) -> AppResult<()> {
    let path = &index.entries[entry_index].display_path;
    let folder_count = normal_component_count(path).saturating_sub(skipped_components + 1);
    let common = previous.map_or(0, |(previous_index, previous_folders)| {
        common_folder_count(
            &index.entries[previous_index].display_path,
            previous_folders,
            path,
            folder_count,
            skipped_components,
        )
    });
    for depth in common + 1..=folder_count {
        push_browser_row(
            rows,
            BrowserRow::Folder {
                entry_index,
                component_index: skipped_components + depth - 1,
                indent: base_indent + depth - 1,
            },
            row_limit,
        )?;
    }
    push_browser_row(
        rows,
        BrowserRow::Track {
            entry_index,
            indent: base_indent + folder_count,
        },
        row_limit,
    )?;
    *previous = Some((entry_index, folder_count));
    Ok(())
}

fn push_browser_row(
    rows: &mut Vec<BrowserRow>,
    row: BrowserRow,
    row_limit: usize,
) -> AppResult<()> {
    if rows.len() == row_limit {
        return Err(AppError::Resource(
            "library browser exceeded its scanner-derived row bound".into(),
        ));
    }
    rows.push(row);
    Ok(())
}

fn common_folder_count(
    left: &Path,
    left_folders: usize,
    right: &Path,
    right_folders: usize,
    skipped_components: usize,
) -> usize {
    let mut common = 0;
    let limit = left_folders.min(right_folders);
    while common < limit
        && normal_component(left, skipped_components + common)
            == normal_component(right, skipped_components + common)
    {
        common += 1;
    }
    common
}

pub(crate) fn normal_component(path: &Path, index: usize) -> Option<&OsStr> {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value),
            Component::Prefix(_)
            | Component::RootDir
            | Component::CurDir
            | Component::ParentDir => None,
        })
        .nth(index)
}

fn normal_component_count(path: &Path) -> usize {
    path.components()
        .filter(|component| matches!(component, Component::Normal(_)))
        .count()
}

fn entry_playlist(entry: &TrackEntry) -> Option<PlaylistId> {
    match &entry.source {
        TrackEntrySource::PlaylistCopy { playlist }
        | TrackEntrySource::PlaylistSymlink { playlist, .. } => Some(*playlist),
        TrackEntrySource::LibraryFile { .. } | TrackEntrySource::LibrarySymlink { .. } => None,
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
    let result = crate::scan::scan_from(root.descriptor(), &root.path, config, 1)
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
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::{
        AppState, BrowserRow, ColorMode, Focus, PlaybackIntent, PlaybackStatus, RepeatMode,
        StatusKind, contains_ascii_case_insensitive, rebuild_ascii_case_prefix,
        reserve_startup_open_files,
    };
    use crate::audio::{AudioEvent, AudioFormat, AudioPosition};
    use crate::config::Config;
    use crate::input::AppAction;
    use crate::model::{
        FileIdentity, MediaAsset, MediaAssetId, Playlist, PlaylistId, ScanCounters, ScanIndex,
        SearchFields, TrackEntry, TrackEntryId, TrackEntrySource, TrackTags,
    };

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

    fn fixture_index() -> ScanIndex {
        let playlist_id = PlaylistId(30);
        ScanIndex {
            generation: 7,
            complete: true,
            assets: fixture_assets(),
            entries: fixture_entries(playlist_id),
            playlists: vec![Playlist {
                id: playlist_id,
                name: "Favorites".into(),
                path: PathBuf::from("playlists/Favorites"),
                entries: vec![TrackEntryId(40), TrackEntryId(50)],
            }],
            warnings: Vec::new(),
            counters: ScanCounters {
                encountered_entries: 7,
                ..ScanCounters::default()
            },
        }
    }

    fn fixture_assets() -> Vec<MediaAsset> {
        let asset = |id, inode, path: &str, artist: &str, album: &str, title: &str| MediaAsset {
            id: MediaAssetId(id),
            canonical_path: PathBuf::from(path),
            tags: TrackTags {
                artist: Some(artist.into()),
                album_artist: None,
                album: Some(album.into()),
                title: Some(title.into()),
            },
            file_identity: FileIdentity {
                device: 1,
                inode,
                size: 100,
                modified_seconds: 10,
                modified_nanoseconds: 0,
            },
            external: false,
            cross_mount: false,
        };
        vec![
            asset(
                1,
                1,
                "audio/library/JDR/Campaign One/night.mp3",
                "Calm Artist",
                "Campaign One",
                "Night Song",
            ),
            asset(
                2,
                2,
                "audio/library/Music/quiet.flac",
                "Still Artist",
                "Quiet Album",
                "Quiet Song",
            ),
        ]
    }

    fn fixture_entries(playlist_id: PlaylistId) -> Vec<TrackEntry> {
        let entry = |id, asset_index, path: &str, source| TrackEntry {
            id: TrackEntryId(id),
            asset_index,
            display_path: PathBuf::from(path),
            source,
            search: SearchFields {
                filename: PathBuf::from(path)
                    .file_stem()
                    .expect("fixture filename")
                    .to_string_lossy()
                    .into_owned(),
                relative_path: path.into(),
            },
            scan_generation: 7,
        };
        vec![
            entry(
                10,
                0,
                "library/JDR/Campaign One/night.mp3",
                TrackEntrySource::LibraryFile {
                    relative_path: PathBuf::from("library/JDR/Campaign One/night.mp3"),
                },
            ),
            entry(
                20,
                1,
                "library/Music/quiet.flac",
                TrackEntrySource::LibraryFile {
                    relative_path: PathBuf::from("library/Music/quiet.flac"),
                },
            ),
            entry(
                40,
                0,
                "playlists/Favorites/night.mp3",
                TrackEntrySource::PlaylistCopy {
                    playlist: playlist_id,
                },
            ),
            entry(
                50,
                1,
                "playlists/Favorites/quiet.flac",
                TrackEntrySource::PlaylistCopy {
                    playlist: playlist_id,
                },
            ),
        ]
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn select_entry(app: &mut AppState, entry_index: usize) {
        app.library_selection = app
            .browser_rows
            .iter()
            .position(|row| {
                matches!(
                    row,
                    BrowserRow::Track {
                        entry_index: candidate,
                        ..
                    } if *candidate == entry_index
                )
            })
            .expect("fixture entry has a browser row");
    }

    #[test]
    fn focus_cycles_both_directions() {
        let mut app = AppState::new(&Config::default(), empty_index()).expect("app state");
        assert_eq!(app.focus, Focus::Library);
        app.apply(AppAction::FocusNext);
        assert_eq!(app.focus, Focus::Player);
        app.apply(AppAction::FocusNext);
        assert_eq!(app.focus, Focus::Queue);
        app.apply(AppAction::FocusNext);
        assert_eq!(app.focus, Focus::Library);
        app.apply(AppAction::FocusPrevious);
        assert_eq!(app.focus, Focus::Queue);
        app.apply(AppAction::FocusPrevious);
        assert_eq!(app.focus, Focus::Player);
        app.apply(AppAction::FocusPrevious);
        assert_eq!(app.focus, Focus::Library);
    }

    #[test]
    fn help_is_modal_and_closes_without_triggering_hidden_actions() {
        let mut app = AppState::new(&Config::default(), empty_index()).expect("app state");

        app.apply(AppAction::ToggleHelp);
        assert!(app.help_visible());
        app.key(key(KeyCode::Char('x')), Duration::ZERO);
        assert!(
            !app.shuffle,
            "hidden controls stay inactive while help is open"
        );

        app.key(key(KeyCode::Esc), Duration::ZERO);
        assert!(!app.help_visible());
        assert_eq!(app.status_kind, StatusKind::Info);
        assert_eq!(app.status_message, "Help closed");
    }

    #[test]
    fn missing_creator_metadata_is_omitted() {
        let mut index = fixture_index();
        index.assets[0].tags.artist = None;
        index.assets[0].tags.album_artist = None;
        let mut app = AppState::new(&Config::default(), index).expect("app state");
        app.apply(AppAction::Activate)
            .expect("untagged fixture starts");

        assert_eq!(app.player_creator(80), "");
        assert_eq!(app.state_identity(80).1, "");
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
        assert!(reserve_startup_open_files(6).is_ok());
        assert!(reserve_startup_open_files(5).is_err());
    }

    #[test]
    fn browser_rows_preserve_nested_folders_and_folder_playlists() {
        let app = AppState::new(&Config::default(), fixture_index()).expect("app state");

        assert!(matches!(app.browser_rows[0], BrowserRow::LibraryRoot));
        assert!(matches!(
            app.browser_rows[1],
            BrowserRow::Folder {
                entry_index: 0,
                component_index: 1,
                indent: 1
            }
        ));
        assert!(matches!(
            app.browser_rows[2],
            BrowserRow::Folder {
                entry_index: 0,
                component_index: 2,
                indent: 2
            }
        ));
        assert!(
            app.browser_rows
                .iter()
                .any(|row| matches!(row, BrowserRow::Playlist { playlist_index: 0 }))
        );
        assert_eq!(app.library_selection, 3, "the first track starts selected");
    }

    #[test]
    fn browser_refuses_an_index_that_exceeds_its_recorded_scan_bound() {
        let mut index = fixture_index();
        index.counters.encountered_entries -= 1;

        let error = AppState::new(&Config::default(), index)
            .expect_err("browser must not grow beyond its reserved bound");

        assert!(error.to_string().contains("scanner-derived row bound"));
    }

    #[test]
    fn search_is_bounded_and_matches_metadata() {
        let mut config = Config::default();
        config.search.max_results = 1;
        config.search.max_query_bytes = 8;
        let mut app = AppState::new(&config, fixture_index()).expect("app state");

        app.key(key(KeyCode::Char('/')), Duration::ZERO);
        for character in "ARTIST".chars() {
            app.key(key(KeyCode::Char(character)), Duration::ZERO);
        }
        assert_eq!(app.search.results, [0], "result count is capped at one");

        app.search.query.clear();
        for character in "campaignX".chars() {
            app.key(key(KeyCode::Char(character)), Duration::ZERO);
        }
        assert_eq!(app.search.query, "campaign");
        assert_eq!(app.status_message, "Search query limit reached");

        app.key(key(KeyCode::Esc), Duration::ZERO);
        assert!(!app.search.active);
        assert!(app.search.query.is_empty());
        assert!(app.search.results.is_empty());
    }

    #[test]
    fn search_independently_matches_filename_and_relative_path() {
        let mut index = fixture_index();
        for entry in &mut index.entries {
            entry.search.filename = "unrelated-file".into();
            entry.search.relative_path = "unrelated/path".into();
        }
        for asset in &mut index.assets {
            asset.tags = TrackTags {
                artist: Some("unrelated metadata".into()),
                album_artist: None,
                album: None,
                title: None,
            };
        }
        index.entries[0].search.filename = "filename-only-token".into();
        index.entries[1].search.relative_path = "folder/path-only-token.flac".into();
        let mut app = AppState::new(&Config::default(), index).expect("app state");

        app.key(key(KeyCode::Char('/')), Duration::ZERO);
        for character in "filename-only-token".chars() {
            app.key(key(KeyCode::Char(character)), Duration::ZERO);
        }
        assert_eq!(app.search.results, [0]);

        app.key(
            KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
            Duration::ZERO,
        );
        for character in "path-only-token".chars() {
            app.key(key(KeyCode::Char(character)), Duration::ZERO);
        }
        assert_eq!(app.search.results, [1]);
    }

    #[test]
    fn q_is_search_text_while_ctrl_c_is_global() {
        let mut app = AppState::new(&Config::default(), fixture_index()).expect("app state");

        app.key(key(KeyCode::Char('/')), Duration::ZERO);
        app.key(key(KeyCode::Char('q')), Duration::ZERO);
        assert_eq!(app.search.query, "q");
        assert!(!app.should_quit);

        app.key(
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            Duration::ZERO,
        );
        assert!(app.should_quit);
    }

    #[test]
    fn ascii_case_search_stays_linear_on_repetitive_near_matches() {
        let haystack = vec![b'a'; 16_384];
        let mut needle = vec![b'a'; 1_024];
        *needle.last_mut().expect("non-empty needle") = b'b';
        let mut prefix_table = Vec::new();
        rebuild_ascii_case_prefix(&needle, &mut prefix_table);

        let result = contains_ascii_case_insensitive(&haystack, &needle, &prefix_table);

        assert!(!result.found);
        assert!(
            result.comparisons <= haystack.len().saturating_mul(2),
            "{} comparisons exceeded the linear bound",
            result.comparisons
        );
    }

    #[test]
    fn activating_a_playlist_queues_its_tracks_as_one_mutation() {
        let mut app = AppState::new(&Config::default(), fixture_index()).expect("app state");
        app.library_selection = app
            .browser_rows
            .iter()
            .position(|row| matches!(row, BrowserRow::Playlist { playlist_index: 0 }))
            .expect("fixture playlist has a browser row");

        let start = app.apply(AppAction::Activate);

        assert_eq!(app.queue.len(), 2);
        assert_eq!(app.queue[0].entry_id, TrackEntryId(40));
        assert_eq!(app.queue[1].entry_id, TrackEntryId(50));
        assert_eq!(app.queue_generation, 1);
        assert!(
            matches!(start, Some(PlaybackIntent::Load { item, .. }) if item.entry_id == TrackEntryId(40))
        );
        assert_eq!(app.status_message, "Loading: Night Song");

        let mut config = Config::default();
        config.queue.max_items = 1;
        config.queue.max_bytes = 32;
        let mut limited = AppState::new(&config, fixture_index()).expect("limited app state");
        limited.library_selection = limited
            .browser_rows
            .iter()
            .position(|row| matches!(row, BrowserRow::Playlist { playlist_index: 0 }))
            .expect("fixture playlist has a browser row");
        limited.apply(AppAction::Activate);
        assert!(limited.queue.is_empty(), "a rejected playlist stays atomic");
        assert_eq!(limited.queue_generation, 0);
        assert_eq!(limited.status_message, "Queue limit reached");
    }

    #[test]
    fn queue_mutations_are_bounded_and_advance_generation_once() {
        let mut config = Config::default();
        config.queue.max_items = 2;
        config.queue.max_bytes = 64;
        let mut app = AppState::new(&config, fixture_index()).expect("app state");

        select_entry(&mut app, 0);
        app.apply(AppAction::Activate);
        select_entry(&mut app, 1);
        app.apply(AppAction::Activate);
        select_entry(&mut app, 2);
        app.apply(AppAction::Activate);
        assert_eq!(app.queue.len(), 2);
        assert_eq!(app.queue_generation, 2);
        assert_eq!(app.status_message, "Queue limit reached");
        assert_eq!(app.queue[0].entry_id, TrackEntryId(10));
        assert_eq!(app.queue[0].scan_generation, 7);
        assert_eq!(app.queue[1].entry_id, TrackEntryId(20));

        app.focus = Focus::Queue;
        app.apply(AppAction::QueueMoveDown);
        assert_eq!(app.queue[0].entry_id, TrackEntryId(20));
        assert_eq!(app.queue_selection, 1);
        assert_eq!(app.queue_generation, 3);

        app.apply(AppAction::QueueRemove);
        assert_eq!(app.queue.len(), 1);
        assert_eq!(app.queue_generation, 4);
        app.apply(AppAction::QueueClear);
        assert!(app.queue.is_empty());
        assert_eq!(app.queue_generation, 5);
        app.apply(AppAction::QueueClear);
        assert_eq!(app.queue_generation, 5, "an empty clear is not a mutation");
    }

    #[test]
    fn playback_generations_and_queue_navigation_stay_in_the_app_loop() {
        let mut app = AppState::new(&Config::default(), fixture_index()).expect("app state");
        select_entry(&mut app, 0);
        let first = app
            .apply(AppAction::Activate)
            .expect("idle activation starts the selected track");
        select_entry(&mut app, 1);
        app.apply(AppAction::Activate);

        let PlaybackIntent::Load {
            generation: first_generation,
            item: first_item,
            ..
        } = first
        else {
            panic!("play must load the selected queue item");
        };
        assert_eq!(first_generation, 1);
        assert_eq!(first_item.entry_id, TrackEntryId(10));
        assert_eq!(app.playback_status, PlaybackStatus::Loading);

        app.audio_event(AudioEvent::Started {
            generation: first_generation,
            timeline_revision: 1,
            format: AudioFormat {
                sample_rate: 48_000,
                channels: 2,
            },
            duration: Some(Duration::from_secs(10)),
            position: Duration::ZERO,
        });
        assert_eq!(app.playback_status, PlaybackStatus::Playing);
        assert!(matches!(
            app.apply(AppAction::PlayPause),
            Some(PlaybackIntent::Pause { generation: 1 })
        ));
        app.audio_event(AudioEvent::Paused { generation: 1 });
        assert_eq!(app.playback_status, PlaybackStatus::Paused);
        assert!(matches!(
            app.apply(AppAction::PlayPause),
            Some(PlaybackIntent::Resume { generation: 1 })
        ));

        let second = app.apply(AppAction::Next).expect("load next track");
        assert!(matches!(
            second,
            PlaybackIntent::Load {
                generation: 2,
                item,
                ..
            } if item.entry_id == TrackEntryId(20)
        ));
        app.audio_event(AudioEvent::Finished { generation: 1 });
        assert_eq!(
            app.playback_status,
            PlaybackStatus::Loading,
            "a stale completion cannot replace the newer load"
        );

        let previous = app
            .apply(AppAction::Previous)
            .expect("return to first track");
        assert!(matches!(
            previous,
            PlaybackIntent::Load {
                generation: 3,
                item,
                ..
            } if item.entry_id == TrackEntryId(10)
        ));
        let automatic = app
            .audio_event(AudioEvent::Finished { generation: 3 })
            .expect("completion advances to the next queue item");
        assert!(matches!(
            automatic,
            PlaybackIntent::Load {
                generation: 4,
                item,
                ..
            } if item.entry_id == TrackEntryId(20)
        ));
        assert!(
            app.audio_event(AudioEvent::Finished { generation: 4 })
                .is_none()
        );
        assert_eq!(app.playback_status, PlaybackStatus::Stopped);
        assert_eq!(app.status_message, "Queue finished");
    }

    #[test]
    fn removing_the_current_queue_item_keeps_the_next_position_deterministic() {
        let mut app = AppState::new(&Config::default(), fixture_index()).expect("app state");
        select_entry(&mut app, 0);
        app.apply(AppAction::Activate);
        select_entry(&mut app, 1);
        app.apply(AppAction::Activate);

        app.focus = Focus::Queue;
        app.queue_selection = 0;
        app.apply(AppAction::QueueRemove);
        let next = app
            .audio_event(AudioEvent::Finished { generation: 1 })
            .expect("removed current item advances to its successor");

        assert!(matches!(
            next,
            PlaybackIntent::Load { item, .. } if item.entry_id == TrackEntryId(20)
        ));
    }

    #[test]
    fn stop_clears_a_failed_playback_without_waiting_for_a_dead_worker_track() {
        let mut app = AppState::new(&Config::default(), fixture_index()).expect("app state");
        select_entry(&mut app, 0);
        app.apply(AppAction::Activate);
        app.audio_event(AudioEvent::Failed {
            generation: 1,
            message: "fixture failure".into(),
        });

        assert!(app.apply(AppAction::Stop).is_none());
        assert_eq!(app.playback_status, PlaybackStatus::Stopped);
        assert_eq!(app.status_message, "Stopped");
    }

    #[test]
    fn playback_controls_are_bounded_and_keep_position_in_app_state() {
        let mut app = AppState::new(&Config::default(), fixture_index()).expect("app state");
        select_entry(&mut app, 0);
        app.apply(AppAction::Activate)
            .expect("idle Enter starts playback");
        app.audio_event(AudioEvent::Started {
            generation: 1,
            timeline_revision: 1,
            format: AudioFormat {
                sample_rate: 48_000,
                channels: 2,
            },
            duration: Some(Duration::from_secs(30)),
            position: Duration::ZERO,
        });
        app.audio_position(AudioPosition {
            generation: 1,
            timeline_revision: 1,
            position: Duration::from_secs(10),
            duration: Some(Duration::from_secs(30)),
        });

        assert!(matches!(
            app.apply(AppAction::SeekBackward),
            Some(PlaybackIntent::Seek { position, .. }) if position == Duration::from_secs(5)
        ));
        assert!(matches!(
            app.apply(AppAction::SeekForward),
            Some(PlaybackIntent::Seek { position, .. }) if position == Duration::from_secs(10)
        ));
        assert!(matches!(
            app.apply(AppAction::VolumeDown),
            Some(PlaybackIntent::SetGain {
                volume_percent: 95,
                muted: false,
                ..
            })
        ));
        assert!(matches!(
            app.apply(AppAction::ToggleMute),
            Some(PlaybackIntent::SetGain {
                volume_percent: 95,
                muted: true,
                ..
            })
        ));
    }

    #[test]
    fn external_controls_use_exact_idempotent_app_actions() {
        let mut app = AppState::new(&Config::default(), fixture_index()).expect("app state");
        select_entry(&mut app, 0);
        app.apply(AppAction::Activate)
            .expect("idle Enter starts playback");
        let queue_instance = app.queue[0].instance_id;
        app.audio_event(AudioEvent::Started {
            generation: 1,
            timeline_revision: 1,
            format: AudioFormat {
                sample_rate: 48_000,
                channels: 2,
            },
            duration: Some(Duration::from_secs(90)),
            position: Duration::from_secs(20),
        });

        assert!(app.apply(AppAction::Play).is_none());
        assert!(matches!(
            app.apply(AppAction::Pause),
            Some(PlaybackIntent::Pause { generation: 1 })
        ));
        app.audio_event(AudioEvent::Paused { generation: 1 });
        assert!(app.apply(AppAction::Pause).is_none());
        assert!(matches!(
            app.apply(AppAction::Play),
            Some(PlaybackIntent::Resume { generation: 1 })
        ));

        app.muted = true;
        assert!(matches!(
            app.apply(AppAction::SetVolume(37)),
            Some(PlaybackIntent::SetGain {
                generation: 1,
                volume_percent: 37,
                muted: false,
            })
        ));
        assert_eq!(app.volume_percent, 37);
        assert!(!app.muted);

        app.apply(AppAction::SetShuffle(true));
        app.apply(AppAction::SetShuffle(true));
        assert!(app.shuffle);
        app.apply(AppAction::RepeatOne);
        assert_eq!(app.repeat, RepeatMode::One);
        app.apply(AppAction::RepeatAll);
        assert_eq!(app.repeat, RepeatMode::All);
        app.apply(AppAction::RepeatOff);
        assert_eq!(app.repeat, RepeatMode::Off);

        assert!(matches!(
            app.apply(AppAction::SeekRelative {
                forward: false,
                distance: Duration::from_secs(7),
            }),
            Some(PlaybackIntent::Seek { position, .. })
                if position == Duration::from_secs(13)
        ));
        assert!(
            app.apply(AppAction::SeekAbsolute {
                playback_generation: 2,
                queue_instance,
                position: Duration::from_secs(30),
            })
            .is_none()
        );
    }

    #[test]
    fn next_and_previous_preserve_paused_playback() {
        let mut app = AppState::new(&Config::default(), fixture_index()).expect("app state");
        select_entry(&mut app, 0);
        app.apply(AppAction::Activate)
            .expect("idle Enter starts playback");
        select_entry(&mut app, 1);
        assert!(app.apply(AppAction::Activate).is_none());
        app.audio_event(AudioEvent::Started {
            generation: 1,
            timeline_revision: 1,
            format: AudioFormat {
                sample_rate: 48_000,
                channels: 2,
            },
            duration: Some(Duration::from_secs(90)),
            position: Duration::from_secs(20),
        });
        app.audio_event(AudioEvent::Paused { generation: 1 });

        assert!(matches!(
            app.apply(AppAction::Next),
            Some(PlaybackIntent::Load {
                generation: 2,
                item,
                paused: true,
                ..
            }) if item.entry_id == TrackEntryId(20)
        ));
        app.audio_event(AudioEvent::Started {
            generation: 2,
            timeline_revision: 1,
            format: AudioFormat {
                sample_rate: 48_000,
                channels: 2,
            },
            duration: Some(Duration::from_secs(80)),
            position: Duration::ZERO,
        });
        assert_eq!(app.playback_status, PlaybackStatus::Paused);

        assert!(matches!(
            app.apply(AppAction::Previous),
            Some(PlaybackIntent::Load {
                generation: 3,
                item,
                paused: true,
                ..
            }) if item.entry_id == TrackEntryId(10)
        ));
        app.audio_event(AudioEvent::Started {
            generation: 3,
            timeline_revision: 1,
            format: AudioFormat {
                sample_rate: 48_000,
                channels: 2,
            },
            duration: Some(Duration::from_secs(90)),
            position: Duration::ZERO,
        });
        assert_eq!(app.playback_status, PlaybackStatus::Paused);
    }

    #[test]
    fn next_and_previous_only_select_tracks_while_stopped() {
        let mut app = AppState::new(&Config::default(), fixture_index()).expect("app state");
        select_entry(&mut app, 0);
        app.apply(AppAction::Activate)
            .expect("idle Enter starts playback");
        select_entry(&mut app, 1);
        assert!(app.apply(AppAction::Activate).is_none());
        app.audio_event(AudioEvent::Stopped { generation: 1 });

        assert!(app.apply(AppAction::Next).is_none());
        assert_eq!(app.playback_status, PlaybackStatus::Stopped);
        assert_eq!(app.queue_position(), Some(2));
        assert!(app.apply(AppAction::Previous).is_none());
        assert_eq!(app.playback_status, PlaybackStatus::Stopped);
        assert_eq!(app.queue_position(), Some(1));
    }

    #[test]
    fn relative_seek_beyond_the_end_advances_to_the_next_track() {
        let mut app = AppState::new(&Config::default(), fixture_index()).expect("app state");
        select_entry(&mut app, 0);
        app.apply(AppAction::Activate)
            .expect("idle Enter starts playback");
        select_entry(&mut app, 1);
        assert!(app.apply(AppAction::Activate).is_none());
        app.audio_event(AudioEvent::Started {
            generation: 1,
            timeline_revision: 1,
            format: AudioFormat {
                sample_rate: 48_000,
                channels: 2,
            },
            duration: Some(Duration::from_secs(10)),
            position: Duration::from_secs(8),
        });

        assert!(matches!(
            app.apply(AppAction::SeekRelative {
                forward: true,
                distance: Duration::from_secs(5),
            }),
            Some(PlaybackIntent::Load {
                generation: 2,
                item,
                ..
            }) if item.entry_id == TrackEntryId(20)
        ));
    }

    #[test]
    fn absolute_seek_beyond_the_duration_is_ignored() {
        let mut app = AppState::new(&Config::default(), fixture_index()).expect("app state");
        select_entry(&mut app, 0);
        app.apply(AppAction::Activate)
            .expect("idle Enter starts playback");
        let queue_instance = app.queue[0].instance_id;
        app.audio_event(AudioEvent::Started {
            generation: 1,
            timeline_revision: 1,
            format: AudioFormat {
                sample_rate: 48_000,
                channels: 2,
            },
            duration: Some(Duration::from_secs(90)),
            position: Duration::from_secs(20),
        });

        assert!(
            app.apply(AppAction::SeekAbsolute {
                playback_generation: 1,
                queue_instance,
                position: Duration::from_secs(91),
            })
            .is_none()
        );
        assert_eq!(app.playback_position(), Duration::from_secs(20));
        assert!(matches!(
            app.apply(AppAction::SeekAbsolute {
                playback_generation: 1,
                queue_instance,
                position: Duration::from_secs(90),
            }),
            Some(PlaybackIntent::Seek { position, .. })
                if position == Duration::from_secs(90)
        ));
    }

    #[test]
    fn repeated_seeks_keep_the_latest_target_until_it_is_acknowledged() {
        let mut app = AppState::new(&Config::default(), fixture_index()).expect("app state");
        select_entry(&mut app, 0);
        app.apply(AppAction::Activate)
            .expect("idle Enter starts playback");
        app.audio_event(AudioEvent::Started {
            generation: 1,
            timeline_revision: 1,
            format: AudioFormat {
                sample_rate: 48_000,
                channels: 2,
            },
            duration: Some(Duration::from_secs(200)),
            position: Duration::from_secs(10),
        });

        let mut final_intent = None;
        for _ in 0..20 {
            final_intent = app.apply(AppAction::SeekForward);
        }
        assert!(matches!(
            final_intent,
            Some(PlaybackIntent::Seek { position, .. })
                if position == Duration::from_secs(110)
        ));

        app.audio_position(AudioPosition {
            generation: 1,
            timeline_revision: 1,
            position: Duration::from_secs(12),
            duration: Some(Duration::from_secs(200)),
        });
        app.audio_event(AudioEvent::Seeked {
            generation: 1,
            timeline_revision: 2,
            position: Duration::from_secs(15),
        });
        assert_eq!(app.playback_position(), Duration::from_secs(110));

        app.audio_event(AudioEvent::Seeked {
            generation: 1,
            timeline_revision: 3,
            position: Duration::from_secs(110),
        });
        app.audio_position(AudioPosition {
            generation: 1,
            timeline_revision: 3,
            position: Duration::from_secs(111),
            duration: Some(Duration::from_secs(200)),
        });
        assert_eq!(app.playback_position(), Duration::from_secs(111));
    }

    #[test]
    fn timing_revisions_prevent_cross_lane_position_regressions() {
        let mut app = AppState::new(&Config::default(), fixture_index()).expect("app state");
        select_entry(&mut app, 0);
        app.apply(AppAction::Activate)
            .expect("idle Enter starts playback");
        app.audio_position(AudioPosition {
            generation: 1,
            timeline_revision: 1,
            position: Duration::from_secs(2),
            duration: Some(Duration::from_secs(30)),
        });

        app.audio_event(AudioEvent::Started {
            generation: 1,
            timeline_revision: 1,
            format: AudioFormat {
                sample_rate: 48_000,
                channels: 2,
            },
            duration: Some(Duration::from_secs(30)),
            position: Duration::ZERO,
        });
        assert_eq!(app.playback_position(), Duration::from_secs(2));

        app.audio_position(AudioPosition {
            generation: 1,
            timeline_revision: 2,
            position: Duration::from_secs(12),
            duration: Some(Duration::from_secs(30)),
        });
        app.audio_event(AudioEvent::Seeked {
            generation: 1,
            timeline_revision: 2,
            position: Duration::from_secs(5),
        });
        app.audio_position(AudioPosition {
            generation: 1,
            timeline_revision: 1,
            position: Duration::from_secs(20),
            duration: Some(Duration::from_secs(30)),
        });
        assert_eq!(app.playback_position(), Duration::from_secs(12));

        app.audio_event(AudioEvent::Stopped { generation: 1 });
        app.audio_position(AudioPosition {
            generation: 1,
            timeline_revision: 2,
            position: Duration::from_secs(20),
            duration: Some(Duration::from_secs(30)),
        });
        assert_eq!(app.playback_position(), Duration::ZERO);
    }

    #[test]
    fn shuffle_is_a_permutation_and_repeat_modes_choose_in_the_app_loop() {
        let mut app = AppState::new(&Config::default(), fixture_index()).expect("app state");
        app.shuffle_seed = 1;
        select_entry(&mut app, 0);
        app.apply(AppAction::Activate);
        select_entry(&mut app, 1);
        app.apply(AppAction::Activate);
        select_entry(&mut app, 2);
        app.apply(AppAction::Activate);

        app.apply(AppAction::ToggleShuffle);
        let mut expected: Vec<_> = app.queue.iter().map(|item| item.instance_id).collect();
        let mut actual = app.shuffle_order.clone();
        expected.sort_unstable();
        actual.sort_unstable();
        assert_eq!(actual, expected);
        assert_eq!(
            app.shuffle_order.first().copied(),
            app.playback.current.map(|item| item.instance_id),
            "enabling shuffle keeps the current track at the head of the round"
        );

        app.apply(AppAction::CycleRepeat);
        assert_eq!(app.repeat, RepeatMode::All);
        app.apply(AppAction::CycleRepeat);
        assert_eq!(app.repeat, RepeatMode::One);
        let current = app.current_queue_index().expect("current queue item");
        let repeated = app
            .audio_event(AudioEvent::Finished { generation: 1 })
            .expect("repeat one reloads the same queue item");
        assert!(matches!(
            repeated,
            PlaybackIntent::Load { item, .. } if item.instance_id == app.queue[current].instance_id
        ));

        app.apply(AppAction::CycleRepeat);
        assert_eq!(app.repeat, RepeatMode::Off);
    }

    #[test]
    fn saved_session_restores_available_queue_entries_at_position() {
        let mut app = AppState::new(&Config::default(), fixture_index()).expect("app state");
        let snapshot = crate::state::SessionSnapshot::new(vec![10, 999, 20, 10], 2, 123_400);

        app.restore_session(&snapshot).expect("restore session");

        assert_eq!(
            app.queue
                .iter()
                .map(|item| item.entry_id)
                .collect::<Vec<_>>(),
            [TrackEntryId(10), TrackEntryId(20), TrackEntryId(10)]
        );
        assert_eq!(app.queue_selection, 1);
        assert_eq!(app.playback_status, PlaybackStatus::Stopped);
        assert_eq!(app.playback_position(), Duration::from_millis(123_400));
        assert!(app.status_message.contains("1 missing track"));

        assert!(matches!(
            app.apply(AppAction::PlayPause),
            Some(PlaybackIntent::Load {
                item,
                position,
                settings,
                ..
            }) if item.entry_id == TrackEntryId(20)
                && position == Duration::from_millis(123_400)
                && settings == crate::audio::PlaybackSettings::default()
        ));
    }

    #[test]
    fn clearing_the_queue_removes_the_resumable_session() {
        let mut app = AppState::new(&Config::default(), fixture_index()).expect("app state");
        select_entry(&mut app, 0);
        app.apply(AppAction::Activate);
        assert!(app.session_snapshot().expect("session snapshot").is_some());

        app.focus = Focus::Queue;
        app.apply(AppAction::QueueClear);

        assert!(app.session_snapshot().expect("cleared snapshot").is_none());
    }

    #[test]
    fn a_finished_track_resumes_from_the_beginning() {
        let mut app = AppState::new(&Config::default(), fixture_index()).expect("app state");
        select_entry(&mut app, 0);
        app.apply(AppAction::Activate);
        app.audio_event(AudioEvent::Started {
            generation: 1,
            timeline_revision: 1,
            format: AudioFormat {
                sample_rate: 48_000,
                channels: 2,
            },
            duration: Some(Duration::from_secs(10)),
            position: Duration::ZERO,
        });
        app.audio_event(AudioEvent::Finished { generation: 1 });

        let snapshot = app
            .session_snapshot()
            .expect("session snapshot")
            .expect("saved queue");

        assert_eq!(snapshot.position_ms, 0);
    }

    #[test]
    fn shuffled_playlist_starts_at_its_first_track_without_skipping_the_round() {
        let mut app = AppState::new(&Config::default(), fixture_index()).expect("app state");
        app.shuffle = true;
        app.shuffle_seed = 2;
        app.library_selection = app
            .browser_rows
            .iter()
            .position(|row| matches!(row, BrowserRow::Playlist { playlist_index: 0 }))
            .expect("fixture playlist has a browser row");

        let first = app.apply(AppAction::Activate).expect("playlist starts");
        let PlaybackIntent::Load {
            generation,
            item: first,
            ..
        } = first
        else {
            panic!("playlist activation must load its first track");
        };
        assert_eq!(first.entry_id, TrackEntryId(40));
        assert_eq!(app.shuffle_order.first(), Some(&first.instance_id));

        let second = app
            .audio_event(AudioEvent::Finished { generation })
            .expect("shuffle round retains the other playlist track");
        let PlaybackIntent::Load {
            generation,
            item: second,
            ..
        } = second
        else {
            panic!("the remaining playlist track must load");
        };
        assert_ne!(second.instance_id, first.instance_id);
        assert!(
            app.audio_event(AudioEvent::Finished { generation })
                .is_none()
        );
        assert_eq!(app.playback_status, PlaybackStatus::Stopped);
    }

    #[test]
    fn queue_edits_preserve_the_played_shuffle_prefix() {
        let mut app = AppState::new(&Config::default(), fixture_index()).expect("app state");
        app.shuffle_seed = 1;
        for entry_index in 0..3 {
            select_entry(&mut app, entry_index);
            app.apply(AppAction::Activate);
        }
        app.apply(AppAction::ToggleShuffle);
        app.apply(AppAction::Next).expect("advance within shuffle");
        let cursor = app.shuffle_cursor.expect("shuffle cursor");
        assert!(cursor > 0);
        let played_prefix = app.shuffle_order[..=cursor].to_vec();

        select_entry(&mut app, 3);
        app.apply(AppAction::Activate);
        assert_eq!(&app.shuffle_order[..=cursor], played_prefix.as_slice());

        let order_before_reorder = app.shuffle_order.clone();
        app.focus = Focus::Queue;
        app.queue_selection = 0;
        app.apply(AppAction::QueueMoveDown);
        assert_eq!(app.shuffle_order, order_before_reorder);

        let unplayed = app.shuffle_order[cursor + 1];
        app.queue_selection = app
            .queue
            .iter()
            .position(|item| item.instance_id == unplayed)
            .expect("unplayed item remains queued");
        app.apply(AppAction::QueueRemove);
        assert_eq!(&app.shuffle_order[..=cursor], played_prefix.as_slice());

        let previous = app
            .apply(AppAction::Previous)
            .expect("shuffle history remains");
        assert!(matches!(
            previous,
            PlaybackIntent::Load { item, .. }
                if item.instance_id == played_prefix[played_prefix.len() - 2]
        ));
    }
}
