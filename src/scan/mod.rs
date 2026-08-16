// SPDX-License-Identifier: Apache-2.0

//! One-pass descriptor-rooted audio discovery.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use rustix::fd::{AsFd, OwnedFd};
use rustix::fs::{AtFlags, Dir, FileType, Mode, OFlags, ResolveFlags};

use crate::config::{Config, ScanConfig, scan_reservation_bytes};
use crate::display::terminal_safe;
use crate::errors::{AppError, AppResult};
use crate::metadata;
use crate::model::{
    FileIdentity, MediaAsset, Playlist, PlaylistId, ScanCounters, ScanIndex, ScanWarning,
    ScanWarningCode, SearchFields, TrackEntry, TrackEntryId, TrackEntrySource, TrackTags,
    stable_id,
};

const METADATA_TIME_BUDGET: Duration = Duration::from_mins(1);

/// Testable behavior when secure file-symlink opening is unavailable.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SecureOpenMode {
    /// Use Linux `openat2` with no magic-link fallback.
    #[default]
    Auto,
    /// Return the production unsupported result without attempting a weaker open.
    Unsupported,
}

/// Scanner limits and deterministic verification seams.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScanOptions {
    /// Secure-open availability seam.
    pub secure_open: SecureOpenMode,
    /// Bytes already retained by the currently active index.
    pub active_index_bytes: usize,
    /// Deterministic mount-identity seam used only by verification.
    pub root_device_override: Option<u64>,
    /// Cumulative wall time allowed for isolated metadata helpers.
    pub metadata_time_budget: Duration,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            secure_open: SecureOpenMode::Auto,
            active_index_bytes: 0,
            root_device_override: None,
            metadata_time_budget: METADATA_TIME_BUDGET,
        }
    }
}

/// Observer for proving descriptor pinning without timing races.
pub trait ScanObserver {
    /// Called after a directory descriptor is pinned and before it is enumerated.
    fn directory_opened(&mut self, _relative: &Path) {}

    /// Called immediately before the secure adapter opens a file symlink.
    fn before_symlink_open(&mut self, _relative: &Path) {}
}

#[derive(Default)]
struct NoopObserver;
impl ScanObserver for NoopObserver {}

/// Coarse path class used by the scanner and its path fuzz target.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PathClass {
    LibraryAudio,
    PlaylistAudio { playlist: Vec<u8> },
    Other,
}

/// Returns whether an extension is a discovery hint supported by the scanner.
#[must_use]
pub fn is_supported_audio_extension(path: &Path) -> bool {
    path.extension().is_some_and(|extension| {
        let bytes = extension.as_bytes();
        [b"mp3".as_slice(), b"flac", b"wav", b"ogg", b"oga"]
            .iter()
            .any(|expected| bytes.eq_ignore_ascii_case(expected))
    })
}

/// Classifies an audio-root-relative path without touching the filesystem.
#[must_use]
pub fn classify_relative_path(path: &Path) -> PathClass {
    let components: Vec<&OsStr> = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value),
            _ => None,
        })
        .collect();
    if !is_supported_audio_extension(path) {
        return PathClass::Other;
    }
    match components.as_slice() {
        [first, ..] if first.as_bytes() == b"library" => PathClass::LibraryAudio,
        [first, playlist, _, ..] if first.as_bytes() == b"playlists" => PathClass::PlaylistAudio {
            playlist: playlist.as_bytes().to_vec(),
        },
        _ => PathClass::Other,
    }
}

/// Fuzzes raw Linux path bytes through the non-I/O classifier.
pub fn fuzz_classify(bytes: &[u8]) {
    let path = PathBuf::from(std::ffi::OsString::from_vec(bytes.to_vec()));
    let _ = classify_relative_path(&path);
}

/// Checks that an active and replacement index can coexist in the process ledger.
///
/// # Errors
///
/// Returns an error on arithmetic overflow or insufficient process budget.
pub fn reserve_replacement(config: &Config, active_index_bytes: usize) -> AppResult<()> {
    if active_index_bytes > config.scan.max_index_bytes {
        return Err(AppError::InvalidConfig(
            "active index exceeds scan.max_index_bytes".into(),
        ));
    }
    let required = scan_reservation_bytes(active_index_bytes, config.scan.max_index_bytes)?;
    if required > config.runtime.process_memory_budget_bytes {
        return Err(AppError::InvalidConfig(format!(
            "active index, replacement index, and scan/parser scratch require {required} bytes, above process_memory_budget_bytes"
        )));
    }
    Ok(())
}

