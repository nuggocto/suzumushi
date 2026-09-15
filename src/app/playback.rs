// SPDX-License-Identifier: Apache-2.0

//! Playback transitions and commands chosen by the app's event loop.

use std::time::Duration;

use super::{AppState, PlaybackIntent, PlaybackStatus, QueueItem, RepeatMode};
use crate::audio::{AudioEvent, AudioFormat, AudioPosition, AudioSpectrum, PlaybackSettings};
use crate::display::terminal_safe;

#[derive(Debug)]
pub(super) struct PlaybackState {
    pub(super) generation: u64,
    pub(super) status: PlaybackStatus,
    pub(super) seek_revision: u64,
    pub(super) current: Option<QueueItem>,
    pub(super) position_hint: usize,
    pub(super) timeline_revision: u64,
    pub(super) seek_target: Option<Duration>,
    pub(super) format: Option<AudioFormat>,
    pub(super) position: Duration,
    pub(super) duration: Option<Duration>,
    pub(super) start_paused: bool,
    pub(super) finished: bool,
    pub(super) spectrum: AudioSpectrum,
}

impl PlaybackState {
    pub(super) const fn new() -> Self {
        Self {
            generation: 0,
            status: PlaybackStatus::Stopped,
            seek_revision: 0,
            current: None,
            position_hint: 0,
            timeline_revision: 0,
            seek_target: None,
            format: None,
            position: Duration::ZERO,
            duration: None,
            start_paused: false,
            finished: false,
            spectrum: AudioSpectrum::silent(),
        }
    }

    fn select(&mut self, item: QueueItem, index: usize, position: Duration, generation: u64) {
        *self = Self {
            current: Some(item),
            position_hint: index,
            position,
            generation,
            seek_revision: self.seek_revision,
            ..Self::new()
        };
    }

    pub(super) fn load(
        &mut self,
        item: QueueItem,
        index: usize,
        position: Duration,
        paused: bool,
    ) -> Result<u64, &'static str> {
        let generation = self
            .generation
            .checked_add(1)
            .ok_or("Playback generation exhausted")?;
        self.select(item, index, position, generation);
        self.status = PlaybackStatus::Loading;
        self.start_paused = paused;
        Ok(generation)
    }

    pub(super) fn select_stopped(
        &mut self,
        item: QueueItem,
        index: usize,
    ) -> Result<(), &'static str> {
        let generation = if self.current == Some(item) {
            self.generation
        } else {
            self.generation
                .checked_add(1)
                .ok_or("Playback generation exhausted")?
        };
        self.select(item, index, Duration::ZERO, generation);
        Ok(())
    }

    pub(super) fn restore(&mut self, item: QueueItem, index: usize, position: Duration) {
        self.select(item, index, position, self.generation);
    }

    pub(super) fn resume_position(&self) -> Duration {
        if self.finished || self.duration.is_some_and(|end| self.position >= end) {
            Duration::ZERO
        } else {
            self.position
        }
    }
}

