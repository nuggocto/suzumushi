// SPDX-License-Identifier: Apache-2.0

//! Root selection and the app-owned directory model.

use std::env;
use std::fs::{self, File};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd};
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

/// Stable filesystem identity of the selected root descriptor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RootIdentity {
    pub device: u64,
    pub inode: u64,
}

/// A selected root pinned for the lifetime of one command or terminal session.
#[derive(Debug)]
pub struct SelectedRoot {
    /// Canonical filesystem path.
    pub path: PathBuf,
    /// Winning precedence source.
    pub source: RootSource,
    /// Identity captured from the verified descriptor.
    pub identity: RootIdentity,
    descriptor: File,
}

impl SelectedRoot {
    /// Borrows the descriptor that all root-relative session work must use.
    #[must_use]
    pub fn descriptor(&self) -> BorrowedFd<'_> {
        self.descriptor.as_fd()
    }
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
    /// Private logs.
    pub logs: PathBuf,
    /// Root configuration.
    pub config: PathBuf,
}

impl RootPaths {
    /// Builds the fixed directory model below a canonical root.
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        let audio = root.join("audio");
        let state = root.join("state");
        Self {
            library: audio.join("library"),
            playlists: audio.join("playlists"),
            logs: root.join("logs"),
            config: root.join("config.toml"),
            root,
            audio,
            state,
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
    let audio_fd = rustix::fs::openat(
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
    drop(audio_fd);
    let stat = rustix::fs::fstat(&root_fd).map_err(|error| AppError::InvalidRoot {
        path: path.to_path_buf(),
        reason: format!("cannot inspect the verified root descriptor: {error}"),
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
        identity: RootIdentity {
            device: stat.st_dev,
            inode: stat.st_ino,
        },
        descriptor: File::from(root_fd),
    })
}
