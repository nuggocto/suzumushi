// SPDX-License-Identifier: Apache-2.0

//! Root selection and the app-owned directory model.

use std::env;
use std::fs;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use crate::errors::{AppError, AppResult};

/// The source that selected a root.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RootSource {
    /// The command-line `--root` option.
    CommandLine,
    /// The `SUZUMUSHI_ROOT` environment variable.
    Environment,
    /// The existing `./suzumushi/audio` convention.
    WorkingDirectory,
}

/// A selected, canonical root and its source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectedRoot {
    /// Canonical filesystem path.
    pub path: PathBuf,
    /// Winning precedence source.
    pub source: RootSource,
}

/// All paths owned by one Suzumushi root.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RootPaths {
    /// Canonical root.
    pub root: PathBuf,
    /// Audio hierarchy.
    pub audio: PathBuf,
    /// Recursive library hierarchy.
    pub library: PathBuf,
    /// Folder playlists hierarchy.
    pub playlists: PathBuf,
    /// Reconstructible state.
    pub state: PathBuf,
    /// Artwork cache.
    pub artwork_cache: PathBuf,
    /// Private logs.
    pub logs: PathBuf,
    /// Durable backups.
    pub backups: PathBuf,
    /// Tag-edit backups.
    pub tag_backups: PathBuf,
    /// Root configuration.
    pub config: PathBuf,
}

impl RootPaths {
    /// Builds the fixed directory model below a canonical root.
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        let audio = root.join("audio");
        let state = root.join("state");
        let backups = root.join("backups");
        Self {
            library: audio.join("library"),
            playlists: audio.join("playlists"),
            artwork_cache: state.join("artwork"),
            logs: root.join("logs"),
            tag_backups: backups.join("tag-edits"),
            config: root.join("config.toml"),
            root,
            audio,
            state,
            backups,
        }
    }
}

/// Selects a root using the single product-wide precedence contract.
///
/// # Errors
///
/// Returns a clear missing-root or invalid-selected-root error.
pub fn discover_root(explicit: Option<&Path>, current_dir: &Path) -> AppResult<SelectedRoot> {
    discover_root_from(
        explicit,
        env::var_os("SUZUMUSHI_ROOT").as_deref().map(Path::new),
        current_dir,
    )
}

/// Selects a root with an injected environment source for deterministic tests.
///
/// # Errors
///
/// Returns a clear missing-root or invalid-selected-root error without fallback
/// from an invalid higher-priority source.
pub fn discover_root_from(
    explicit: Option<&Path>,
    environment: Option<&Path>,
    current_dir: &Path,
) -> AppResult<SelectedRoot> {
    if let Some(path) = explicit {
        return validate_selected(path, RootSource::CommandLine);
    }
    if let Some(path) = environment {
        return validate_selected(path, RootSource::Environment);
    }
    let conventional = current_dir.join("suzumushi");
    if conventional.join("audio").is_dir() {
        return validate_selected(&conventional, RootSource::WorkingDirectory);
    }
    Err(AppError::RootMissing)
}

fn validate_selected(path: &Path, source: RootSource) -> AppResult<SelectedRoot> {
    let metadata = fs::symlink_metadata(path).map_err(|error| AppError::InvalidRoot {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(AppError::InvalidRoot {
            path: path.to_path_buf(),
            reason: "the selected root must be a real directory, not a symlink".into(),
        });
    }
    let root_fd = rustix::fs::open(
        path,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::DIRECTORY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .map_err(|error| AppError::InvalidRoot {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })?;
    rustix::fs::openat(
        &root_fd,
        "audio",
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::DIRECTORY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .map_err(|error| AppError::InvalidRoot {
        path: path.to_path_buf(),
        reason: format!("audio must be a real readable directory: {error}"),
    })?;
    let canonical =
        fs::read_link(format!("/proc/self/fd/{}", root_fd.as_raw_fd())).map_err(|error| {
            AppError::InvalidRoot {
                path: path.to_path_buf(),
                reason: format!("cannot resolve the verified root descriptor: {error}"),
            }
        })?;
    Ok(SelectedRoot {
        path: canonical,
        source,
    })
}
