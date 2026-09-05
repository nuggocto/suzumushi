// SPDX-License-Identifier: Apache-2.0

//! Scanner-owned media and contextual entry models.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Stable-within-context browser entry identifier.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct TrackEntryId(pub u64);

/// Stable-within-path playlist identifier.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PlaylistId(pub u64);

/// Linux identity captured from the verified media descriptor.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct FileIdentity {
    pub device: u64,
    pub inode: u64,
    pub size: u64,
    pub modified_seconds: i64,
    pub modified_nanoseconds: u64,
}

/// Lightweight searchable tag fields.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TrackTags {
    pub artist: Option<String>,
    pub album_artist: Option<String>,
    pub album: Option<String>,
    pub title: Option<String>,
}

/// One canonical media file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MediaAsset {
    pub tags: TrackTags,
    pub file_identity: FileIdentity,
}

/// The context through which a user encountered an asset.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TrackEntrySource {
    LibraryFile,
    LibrarySymlink,
    PlaylistCopy { playlist: PlaylistId },
    PlaylistSymlink { playlist: PlaylistId },
}

/// A contextual browser entry. Several entries may name one asset.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrackEntry {
    pub id: TrackEntryId,
    pub asset_index: usize,
    pub display_path: PathBuf,
    pub source: TrackEntrySource,
    pub search: SearchFields,
}

/// Searchable path fields owned by one contextual entry.
///
/// Metadata remains on [`MediaAsset`], so playlist entries can share it instead
/// of retaining another copy for every path that names the same file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchFields {
    pub filename: String,
    pub relative_path: String,
}

/// One filesystem playlist.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Playlist {
    pub id: PlaylistId,
    pub name: String,
    pub path: PathBuf,
    pub entry_count: usize,
}

/// Stable warning category emitted by a bounded scan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScanWarningCode {
    BrokenSymlink,
    IgnoredDirectorySymlink,
    UnsupportedSecureOpen,
    CrossMountSkipped,
    NonRegularTarget,
    UnsupportedExtension,
    MetadataRead,
    LimitReached,
    EntryRead,
}

/// A bounded diagnostic tied to a root-relative entry when available.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScanWarning {
    pub code: ScanWarningCode,
    pub path: PathBuf,
    pub message: String,
}

/// Observable counters proving scanner bounds and traversal behavior.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ScanCounters {
    pub directory_enumerations: usize,
    pub encountered_entries: usize,
    pub directories: usize,
    pub media_files: usize,
    pub symlink_resolutions: usize,
    pub parser_attempts: usize,
    pub mount_decisions: usize,
    pub path_bytes: usize,
    pub metadata_bytes: usize,
    pub warning_bytes: usize,
    pub index_bytes: usize,
    pub open_files_high_water: usize,
}

/// Complete or visibly partial library index.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScanIndex {
    pub generation: u64,
    pub complete: bool,
    pub assets: Vec<MediaAsset>,
    pub entries: Vec<TrackEntry>,
    pub playlists: Vec<Playlist>,
    pub warnings: Vec<ScanWarning>,
    pub counters: ScanCounters,
}

impl ScanIndex {
    /// Returns the canonical asset named by a contextual entry.
    #[must_use]
    pub fn asset_for_entry(&self, entry: &TrackEntry) -> Option<&MediaAsset> {
        self.assets.get(entry.asset_index)
    }
}

/// Deterministic app-owned FNV-1a identifier.
#[must_use]
pub fn stable_id(parts: &[&[u8]]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for part in parts {
        for byte in *part {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash ^= 0xff;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}
