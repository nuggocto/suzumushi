// SPDX-License-Identifier: Apache-2.0

//! Ordered queue with atomic asset membership and a stable played shuffle prefix.

use std::mem::size_of;
use std::ops::Range;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::reserve_exact;
use crate::config::Config;
use crate::errors::{AppError, AppResult};
use crate::model::{ScanIndex, TrackEntryId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct QueueItem {
    pub instance_id: u64,
    pub entry_index: usize,
    pub entry_id: TrackEntryId,
    pub scan_generation: u64,
}

#[derive(Debug)]
pub(super) struct QueueState {
    items: Vec<QueueItem>,
    queued_assets: Vec<u8>,
    pub(super) selection: usize,
    generation: u64,
    next_item_id: u64,
    max_items: usize,
    max_bytes: usize,
    pub(super) shuffle: bool,
    pub(super) shuffle_order: Vec<u64>,
    pub(super) shuffle_cursor: Option<usize>,
    pub(super) shuffle_seed: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum QueueError {
    Unavailable,
    Duplicate,
    Limit,
    GenerationExhausted,
}

pub(super) struct RestoredQueue {
    pub(super) current_index: usize,
    pub(super) position: Duration,
    pub(super) skipped: usize,
}

impl QueueState {
    pub(super) fn new(config: &Config, asset_count: usize) -> AppResult<Self> {
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
        let mut queued_assets = Vec::new();
        reserve_exact(&mut queued_assets, asset_count, "queued assets")?;
        queued_assets.resize(asset_count, 0);
        Ok(Self {
            items: queue,
            queued_assets,
            selection: 0,
            generation: 0,
            next_item_id: 1,
            max_items: config.queue.max_items,
            max_bytes: config.queue.max_bytes,
            shuffle: false,
            shuffle_order,
            shuffle_cursor: None,
            shuffle_seed: random_seed(),
        })
    }

    pub(super) fn items(&self) -> &[QueueItem] {
        &self.items
    }

    pub(super) const fn generation(&self) -> u64 {
        self.generation
    }

    pub(super) fn move_selection(&mut self, next: bool) {
        super::move_index(&mut self.selection, self.items.len(), next);
    }

    pub(super) fn restore(
        &mut self,
        index: &ScanIndex,
        snapshot: &crate::state::SessionSnapshot,
    ) -> AppResult<Option<RestoredQueue>> {
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
        for (entry_index, entry) in index.entries.iter().enumerate() {
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
            let asset_index = index.entries[entry_index].asset_index;
            let queue_index = match self.append(index, std::iter::once(entry_index)) {
                Ok(appended) => appended.start,
                Err(QueueError::Duplicate) => {
                    if saved_index == snapshot.current_index {
                        restored_current = self.items.iter().position(|item| {
                            index.entries[item.entry_index].asset_index == asset_index
                        });
                    }
                    continue;
                }
                Err(QueueError::Unavailable) => continue,
                Err(error) => {
                    return Err(AppError::Resource(format!(
                        "cannot restore queue: {error:?}"
                    )));
                }
            };
            if saved_index == snapshot.current_index {
                restored_current = Some(queue_index);
            }
        }

        if self.items.is_empty() {
            return Ok(None);
        }

        self.generation = 1;
        let current_index = restored_current.unwrap_or(0);
        let position = if restored_current.is_some() {
            Duration::from_millis(snapshot.position_ms)
        } else {
            Duration::ZERO
        };
        self.selection = current_index;
        Ok(Some(RestoredQueue {
            current_index,
            position,
            skipped: snapshot
                .queue_entry_ids
                .len()
                .saturating_sub(self.items.len()),
        }))
    }

    /// Preflight every entry before committing one queue edit. The temporary
    /// membership marks also reject two aliases of one asset within a playlist.
    pub(super) fn append(
        &mut self,
        index: &ScanIndex,
        entries: impl Iterator<Item = usize> + Clone,
    ) -> Result<Range<usize>, QueueError> {
        let mut count = 0;
        for entry_index in entries.clone() {
            if let Err(error) = self.mark_asset(index, entry_index) {
                self.unmark_assets(index, entries.take(count));
                return Err(error);
            }
            count += 1;
        }
        let start = self.items.len();
        if count == 0 {
            return Ok(start..start);
        }
        if let Err(error) = self.check_append(count) {
            self.unmark_assets(index, entries);
            return Err(error);
        }

        for entry_index in entries {
            let entry = &index.entries[entry_index];
            self.items.push(QueueItem {
                instance_id: self.next_item_id,
                entry_index,
                entry_id: entry.id,
                scan_generation: index.generation,
            });
            self.next_item_id += 1;
        }
        self.generation += 1;
        self.extend_shuffle_order(start);
        Ok(start..self.items.len())
    }

    fn mark_asset(&mut self, index: &ScanIndex, entry_index: usize) -> Result<(), QueueError> {
        let queued = index
            .entries
            .get(entry_index)
            .and_then(|entry| self.queued_assets.get_mut(entry.asset_index))
            .ok_or(QueueError::Unavailable)?;
        if *queued != 0 {
            return Err(QueueError::Duplicate);
        }
        *queued = 1;
        Ok(())
    }

    fn unmark_assets(&mut self, index: &ScanIndex, entries: impl Iterator<Item = usize>) {
        for entry_index in entries {
            let asset_index = index.entries[entry_index].asset_index;
            self.queued_assets[asset_index] = 0;
        }
    }

    fn check_generation(&self) -> Result<(), QueueError> {
        if self.generation == u64::MAX {
            Err(QueueError::GenerationExhausted)
        } else {
            Ok(())
        }
    }

    fn check_append(&self, count: usize) -> Result<(), QueueError> {
        let next_items = self.items.len().checked_add(count);
        let next_bytes = next_items.and_then(|items| items.checked_mul(size_of::<QueueItem>()));
        let identities_fit = u64::try_from(count)
            .ok()
            .and_then(|count| self.next_item_id.checked_add(count))
            .is_some();
        if next_items.is_none_or(|items| items > self.max_items)
            || next_bytes.is_none_or(|bytes| bytes > self.max_bytes)
            || !identities_fit
        {
            return Err(QueueError::Limit);
        }
        self.check_generation()
    }

    pub(super) fn remove_selected(
        &mut self,
        index: &ScanIndex,
    ) -> Result<Option<QueueItem>, QueueError> {
        let Some(item) = self.items.get(self.selection).copied() else {
            return Ok(None);
        };
        self.check_generation()?;
        self.items.remove(self.selection);
        self.unmark_assets(index, std::iter::once(item.entry_index));
        self.remove_from_shuffle_order(item.instance_id);
        self.selection = self.selection.min(self.items.len().saturating_sub(1));
        self.generation += 1;
        Ok(Some(item))
    }

    pub(super) fn clear(&mut self) -> Result<bool, QueueError> {
        if self.items.is_empty() {
            return Ok(false);
        }
        self.check_generation()?;
        self.items.clear();
        self.queued_assets.fill(0);
        self.shuffle_order.clear();
        self.shuffle_cursor = None;
        self.selection = 0;
        self.generation += 1;
        Ok(true)
    }

    pub(super) fn move_selected(&mut self, down: bool) -> Result<bool, QueueError> {
        let destination = if down {
            self.selection.checked_add(1)
        } else {
            self.selection.checked_sub(1)
        };
        let Some(destination) = destination.filter(|position| *position < self.items.len()) else {
            return Ok(false);
        };
        self.check_generation()?;
        self.items.swap(self.selection, destination);
        self.selection = destination;
        self.generation += 1;
        Ok(true)
    }

    pub(super) fn select_current(&mut self, item: QueueItem) {
        if self.shuffle {
            self.shuffle_cursor = self
                .shuffle_order
                .iter()
                .position(|candidate| *candidate == item.instance_id);
        }
    }

    pub(super) fn refresh_shuffle_order(&mut self, current: Option<QueueItem>) {
        self.shuffle_order.clear();
        self.shuffle_cursor = None;
        if !self.shuffle {
            return;
        }
        self.shuffle_order
            .extend(self.items.iter().map(|item| item.instance_id));
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

    pub(super) fn extend_shuffle_order(&mut self, queue_start: usize) {
        if !self.shuffle {
            return;
        }
        let suffix_start = self
            .shuffle_cursor
            .map_or(0, |cursor| cursor.saturating_add(1))
            .min(self.shuffle_order.len());
        self.shuffle_order.extend(
            self.items[queue_start.min(self.items.len())..]
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

    pub(super) fn remove_from_shuffle_order(&mut self, instance_id: u64) {
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

    pub(super) fn next_random(&mut self) -> u64 {
        let mut value = self.shuffle_seed;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.shuffle_seed = value;
        value
    }

    pub(super) fn next_index(
        &self,
        current: Option<QueueItem>,
        position_hint: usize,
        wrap: bool,
    ) -> Option<usize> {
        if self.items.is_empty() {
            return None;
        }
        if self.shuffle {
            let current = current.map(|item| item.instance_id);
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
                .items
                .iter()
                .position(|item| item.instance_id == instance_id);
        }
        let next = self.current_index(current, position_hint).map_or_else(
            || position_hint.min(self.items.len()),
            |index| index.saturating_add(1),
        );
        if next < self.items.len() {
            Some(next)
        } else if wrap {
            Some(0)
        } else {
            None
        }
    }

    pub(super) fn previous_index(
        &self,
        current: Option<QueueItem>,
        position_hint: usize,
        wrap: bool,
    ) -> Option<usize> {
        if self.items.is_empty() {
            return None;
        }
        if self.shuffle {
            let current = current.map(|item| item.instance_id)?;
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
                .items
                .iter()
                .position(|item| item.instance_id == instance_id);
        }
        let current = self
            .current_index(current, position_hint)
            .unwrap_or(self.selection);
        current
            .checked_sub(1)
            .or_else(|| wrap.then(|| self.items.len() - 1))
    }

    pub(super) fn current_index(
        &self,
        current: Option<QueueItem>,
        position_hint: usize,
    ) -> Option<usize> {
        let current = current?;
        if self
            .items
            .get(position_hint)
            .is_some_and(|item| item.instance_id == current.instance_id)
        {
            return Some(position_hint);
        }
        self.items
            .iter()
            .position(|item| item.instance_id == current.instance_id)
    }
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