/// Scans one selected canonical root with production secure-open behavior.
///
/// # Errors
///
/// Returns an error when the root or descriptor traversal cannot be established.
pub fn scan(root: &Path, config: &Config, generation: u64) -> AppResult<ScanIndex> {
    scan_with_options(root, config, generation, ScanOptions::default())
}

/// Scans relative to an already pinned session root descriptor.
///
/// # Errors
///
/// Returns an error when reservation or descriptor traversal cannot be established.
pub fn scan_from<Fd: AsFd>(
    root_file: Fd,
    root: &Path,
    config: &Config,
    generation: u64,
) -> AppResult<ScanIndex> {
    let mut reader = metadata::HelperMetadataReader;
    let mut observer = NoopObserver;
    scan_with_components_from(
        root_file,
        root,
        config,
        generation,
        ScanOptions::default(),
        &mut reader,
        &mut observer,
    )
}

/// Scans one root with explicit instrumentation seams.
///
/// # Errors
///
/// Returns an error when reservation, root validation, or traversal fails.
pub fn scan_with_options(
    root: &Path,
    config: &Config,
    generation: u64,
    options: ScanOptions,
) -> AppResult<ScanIndex> {
    let mut reader = metadata::HelperMetadataReader;
    let mut observer = NoopObserver;
    scan_with_components(
        root,
        config,
        generation,
        options,
        &mut reader,
        &mut observer,
    )
}

/// Scans with explicit parser and observation seams for deterministic verification.
///
/// # Errors
///
/// Returns an error when reservation, root validation, or traversal fails.
pub fn scan_with_components(
    root: &Path,
    config: &Config,
    generation: u64,
    options: ScanOptions,
    reader: &mut dyn metadata::MetadataReader,
    observer: &mut dyn ScanObserver,
) -> AppResult<ScanIndex> {
    let root_file = rustix::fs::open(
        root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(|error| AppError::io("open scan root", root, error.into()))?;
    scan_with_components_from(
        &root_file, root, config, generation, options, reader, observer,
    )
}

fn scan_with_components_from<Fd: AsFd>(
    root_file: Fd,
    root: &Path,
    config: &Config,
    generation: u64,
    options: ScanOptions,
    reader: &mut dyn metadata::MetadataReader,
    observer: &mut dyn ScanObserver,
) -> AppResult<ScanIndex> {
    reserve_replacement(config, options.active_index_bytes)?;
    let audio_fd = open_direct_directory(root_file, OsStr::new("audio"), false)
        .map_err(|error| AppError::io("open audio root", root.join("audio"), error.into()))?;
    let root_device = options.root_device_override.unwrap_or(
        rustix::fs::fstat(&audio_fd)
            .map_err(|error| AppError::io("inspect audio root", root.join("audio"), error.into()))?
            .st_dev,
    );
    let mut scanner = Scanner::new(
        root,
        config,
        generation,
        options,
        root_device,
        reader,
        observer,
    );
    scanner.walk_directory(audio_fd, PathBuf::new(), 0)?;
    Ok(scanner.finish())
}

struct PendingEntry {
    id: TrackEntryId,
    asset_index: usize,
    display_path: PathBuf,
    source: TrackEntrySource,
}

struct Scanner<'a> {
    root: &'a Path,
    limits: &'a ScanConfig,
    generation: u64,
    options: ScanOptions,
    root_device: u64,
    max_open_files: usize,
    open_files: usize,
    reader: &'a mut dyn metadata::MetadataReader,
    observer: &'a mut dyn ScanObserver,
    complete: bool,
    stopped: bool,
    assets: Vec<MediaAsset>,
    asset_indices: BTreeMap<(u64, u64), usize>,
    entries: Vec<PendingEntry>,
    playlists: BTreeMap<Vec<u8>, Playlist>,
    warnings: Vec<ScanWarning>,
    counters: ScanCounters,
    metadata_elapsed: Duration,
    metadata_budget_reported: bool,
}

impl<'a> Scanner<'a> {
    fn new(
        root: &'a Path,
        config: &'a Config,
        generation: u64,
        options: ScanOptions,
        root_device: u64,
        reader: &'a mut dyn metadata::MetadataReader,
        observer: &'a mut dyn ScanObserver,
    ) -> Self {
        Self {
            root,
            limits: &config.scan,
            generation,
            options,
            root_device,
            max_open_files: config.runtime.max_open_files,
            open_files: 1,
            reader,
            observer,
            complete: true,
            stopped: false,
            assets: Vec::new(),
            asset_indices: BTreeMap::new(),
            entries: Vec::new(),
            playlists: BTreeMap::new(),
            warnings: Vec::new(),
            counters: ScanCounters {
                open_files_high_water: 1,
                ..ScanCounters::default()
            },
            metadata_elapsed: Duration::ZERO,
            metadata_budget_reported: false,
        }
    }