impl AppState {
    pub(crate) fn audio_position(&mut self, update: AudioPosition) {
        if update.generation != self.playback.generation
            || self.playback.current.is_none()
            || matches!(
                self.playback.status,
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
        if generation != self.playback.generation || self.playback.current.is_none() {
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
                self.playback.status = status;
                self.playback.seek_target = None;
                self.playback.format = Some(format);
                self.playback.duration = duration;
                self.playback.spectrum = AudioSpectrum::default();
                self.apply_event_position(timeline_revision, position);
                self.set_status(if status == PlaybackStatus::Paused {
                    "Paused"
                } else {
                    "Playing"
                });
                None
            }
            AudioEvent::Paused { .. } => {
                self.playback.status = PlaybackStatus::Paused;
                self.set_status("Paused");
                None
            }
            AudioEvent::Resumed { .. } => {
                self.playback.status = PlaybackStatus::Playing;
                self.set_status("Playing");
                None
            }
            AudioEvent::Stopped { .. } => {
                self.playback.status = PlaybackStatus::Stopped;
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
                    self.playback.seek_revision = self.playback.seek_revision.saturating_add(1);
                    self.set_status("Seeked");
                }
                None
            }
            AudioEvent::Finished { .. } => {
                self.playback.seek_target = None;
                self.advance_after_finish()
            }
            AudioEvent::Failed { message, .. } => {
                self.playback.status = PlaybackStatus::Error;
                self.playback.seek_target = None;
                self.playback.format = None;
                let message = terminal_safe(message.as_bytes(), self.status_text_max_bytes);
                self.set_error(&format!("Playback failed: {message}"));
                None
            }
        }
    }

    pub(super) fn play_pause(&mut self) -> Option<PlaybackIntent> {
        match self.playback.status {
            PlaybackStatus::Playing => Some(PlaybackIntent::Pause {
                generation: self.playback.generation,
            }),
            PlaybackStatus::Paused => Some(PlaybackIntent::Resume {
                generation: self.playback.generation,
            }),
            PlaybackStatus::Loading => {
                self.set_status("Track is still loading");
                None
            }
            PlaybackStatus::Stopped | PlaybackStatus::Error => {
                if self.queue.items().is_empty() {
                    self.set_status("Queue a track before starting playback");
                    None
                } else {
                    let index = self.queue.selection.min(self.queue.items().len() - 1);
                    let position = self
                        .current_queue_index()
                        .filter(|current| *current == index)
                        .map_or(Duration::ZERO, |_| self.playback.resume_position());
                    self.start_new_shuffle_round_at(index, position)
                }
            }
        }
    }

    pub(super) fn play(&mut self) -> Option<PlaybackIntent> {
        match self.playback.status {
            PlaybackStatus::Paused => Some(PlaybackIntent::Resume {
                generation: self.playback.generation,
            }),
            PlaybackStatus::Stopped | PlaybackStatus::Error => self.play_pause(),
            PlaybackStatus::Loading | PlaybackStatus::Playing => None,
        }
    }

    pub(super) fn pause(&self) -> Option<PlaybackIntent> {
        (self.playback.status == PlaybackStatus::Playing).then_some(PlaybackIntent::Pause {
            generation: self.playback.generation,
        })
    }

    pub(super) fn stop_playback(&mut self) -> Option<PlaybackIntent> {
        if self.playback.current.is_none()
            || matches!(self.playback.status, PlaybackStatus::Stopped)
        {
            self.set_status("Nothing is playing");
            return None;
        }
        if self.playback.status == PlaybackStatus::Error {
            self.playback.status = PlaybackStatus::Stopped;
            self.playback.seek_target = None;
            self.playback.format = None;
            self.set_status("Stopped");
            return None;
        }
        Some(PlaybackIntent::Stop {
            generation: self.playback.generation,
        })
    }

    pub(super) fn next_track(&mut self) -> Option<PlaybackIntent> {
        if self.queue.items().is_empty() {
            return self.stop_playback();
        }
        if let Some(next) = self.next_queue_index(self.repeat == RepeatMode::All) {
            self.queue.selection = next;
            match self.playback.status {
                PlaybackStatus::Paused => self.start_queue_index_paused(next),
                PlaybackStatus::Stopped | PlaybackStatus::Error => {
                    self.select_stopped_queue_index(next);
                    None
                }
                PlaybackStatus::Loading | PlaybackStatus::Playing => self.start_queue_index(next),
            }
        } else if matches!(
            self.playback.status,
            PlaybackStatus::Loading | PlaybackStatus::Playing | PlaybackStatus::Paused
        ) {
            self.set_status("End of queue");
            Some(PlaybackIntent::Stop {
                generation: self.playback.generation,
            })
        } else {
            self.playback.status = PlaybackStatus::Stopped;
            self.playback.format = None;
            self.set_status("End of queue");
            None
        }
    }

    pub(super) fn previous_track(&mut self) -> Option<PlaybackIntent> {
        if self.queue.items().is_empty() {
            return self.stop_playback();
        }
        let current = self.current_queue_index().unwrap_or(self.queue.selection);
        let previous = self
            .previous_queue_index(self.repeat == RepeatMode::All)
            .unwrap_or(current)
            .min(self.queue.items().len() - 1);
        self.queue.selection = previous;
        match self.playback.status {
            PlaybackStatus::Paused => self.start_queue_index_paused(previous),
            PlaybackStatus::Stopped | PlaybackStatus::Error => {
                self.select_stopped_queue_index(previous);
                None
            }
            PlaybackStatus::Loading | PlaybackStatus::Playing => self.start_queue_index(previous),
        }
    }

    pub(super) fn advance_after_finish(&mut self) -> Option<PlaybackIntent> {
        let paused = self.playback.status == PlaybackStatus::Paused;
        if self.repeat == RepeatMode::One
            && let Some(current) = self.current_queue_index()
        {
            return self.start_queue_index_with_state(current, Duration::ZERO, paused);
        }
        if let Some(next) = self.next_queue_index(self.repeat == RepeatMode::All) {
            self.queue.selection = next;
            self.start_queue_index_with_state(next, Duration::ZERO, paused)
        } else {
            self.playback.status = PlaybackStatus::Stopped;
            self.playback.format = None;
            if let Some(duration) = self.playback.duration {
                self.playback.position = duration;
            }
            self.set_status("Queue finished");
            self.playback.finished = true;
            None
        }
    }

    pub(super) fn start_queue_index(&mut self, index: usize) -> Option<PlaybackIntent> {
        self.start_queue_index_at(index, Duration::ZERO)
    }

    pub(super) fn start_queue_index_paused(&mut self, index: usize) -> Option<PlaybackIntent> {
        self.start_queue_index_with_state(index, Duration::ZERO, true)
    }

    pub(super) fn start_queue_index_at(
        &mut self,
        index: usize,
        position: Duration,
    ) -> Option<PlaybackIntent> {
        self.start_queue_index_with_state(index, position, false)
    }

    pub(super) fn start_queue_index_with_state(
        &mut self,
        index: usize,
        position: Duration,
        paused: bool,
    ) -> Option<PlaybackIntent> {
        let item = self.queue.items().get(index).copied()?;
        let generation = match self.playback.load(item, index, position, paused) {
            Ok(generation) => generation,
            Err(message) => {
                self.set_error(message);
                return None;
            }
        };
        self.queue.select_current(item);
        let title = self.queue_item_title(item, self.status_text_max_bytes);
        self.set_persistent_status(&format!("Loading: {title}"));
        Some(PlaybackIntent::Load {
            generation,
            item,
            position,
            settings: self.playback_settings(),
            paused,
        })
    }

    pub(super) fn select_stopped_queue_index(&mut self, index: usize) {
        let Some(item) = self.queue.items().get(index).copied() else {
            return;
        };
        if let Err(message) = self.playback.select_stopped(item, index) {
            self.set_error(message);
            return;
        }
        self.queue.select_current(item);
        let title = self.queue_item_title(item, self.status_text_max_bytes);
        self.set_status(&format!("Selected: {title}"));
    }

    pub(super) fn start_new_shuffle_round(&mut self, index: usize) -> Option<PlaybackIntent> {
        self.start_new_shuffle_round_at(index, Duration::ZERO)
    }

    pub(super) fn start_new_shuffle_round_at(
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

    pub(super) fn current_queue_index(&self) -> Option<usize> {
        self.queue
            .current_index(self.playback.current, self.playback.position_hint)
    }

    pub(super) fn apply_event_position(&mut self, timeline_revision: u64, position: Duration) {
        if timeline_revision > self.playback.timeline_revision {
            self.playback.timeline_revision = timeline_revision;
            self.playback.position = position;
        }
    }

    pub(super) fn next_queue_index(&self, wrap: bool) -> Option<usize> {
        self.queue
            .next_index(self.playback.current, self.playback.position_hint, wrap)
    }

    pub(super) fn previous_queue_index(&self, wrap: bool) -> Option<usize> {
        self.queue
            .previous_index(self.playback.current, self.playback.position_hint, wrap)
    }

    pub(super) const fn playback_settings(&self) -> PlaybackSettings {
        PlaybackSettings {
            volume_percent: self.volume_percent,
            muted: self.muted,
        }
    }

    pub(super) fn change_volume(&mut self, increase: bool) -> Option<PlaybackIntent> {
        self.volume_percent = if increase {
            self.volume_percent.saturating_add(5).min(100)
        } else {
            self.volume_percent.saturating_sub(5)
        };
        self.set_status(&format!("Volume: {}%", self.volume_percent));
        self.gain_intent()
    }

    pub(super) fn set_volume(&mut self, volume_percent: u8) -> Option<PlaybackIntent> {
        let volume_percent = volume_percent.min(100);
        if self.volume_percent == volume_percent && !self.muted {
            return None;
        }
        self.volume_percent = volume_percent;
        self.muted = false;
        self.set_status(&format!("Volume: {}%", self.volume_percent));
        self.gain_intent()
    }

    pub(super) fn toggle_mute(&mut self) -> Option<PlaybackIntent> {
        self.muted = !self.muted;
        self.set_status(if self.muted { "Muted" } else { "Unmuted" });
        self.gain_intent()
    }

    pub(super) fn gain_intent(&self) -> Option<PlaybackIntent> {
        self.playback.current.filter(|_| self.worker_has_track())?;
        Some(PlaybackIntent::SetGain {
            generation: self.playback.generation,
            volume_percent: self.volume_percent,
            muted: self.muted,
        })
    }

    pub(super) fn seek(&mut self, forward: bool) -> Option<PlaybackIntent> {
        self.seek_relative(forward, Duration::from_secs(5))
    }

    pub(super) fn seek_relative(
        &mut self,
        forward: bool,
        distance: Duration,
    ) -> Option<PlaybackIntent> {
        if !matches!(
            self.playback.status,
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
        let position = crate::audio::decoder_position(position);
        self.playback.seek_target = Some(position);
        self.playback.position = position;
        let status = if forward {
            format!("Seek forward {seconds} seconds")
        } else {
            format!("Seek backward {seconds} seconds")
        };
        self.set_status(&status);
        Some(PlaybackIntent::Seek {
            generation: self.playback.generation,
            position,
        })
    }

    pub(super) fn seek_absolute(
        &mut self,
        playback_generation: u64,
        queue_instance: u64,
        position: Duration,
    ) -> Option<PlaybackIntent> {
        if self.current_track_token() != Some((playback_generation, queue_instance)) {
            return None;
        }
        if !matches!(
            self.playback.status,
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
        let position = crate::audio::decoder_position(position);
        self.playback.seek_target = Some(position);
        self.playback.position = position;
        self.set_status("Seeked");
        Some(PlaybackIntent::Seek {
            generation: self.playback.generation,
            position,
        })
    }

    pub(super) fn worker_has_track(&self) -> bool {
        self.playback.current.is_some()
            && matches!(
                self.playback.status,
                PlaybackStatus::Loading | PlaybackStatus::Playing | PlaybackStatus::Paused
            )
    }

    pub(super) fn reconcile_playback_position(&mut self) {
        if let Some(index) = self.current_queue_index() {
            self.playback.position_hint = index;
        } else {
            self.playback.position_hint = self.playback.position_hint.min(self.queue.items().len());
        }
    }
}
