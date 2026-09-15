// SPDX-License-Identifier: Apache-2.0

//! Bounded browser rows, folder visibility, and incremental search state.

use std::ffi::OsStr;
use std::mem::size_of;
use std::path::{Component, Path};

use super::reserve_exact;
use crate::config::Config;
use crate::errors::{AppError, AppResult};
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
    prefix_table: Vec<usize>,
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
    pub(super) fn new(index: &ScanIndex, config: &Config, capacity: usize) -> AppResult<Self> {
        let mut rows = Vec::new();
        reserve_exact(&mut rows, capacity, "library browser")?;
        build_browser_rows(index, &mut rows, capacity)?;
        let mut visible_rows = Vec::new();
        reserve_exact(&mut visible_rows, capacity, "library browser visibility")?;
        visible_rows.extend(0..rows.len());
        let selection = visible_rows
            .iter()
            .position(|position| matches!(rows[*position], BrowserRow::Track { .. }))
            .unwrap_or(0);

        Ok(Self {
            rows,
            visible_rows,
            selection,
            search: allocate_search(config, selection)?,
        })
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
        for (entry_index, entry) in index.entries.iter().enumerate() {
            let tags = index.asset_for_entry(entry).map(|asset| &asset.tags);
            if entry_matches(entry, tags, needle, prefix_table) {
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

pub(super) fn browser_reservation(index: &ScanIndex) -> AppResult<(usize, usize)> {
    let capacity = index
        .counters
        .encountered_entries
        .checked_add(index.playlists.len())
        .and_then(|value| value.checked_add(2))
        .ok_or_else(|| AppError::Resource("library browser reservation overflow".into()))?;
    let row_bytes = capacity
        .checked_mul(size_of::<BrowserRow>())
        .ok_or_else(|| AppError::Resource("library browser byte reservation overflow".into()))?;
    let visible_bytes = capacity.checked_mul(size_of::<usize>()).ok_or_else(|| {
        AppError::Resource("library browser visibility reservation overflow".into())
    })?;
    let reserved_bytes = row_bytes.checked_add(visible_bytes).ok_or_else(|| {
        AppError::Resource("combined library browser reservation overflow".into())
    })?;
    Ok((capacity, reserved_bytes))
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

pub(super) fn search_reservation_bytes(config: &Config) -> AppResult<usize> {
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
pub(super) struct SearchMatch {
    pub(super) found: bool,
    #[cfg(test)]
    pub(super) comparisons: usize,
}

pub(super) fn rebuild_ascii_case_prefix(needle: &[u8], prefix_table: &mut Vec<usize>) {
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

pub(super) fn contains_ascii_case_insensitive(
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
            TrackEntrySource::LibraryFile | TrackEntrySource::LibrarySymlink
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
                collapsed: false,
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
        TrackEntrySource::LibraryFile | TrackEntrySource::LibrarySymlink => None,
    }
}