    // Ownership keeps the pinned directory descriptor alive for the full enumeration.
    #[allow(clippy::needless_pass_by_value)]
    fn walk_directory(&mut self, fd: OwnedFd, relative: PathBuf, depth: usize) -> AppResult<()> {
        if self.stopped {
            return Ok(());
        }
        if !self.acquire_open_slots(1, &relative) {
            return Ok(());
        }
        self.observer.directory_opened(&relative);
        let mut directory = match Dir::read_from(&fd) {
            Ok(directory) => directory,
            Err(error) => {
                self.release_open_slots(1);
                return Err(AppError::io(
                    "enumerate directory",
                    self.root.join("audio").join(&relative),
                    error.into(),
                ));
            }
        };
        self.counters.directory_enumerations += 1;
        let mut names = Vec::new();
        for result in &mut directory {
            if self.stopped {
                break;
            }
            let entry = match result {
                Ok(entry) => entry,
                Err(error) => {
                    self.warn(
                        ScanWarningCode::EntryRead,
                        &relative,
                        format!("cannot read directory entry: {error}"),
                    );
                    continue;
                }
            };
            let name = OsString::from_vec(entry.file_name().to_bytes().to_vec());
            if name.as_bytes() == b"." || name.as_bytes() == b".." {
                continue;
            }
            let child = relative.join(&name);
            if !self.encounter(&child) {
                if self.stopped {
                    break;
                }
                continue;
            }
            names.push(name);
        }
        names.sort_by(|left, right| natural_bytes_cmp(left.as_bytes(), right.as_bytes()));
        for name in names {
            if self.stopped {
                break;
            }
            let child = relative.join(&name);
            let stat = match rustix::fs::statat(&fd, &name, AtFlags::SYMLINK_NOFOLLOW) {
                Ok(stat) => stat,
                Err(error) => {
                    self.warn(
                        ScanWarningCode::EntryRead,
                        &child,
                        format!("entry changed before inspection: {error}"),
                    );
                    continue;
                }
            };
            match FileType::from_raw_mode(stat.st_mode) {
                FileType::Directory => self.directory_entry(&fd, &name, child, depth + 1)?,
                FileType::RegularFile => self.regular_entry(&fd, &name, child, false)?,
                FileType::Symlink => self.symlink_entry(&fd, &name, child)?,
                _ => self.warn(
                    ScanWarningCode::NonRegularTarget,
                    &child,
                    "special filesystem entry ignored",
                ),
            }
        }
        self.release_open_slots(1);
        Ok(())
    }

    fn directory_entry<Fd: AsFd>(
        &mut self,
        parent: Fd,
        name: &OsStr,
        child: PathBuf,
        depth: usize,
    ) -> AppResult<()> {
        self.counters.directories += 1;
        if depth > self.limits.max_depth {
            self.limit(&child, "scan.max_depth");
            return Ok(());
        }
        if self.limits.ignore_hidden_audio && is_hidden(name) {
            return Ok(());
        }
        if child.components().count() == 1 && !matches!(name.as_bytes(), b"library" | b"playlists")
        {
            return Ok(());
        }
        if playlist_key(&child).is_some_and(|key| {
            child.components().count() == 2 && !self.ensure_playlist(key, &child)
        }) {
            return Ok(());
        }
        if !self.acquire_open_slots(1, &child) {
            return Ok(());
        }
        self.counters.mount_decisions += 1;
        let directory = match open_direct_directory(parent, name, !self.limits.cross_mounts) {
            Ok(directory) => directory,
            Err(error) => {
                self.release_open_slots(1);
                let code = if !self.limits.cross_mounts && error == rustix::io::Errno::XDEV {
                    ScanWarningCode::CrossMountSkipped
                } else {
                    ScanWarningCode::EntryRead
                };
                self.warn(
                    code,
                    &child,
                    format!("cannot securely open directory: {error}"),
                );
                return Ok(());
            }
        };
        let result = (|| {
            let device = rustix::fs::fstat(&directory)
                .map_err(|error| {
                    AppError::io(
                        "inspect opened directory",
                        self.root.join("audio").join(&child),
                        error.into(),
                    )
                })?
                .st_dev;
            if !self.limits.cross_mounts && device != self.root_device {
                self.warn(
                    ScanWarningCode::CrossMountSkipped,
                    &child,
                    "cross-mount directory skipped",
                );
                return Ok(());
            }
            self.walk_directory(directory, child, depth)
        })();
        self.release_open_slots(1);
        result
    }

