// SPDX-License-Identifier: Apache-2.0

//! Bounded browser rows, folder visibility, and incremental search state.

use std::ffi::OsStr;
use std::path::{Component, Path};

use crate::config::Config;
use crate::model::{PlaylistId, ScanIndex, TrackEntry, TrackEntrySource};

/// One lightweight row in the normal library browser.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BrowserRow {
    LibraryRoot,
    PlaylistsRoot,
    Folder {
        entry_index: usize,
        component_index: usize,
        indent: usize,
        collapsed: bool,
    },
    Playlist {
        playlist_index: usize,
    },
    Track {
        entry_index: usize,
        indent: usize,
    },
}

impl BrowserRow {
    const fn indent(self) -> usize {
        match self {
            Self::LibraryRoot | Self::PlaylistsRoot => 0,
            Self::Playlist { .. } => 1,
            Self::Folder { indent, .. } | Self::Track { indent, .. } => indent,
        }
    }
}

#[derive(Debug)]
pub(crate) struct SearchState {
    pub active: bool,
    pub query: String,
    pub results: Vec<usize>,
    saved_selection: usize,
    max_results: usize,
    max_query_bytes: usize,
}

#[derive(Debug)]
pub(super) struct BrowserState {
    pub(super) rows: Vec<BrowserRow>,
    visible_rows: Vec<usize>,
    pub(super) selection: usize,
    pub(super) search: SearchState,
}

impl BrowserState {
    pub(super) fn new(index: &ScanIndex, config: &Config) -> Self {
        let rows = build_browser_rows(index);
        let visible_rows = (0..rows.len()).collect();
        let selection = rows
            .iter()
            .position(|row| matches!(row, BrowserRow::Track { .. }))
            .unwrap_or(0);

        Self {
            rows,
            visible_rows,
            selection,
            search: SearchState {
                active: false,
                query: String::new(),
                results: Vec::new(),
                saved_selection: selection,
                max_results: config.search.max_results,
                max_query_bytes: config.search.max_query_bytes,
            },
        }
    }

    pub(super) fn playlist_entries(
        &self,
        position: usize,
    ) -> impl Iterator<Item = usize> + Clone + '_ {
        let start = position.saturating_add(1).min(self.rows.len());
        self.rows[start..]
            .iter()
            .take_while(|row| !matches!(row, BrowserRow::Playlist { .. }))
            .filter_map(|row| match row {
                BrowserRow::Track { entry_index, .. } => Some(*entry_index),
                _ => None,
            })
    }

    pub(super) fn row_count(&self) -> usize {
        if self.search.active && !self.search.query.is_empty() {
            self.search.results.len()
        } else {
            self.visible_rows.len()
        }
    }

    pub(super) fn row(&self, position: usize) -> Option<BrowserRow> {
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
            self.visible_rows
                .get(position)
                .and_then(|row| self.rows.get(*row))
                .copied()
        }
    }

    pub(super) fn selected_entry_index(&self) -> Option<usize> {
        match self.row(self.selection)? {
            BrowserRow::Track { entry_index, .. } => Some(entry_index),
            BrowserRow::LibraryRoot
            | BrowserRow::PlaylistsRoot
            | BrowserRow::Folder { .. }
            | BrowserRow::Playlist { .. } => None,
        }
    }

    pub(super) fn normal_position(&self, visible_position: usize) -> Option<usize> {
        if self.search.active && !self.search.query.is_empty() {
            None
        } else {
            self.visible_rows.get(visible_position).copied()
        }
    }

    pub(super) fn open_search(&mut self) {
        self.search.saved_selection = self.selection;
        self.search.active = true;
        self.search.query.clear();
        self.search.results.clear();
        self.selection = 0;
    }

    pub(super) fn close_search(&mut self) {
        self.search.active = false;
        self.search.query.clear();
        self.search.results.clear();
        self.selection = self
            .search
            .saved_selection
            .min(self.visible_rows.len().saturating_sub(1));
    }

    pub(super) fn clear_query(&mut self, index: &ScanIndex) {
        self.search.query.clear();
        self.rebuild_search(index);
    }

    pub(super) fn pop_query(&mut self, index: &ScanIndex) {
        self.search.query.pop();
        self.rebuild_search(index);
    }

    pub(super) fn push_query(&mut self, character: char, index: &ScanIndex) -> bool {
        let next_bytes = self.search.query.len().saturating_add(character.len_utf8());
        if next_bytes > self.search.max_query_bytes {
            return false;
        }
        self.search.query.push(character);
        self.rebuild_search(index);
        true
    }

    pub(super) fn rebuild_search(&mut self, index: &ScanIndex) {
        self.selection = 0;
        let SearchState {
            query,
            results,
            max_results,
            ..
        } = &mut self.search;
        results.clear();
        if query.is_empty() {
            return;
        }
        let needle = query.as_bytes();
        for (entry_index, entry) in index.entries.iter().enumerate() {
            let tags = index.asset_for_entry(entry).map(|asset| &asset.tags);
            if entry_matches(entry, tags, needle) {
                results.push(entry_index);
                if results.len() == *max_results {
                    break;
                }
            }
        }
    }

    pub(super) fn rebuild_visible_browser_rows(&mut self) {
        self.visible_rows.clear();
        let mut collapsed_indent = None;
        for (position, row) in self.rows.iter().copied().enumerate() {
            let indent = row.indent();
            if collapsed_indent.is_some_and(|parent_indent| indent > parent_indent) {
                continue;
            }
            collapsed_indent = None;
            self.visible_rows.push(position);
            if let BrowserRow::Folder {
                collapsed: true, ..
            } = row
            {
                collapsed_indent = Some(indent);
            }
        }
    }

    pub(super) fn toggle_selected_folder(&mut self) -> Option<bool> {
        let position = self.normal_position(self.selection)?;
        let Some(BrowserRow::Folder { collapsed, .. }) = self.rows.get_mut(position) else {
            return None;
        };
        *collapsed = !*collapsed;
        let collapsed = *collapsed;
        self.rebuild_visible_browser_rows();
        Some(collapsed)
    }
}