    fn regular_entry<Fd: AsFd>(
        &mut self,
        parent: Fd,
        name: &OsStr,
        child: PathBuf,
        symlink: bool,
    ) -> AppResult<()> {
        match classify_relative_path(&child) {
            PathClass::LibraryAudio | PathClass::PlaylistAudio { .. } => {
                if self.limits.ignore_hidden_audio && is_hidden(name) {
                    return Ok(());
                }
                if !self.acquire_open_slots(1, &child) {
                    return Ok(());
                }
                let opened = rustix::fs::openat(
                    parent,
                    name,
                    OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                );
                let result = match opened {
                    Ok(fd) => self.accept_media(File::from(fd), child, symlink),
                    Err(error) => {
                        self.warn(
                            ScanWarningCode::EntryRead,
                            &child,
                            format!("media changed before secure open: {error}"),
                        );
                        Ok(())
                    }
                };
                self.release_open_slots(1);
                result?;
            }
            PathClass::Other => {
                if is_media_hierarchy(&child)
                    && !child
                        .file_name()
                        .is_some_and(|value| value.as_bytes().eq_ignore_ascii_case(b"README.txt"))
                {
                    self.warn(
                        ScanWarningCode::UnsupportedExtension,
                        &child,
                        "file extension is not a supported audio discovery hint",
                    );
                }
            }
        }
        Ok(())
    }

    fn symlink_entry<Fd: AsFd>(
        &mut self,
        parent: Fd,
        name: &OsStr,
        child: PathBuf,
    ) -> AppResult<()> {
        if self.limits.ignore_hidden_audio
            && is_hidden(name)
            && matches!(
                classify_relative_path(&child),
                PathClass::LibraryAudio | PathClass::PlaylistAudio { .. }
            )
        {
            return Ok(());
        }
        if self.counters.symlink_resolutions >= self.limits.max_symlink_resolutions {
            self.limit(&child, "scan.max_symlink_resolutions");
            return Ok(());
        }
        self.counters.symlink_resolutions += 1;
        if !self.limits.follow_file_symlinks {
            self.warn(
                ScanWarningCode::UnsupportedSecureOpen,
                &child,
                "file symlink following is disabled",
            );
            return Ok(());
        }
        if self.options.secure_open == SecureOpenMode::Unsupported {
            self.warn(
                ScanWarningCode::UnsupportedSecureOpen,
                &child,
                "secure file-symlink open is unavailable; no path fallback used",
            );
            return Ok(());
        }
        self.observer.before_symlink_open(&child);
        if !self.acquire_open_slots(1, &child) {
            return Ok(());
        }
        let fd = match rustix::fs::openat2(
            parent,
            name,
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::NO_MAGICLINKS,
        ) {
            Ok(fd) => fd,
            Err(error) => {
                self.release_open_slots(1);
                let code = if error == rustix::io::Errno::NOENT {
                    ScanWarningCode::BrokenSymlink
                } else {
                    ScanWarningCode::UnsupportedSecureOpen
                };
                self.warn(
                    code,
                    &child,
                    format!("cannot securely open file symlink: {error}"),
                );
                return Ok(());
            }
        };
        let result = (|| {
            let stat = rustix::fs::fstat(&fd).map_err(|error| {
                AppError::io(
                    "inspect symlink target",
                    self.root.join("audio").join(&child),
                    error.into(),
                )
            })?;
            let kind = FileType::from_raw_mode(stat.st_mode);
            if kind.is_dir() {
                self.warn(
                    ScanWarningCode::IgnoredDirectorySymlink,
                    &child,
                    "directory symlink ignored",
                );
                return Ok(());
            }
            if !kind.is_file() {
                self.warn(
                    ScanWarningCode::NonRegularTarget,
                    &child,
                    "file symlink target is not regular",
                );
                return Ok(());
            }
            if !matches!(
                classify_relative_path(&child),
                PathClass::LibraryAudio | PathClass::PlaylistAudio { .. }
            ) {
                if is_media_hierarchy(&child) {
                    self.warn(
                        ScanWarningCode::UnsupportedExtension,
                        &child,
                        "file symlink extension is not a supported audio discovery hint",
                    );
                }
                return Ok(());
            }
            self.counters.mount_decisions += 1;
            self.accept_media(File::from(fd), child, true)
        })();
        self.release_open_slots(1);
        result
    }

    // Ownership prevents the verified media descriptor from escaping this admission step.
    #[allow(clippy::needless_pass_by_value, clippy::too_many_lines)]
    fn accept_media(&mut self, file: File, child: PathBuf, symlink: bool) -> AppResult<()> {
        if self.counters.media_files >= self.limits.max_files {
            self.limit(&child, "scan.max_files");
            return Ok(());
        }
        let playlist = playlist_key(&child).map(ToOwned::to_owned);
        if let Some(key) = playlist.as_deref() {
            if !self.ensure_playlist(key, playlist_path(&child).as_path()) {
                return Ok(());
            }
            if self
                .playlists
                .get(key)
                .is_some_and(|value| value.entries.len() >= self.limits.max_entries_per_playlist)
            {
                self.limit(&child, "scan.max_entries_per_playlist");
                return Ok(());
            }
        }
        self.counters.media_files += 1;
        let stat = rustix::fs::fstat(&file).map_err(|error| {
            AppError::io(
                "inspect verified media",
                self.root.join("audio").join(&child),
                error.into(),
            )
        })?;
        if !FileType::from_raw_mode(stat.st_mode).is_file() {
            self.warn(
                ScanWarningCode::NonRegularTarget,
                &child,
                "verified media descriptor is not regular",
            );
            return Ok(());
        }
        let size = u64::try_from(stat.st_size).map_err(|_| {
            AppError::Unsupported("regular media descriptor reported a negative size".into())
        })?;
        let identity = FileIdentity {
            device: stat.st_dev,
            inode: stat.st_ino,
            size,
            modified_seconds: stat.st_mtime,
            modified_nanoseconds: stat.st_mtime_nsec,
        };
        let identity_key = (identity.device, identity.inode);
        let asset_index = if let Some(asset_index) = self.asset_indices.get(&identity_key).copied()
        {
            asset_index
        } else {
            let tags = self.read_tags(&file, &child);
            self.account_index(retained_asset_bytes(tag_bytes(&tags)), &child);
            if self.stopped {
                return Ok(());
            }
            self.counters.metadata_bytes = self
                .counters
                .metadata_bytes
                .saturating_add(tag_bytes(&tags));
            let asset_index = self.assets.len();
            self.asset_indices.insert(identity_key, asset_index);
            self.assets.push(MediaAsset {
                tags,
                file_identity: identity,
            });
            asset_index
        };
        let child_bytes = child.as_os_str().as_bytes();
        let playlist_id = playlist.as_deref().map(|key| PlaylistId(stable_id(&[key])));
        let source = match (playlist_id, symlink) {
            (Some(playlist), true) => TrackEntrySource::PlaylistSymlink { playlist },
            (Some(playlist), false) => TrackEntrySource::PlaylistCopy { playlist },
            (None, true) => TrackEntrySource::LibrarySymlink,
            (None, false) => TrackEntrySource::LibraryFile,
        };
        let context = playlist.as_deref().unwrap_or(b"library");
        let id = TrackEntryId(stable_id(&[context, child_bytes]));
        self.account_index(retained_entry_bytes(child_bytes.len()), &child);
        if self.stopped {
            return Ok(());
        }
        self.entries.push(PendingEntry {
            id,
            asset_index,
            display_path: child.clone(),
            source,
        });
        if let Some(key) = playlist
            && let Some(playlist) = self.playlists.get_mut(&key)
        {
            playlist.entries.push(id);
        }
        Ok(())
    }

    fn read_tags(&mut self, file: &File, path: &Path) -> TrackTags {
        if self.metadata_elapsed >= self.options.metadata_time_budget {
            if !self.metadata_budget_reported {
                self.metadata_budget_reported = true;
                self.complete = false;
                self.warn(
                    ScanWarningCode::LimitReached,
                    path,
                    "metadata helper time budget reached; remaining tracks use filenames only",
                );
            }
            return TrackTags::default();
        }
        if self.counters.parser_attempts >= self.limits.max_parser_attempts {
            self.limit(path, "scan.max_parser_attempts");
            return TrackTags::default();
        }
        self.counters.parser_attempts += 1;
        if !self.acquire_open_slots(2, path) {
            return TrackTags::default();
        }
        let started = Instant::now();
        let parsed = self.reader.read(file);
        self.metadata_elapsed = self.metadata_elapsed.saturating_add(started.elapsed());
        self.release_open_slots(2);
        match parsed {
            Ok(tags) if fields_within_limit(&tags, self.limits.max_metadata_field_bytes) => {
                let bytes = tag_bytes(&tags);
                if self.counters.metadata_bytes.saturating_add(bytes)
                    > self.limits.max_total_metadata_bytes
                {
                    self.limit(path, "scan.max_total_metadata_bytes");
                    TrackTags::default()
                } else {
                    tags
                }
            }
            Ok(_) => {
                self.warn(
                    ScanWarningCode::MetadataRead,
                    path,
                    "metadata field exceeds scan.max_metadata_field_bytes",
                );
                TrackTags::default()
            }
            Err(error) => {
                self.warn(ScanWarningCode::MetadataRead, path, error);
                TrackTags::default()
            }
        }
    }