fn entry_matches(
    entry: &TrackEntry,
    tags: Option<&crate::model::TrackTags>,
    needle: &[u8],
) -> bool {
    contains_ignoring_ascii_case(entry.search.filename.as_bytes(), needle)
        || contains_ignoring_ascii_case(entry.search.relative_path.as_bytes(), needle)
        || tags.is_some_and(|tags| {
            [&tags.artist, &tags.album_artist, &tags.album, &tags.title]
                .into_iter()
                .flatten()
                .any(|field| contains_ignoring_ascii_case(field.as_bytes(), needle))
        })
}

/// Fields are short (at most a few KiB), so a window scan is fast enough per keystroke.
pub(super) fn contains_ignoring_ascii_case(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window.eq_ignore_ascii_case(needle))
}

fn build_browser_rows(index: &ScanIndex) -> Vec<BrowserRow> {
    let mut rows = vec![BrowserRow::LibraryRoot];
    let mut previous = None;
    let mut entry_cursor = 0;
    while let Some(entry) = index.entries.get(entry_cursor) {
        if matches!(
            &entry.source,
            TrackEntrySource::LibraryFile | TrackEntrySource::LibrarySymlink
        ) {
            append_tree_entry(index, &mut rows, entry_cursor, 1, 1, &mut previous);
            entry_cursor += 1;
        } else {
            break;
        }
    }

    rows.push(BrowserRow::PlaylistsRoot);
    for (playlist_index, playlist) in index.playlists.iter().enumerate() {
        rows.push(BrowserRow::Playlist { playlist_index });
        let mut previous = None;
        while index
            .entries
            .get(entry_cursor)
            .is_some_and(|entry| entry_playlist(entry) == Some(playlist.id))
        {
            append_tree_entry(index, &mut rows, entry_cursor, 2, 2, &mut previous);
            entry_cursor += 1;
        }
    }
    rows
}

fn append_tree_entry(
    index: &ScanIndex,
    rows: &mut Vec<BrowserRow>,
    entry_index: usize,
    skipped_components: usize,
    base_indent: usize,
    previous: &mut Option<(usize, usize)>,
) {
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
        rows.push(BrowserRow::Folder {
            entry_index,
            component_index: skipped_components + depth - 1,
            indent: base_indent + depth - 1,
            collapsed: false,
        });
    }
    rows.push(BrowserRow::Track {
        entry_index,
        indent: base_indent + folder_count,
    });
    *previous = Some((entry_index, folder_count));
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
        TrackEntrySource::LibraryFile | TrackEntrySource::LibrarySymlink => None,
    }
}