    fn ensure_playlist(&mut self, key: &[u8], path: &Path) -> bool {
        if self.playlists.contains_key(key) {
            return true;
        }
        if self.playlists.len() >= self.limits.max_playlists {
            self.limit(path, "scan.max_playlists");
            return false;
        }
        let id = PlaylistId(stable_id(&[key]));
        let name = terminal_safe(key, self.limits.max_path_bytes);
        self.account_index(retained_playlist_bytes(key.len(), name.len(), path), path);
        if self.stopped {
            return false;
        }
        self.playlists.insert(
            key.to_vec(),
            Playlist {
                id,
                name,
                path: path.to_path_buf(),
                entries: Vec::new(),
            },
        );
        true
    }

    fn encounter(&mut self, path: &Path) -> bool {
        if self.counters.encountered_entries >= self.limits.max_entries {
            self.limit(path, "scan.max_entries");
            return false;
        }
        let bytes = path_bytes(path);
        if bytes > self.limits.max_path_bytes {
            self.complete = false;
            self.warn(
                ScanWarningCode::LimitReached,
                path,
                "path exceeds scan.max_path_bytes",
            );
            return false;
        }
        if self.counters.path_bytes.saturating_add(bytes) > self.limits.max_total_path_bytes {
            self.limit(path, "scan.max_total_path_bytes");
            return false;
        }
        self.counters.encountered_entries += 1;
        self.counters.path_bytes += bytes;
        true
    }

    fn acquire_open_slots(&mut self, count: usize, path: &Path) -> bool {
        let Some(next) = self.open_files.checked_add(count) else {
            self.limit(path, "runtime.max_open_files");
            return false;
        };
        if next > self.max_open_files {
            self.limit(path, "runtime.max_open_files");
            return false;
        }
        self.open_files = next;
        self.counters.open_files_high_water = self.counters.open_files_high_water.max(next);
        true
    }

    fn release_open_slots(&mut self, count: usize) {
        if let Some(remaining) = self.open_files.checked_sub(count) {
            self.open_files = remaining;
        } else {
            self.open_files = 0;
            self.complete = false;
        }
    }

    fn account_index(&mut self, bytes: usize, path: &Path) {
        if self.counters.index_bytes.saturating_add(bytes) > self.limits.max_index_bytes {
            self.limit(path, "scan.max_index_bytes");
        } else {
            self.counters.index_bytes += bytes;
        }
    }

    fn limit(&mut self, path: &Path, name: &str) {
        self.complete = false;
        self.warn(
            ScanWarningCode::LimitReached,
            path,
            format!("{name} limit reached"),
        );
        self.stopped = true;
    }

    fn warn(&mut self, code: ScanWarningCode, path: &Path, message: impl Into<String>) {
        let message = terminal_safe(
            message.into().as_bytes(),
            self.limits.max_metadata_field_bytes,
        );
        let bytes = retained_warning_bytes(path, message.len());
        if self.counters.warning_bytes.saturating_add(bytes) > self.limits.max_warning_bytes {
            self.complete = false;
            return;
        }
        if self.counters.index_bytes.saturating_add(bytes) > self.limits.max_index_bytes {
            self.complete = false;
            self.stopped = true;
            return;
        }
        self.counters.warning_bytes += bytes;
        self.counters.index_bytes += bytes;
        self.warnings.push(ScanWarning {
            code,
            path: path.to_path_buf(),
            message,
        });
    }

    fn finish(self) -> ScanIndex {
        let mut contextual = Vec::with_capacity(self.entries.len());
        for pending in self.entries {
            let filename = pending
                .display_path
                .file_stem()
                .map_or_else(String::new, |value| {
                    terminal_safe(value.as_bytes(), self.limits.max_path_bytes)
                });
            let relative_path = terminal_safe(
                pending.display_path.as_os_str().as_bytes(),
                self.limits.max_path_bytes,
            );
            contextual.push(TrackEntry {
                id: pending.id,
                asset_index: pending.asset_index,
                display_path: pending.display_path,
                source: pending.source,
                search: SearchFields {
                    filename,
                    relative_path,
                },
                scan_generation: self.generation,
            });
        }
        contextual.sort_by(|left, right| natural_path_cmp(&left.display_path, &right.display_path));
        let ranks: BTreeMap<_, _> = contextual
            .iter()
            .enumerate()
            .map(|(rank, entry)| (entry.id, rank))
            .collect();
        let mut playlists: Vec<_> = self.playlists.into_values().collect();
        for playlist in &mut playlists {
            playlist
                .entries
                .sort_by_key(|entry| ranks.get(entry).copied().unwrap_or(usize::MAX));
        }
        playlists.sort_by(|left, right| natural_path_cmp(&left.path, &right.path));
        ScanIndex {
            generation: self.generation,
            complete: self.complete,
            assets: self.assets,
            entries: contextual,
            playlists,
            warnings: self.warnings,
            counters: self.counters,
        }
    }
}

fn open_direct_directory<Fd: AsFd>(
    parent: Fd,
    name: &OsStr,
    no_xdev: bool,
) -> rustix::io::Result<OwnedFd> {
    let mut resolve =
        ResolveFlags::BENEATH | ResolveFlags::NO_MAGICLINKS | ResolveFlags::NO_SYMLINKS;
    if no_xdev {
        resolve |= ResolveFlags::NO_XDEV;
    }
    match rustix::fs::openat2(
        &parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        resolve,
    ) {
        Ok(fd) => Ok(fd),
        Err(rustix::io::Errno::NOSYS | rustix::io::Errno::INVAL) => rustix::fs::openat(
            parent,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ),
        Err(error) => Err(error),
    }
}

fn playlist_key(path: &Path) -> Option<&[u8]> {
    let mut components = path.components();
    if components.next()?.as_os_str().as_bytes() != b"playlists" {
        return None;
    }
    Some(components.next()?.as_os_str().as_bytes())
}

fn is_media_hierarchy(path: &Path) -> bool {
    path.components().next().is_some_and(|component| {
        matches!(component, Component::Normal(value) if matches!(value.as_bytes(), b"library" | b"playlists"))
    })
}

fn playlist_path(path: &Path) -> PathBuf {
    let mut components = path.components();
    let mut result = PathBuf::new();
    if let Some(component) = components.next() {
        result.push(component);
    }
    if let Some(component) = components.next() {
        result.push(component);
    }
    result
}

fn path_bytes(path: &Path) -> usize {
    path.as_os_str().as_bytes().len()
}
fn tag_bytes(tags: &TrackTags) -> usize {
    [&tags.artist, &tags.album_artist, &tags.album, &tags.title]
        .into_iter()
        .flatten()
        .map(String::capacity)
        .sum()
}

// These charges intentionally exceed the retained model payload. They cover
// both build-time and final containers, lookup-tree nodes, cloned path
// buffers, vector slack, and allocator bookkeeping under one index budget.
fn retained_asset_bytes(metadata_bytes: usize) -> usize {
    std::mem::size_of::<MediaAsset>()
        .saturating_add(std::mem::size_of::<((u64, u64), usize)>())
        .saturating_add(256)
        .saturating_add(metadata_bytes.saturating_mul(3))
}

fn retained_entry_bytes(path_bytes: usize) -> usize {
    std::mem::size_of::<PendingEntry>()
        .saturating_add(std::mem::size_of::<TrackEntry>())
        .saturating_add(256)
        .saturating_add(path_bytes.saturating_mul(12))
}

fn retained_playlist_bytes(key_bytes: usize, name_bytes: usize, path: &Path) -> usize {
    std::mem::size_of::<(Vec<u8>, Playlist)>()
        .saturating_add(256)
        .saturating_add(key_bytes.saturating_mul(2))
        .saturating_add(name_bytes)
        .saturating_add(path_bytes(path))
}

fn retained_warning_bytes(path: &Path, message_bytes: usize) -> usize {
    std::mem::size_of::<ScanWarning>()
        .saturating_add(128)
        .saturating_add(path_bytes(path).saturating_mul(2))
        .saturating_add(message_bytes)
}
fn fields_within_limit(tags: &TrackTags, limit: usize) -> bool {
    [&tags.artist, &tags.album_artist, &tags.album, &tags.title]
        .into_iter()
        .flatten()
        .all(|value| value.len() <= limit)
}
fn is_hidden(name: &OsStr) -> bool {
    name.as_bytes().first() == Some(&b'.')
}
fn natural_path_cmp(left: &Path, right: &Path) -> std::cmp::Ordering {
    let mut left_components = left.components();
    let mut right_components = right.components();
    let component_order = loop {
        match (left_components.next(), right_components.next()) {
            (Some(left), Some(right)) => {
                let order =
                    natural_bytes_cmp(left.as_os_str().as_bytes(), right.as_os_str().as_bytes());
                if order != std::cmp::Ordering::Equal {
                    break order;
                }
            }
            (None, Some(_)) => break std::cmp::Ordering::Less,
            (Some(_), None) => break std::cmp::Ordering::Greater,
            (None, None) => break std::cmp::Ordering::Equal,
        }
    };
    component_order.then_with(|| {
        left.as_os_str()
            .as_bytes()
            .cmp(right.as_os_str().as_bytes())
    })
}

fn natural_bytes_cmp(mut left: &[u8], mut right: &[u8]) -> std::cmp::Ordering {
    while !left.is_empty() && !right.is_empty() {
        let left_digit = left[0].is_ascii_digit();
        let right_digit = right[0].is_ascii_digit();
        let left_end = left
            .iter()
            .position(|byte| byte.is_ascii_digit() != left_digit)
            .unwrap_or(left.len());
        let right_end = right
            .iter()
            .position(|byte| byte.is_ascii_digit() != right_digit)
            .unwrap_or(right.len());
        let left_run = &left[..left_end];
        let right_run = &right[..right_end];
        let order = if left_digit && right_digit {
            let left_significant = left_run
                .iter()
                .position(|byte| *byte != b'0')
                .map_or(&left_run[left_run.len()..], |index| &left_run[index..]);
            let right_significant = right_run
                .iter()
                .position(|byte| *byte != b'0')
                .map_or(&right_run[right_run.len()..], |index| &right_run[index..]);
            left_significant
                .len()
                .cmp(&right_significant.len())
                .then_with(|| left_significant.cmp(right_significant))
                .then_with(|| {
                    (left_run.len() - left_significant.len())
                        .cmp(&(right_run.len() - right_significant.len()))
                })
                .then_with(|| left_run.cmp(right_run))
        } else {
            left_run
                .iter()
                .map(u8::to_ascii_lowercase)
                .cmp(right_run.iter().map(u8::to_ascii_lowercase))
                .then_with(|| left_run.cmp(right_run))
        };
        if order != std::cmp::Ordering::Equal {
            return order;
        }
        left = &left[left_end..];
        right = &right[right_end..];
    }
    left.len().cmp(&right.len())
}

#[cfg(test)]
mod tests {
    use std::cmp::Ordering;
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    use std::path::PathBuf;

    use super::{natural_bytes_cmp, natural_path_cmp};

    #[test]
    fn natural_order_v1_fixed_vectors_compare_components_independently() {
        const ORDERED_PAIRS: &[(&[u8], &[u8])] = &[
            (b"", b"1"),
            (b"1", b"01"),
            (b"01", b"001"),
            (b"001", b"2"),
            (b"2", b"10"),
            (b"A", b"a"),
            (b"\xc3\xa9", b"\xc3\xaa"),
            (b"bad-\xfe", b"bad-\xff"),
            (b"library/a/x.mp3", b"library/a-/x.mp3"),
            (b"library/2/x.mp3", b"library/10/x.mp3"),
        ];

        for &(left, right) in ORDERED_PAIRS {
            let left = PathBuf::from(OsString::from_vec(left.to_vec()));
            let right = PathBuf::from(OsString::from_vec(right.to_vec()));
            assert_eq!(natural_path_cmp(&left, &right), Ordering::Less);
            assert_eq!(natural_path_cmp(&right, &left), Ordering::Greater);
        }
    }

    #[test]
    fn natural_order_keeps_numeric_case_and_byte_tie_breakers() {
        const ORDERED_PAIRS: &[(&[u8], &[u8])] = &[
            (b"track2", b"track10"),
            (b"track2", b"track02"),
            (b"a", b"aa"),
            (b"ABC", b"abd"),
            (b"Abc", b"abc"),
            (b"bad-\xfe", b"bad-\xff"),
        ];

        for &(left, right) in ORDERED_PAIRS {
            assert_eq!(natural_bytes_cmp(left, right), Ordering::Less);
            assert_eq!(natural_bytes_cmp(right, left), Ordering::Greater);
        }
    }
}
