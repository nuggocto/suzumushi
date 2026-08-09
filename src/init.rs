// SPDX-License-Identifier: Apache-2.0

//! Secure, idempotent root initialization.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::path::{Component, Path, PathBuf};

use rustix::fd::{AsFd, OwnedFd};
use rustix::fs::{Dir, Mode, OFlags};
use rustix::process::getuid;

use crate::config::DEFAULT_CONFIG;
use crate::errors::{AppError, AppResult};
use crate::paths::RootPaths;

const DEMO_README: &str = "Suzumushi demo playlist\n\nCopy audio files here, or add file symlinks to local audio.\nFolders are playlists; directory symlinks are ignored.\n";

/// Initializes one app-owned root without following its final component.
///
/// # Errors
///
/// Returns a typed refusal or I/O error if the destination, ownership, mode, or
/// entry type cannot satisfy the app-owned root contract.
pub fn initialize(path: &Path) -> AppResult<RootPaths> {
    let root_fd = open_or_create_root(path)?;
    verify_root_owner(&root_fd, path)?;
    let _root_lease = crate::locks::RootMutationLease::acquire_from(&root_fd, path)?;

    let audio = ensure_dir(&root_fd, "audio", 0o755, false, &path.join("audio"))?;
    ensure_dir(&audio, "library", 0o755, false, &path.join("audio/library"))?;
    let playlists = ensure_dir(
        &audio,
        "playlists",
        0o755,
        false,
        &path.join("audio/playlists"),
    )?;
    let demo = ensure_dir(
        &playlists,
        "demo",
        0o755,
        false,
        &path.join("audio/playlists/demo"),
    )?;
    let state = ensure_dir(&root_fd, "state", 0o700, true, &path.join("state"))?;
    ensure_dir(&state, "artwork", 0o700, true, &path.join("state/artwork"))?;
    ensure_dir(&root_fd, "logs", 0o700, true, &path.join("logs"))?;
    let backups = ensure_dir(&root_fd, "backups", 0o700, true, &path.join("backups"))?;
    ensure_dir(
        &backups,
        "tag-edits",
        0o700,
        true,
        &path.join("backups/tag-edits"),
    )?;

    ensure_file(
        &root_fd,
        "config.toml",
        DEFAULT_CONFIG.as_bytes(),
        0o600,
        true,
        &path.join("config.toml"),
    )?;
    ensure_file(
        &demo,
        "README.txt",
        DEMO_README.as_bytes(),
        0o644,
        false,
        &path.join("audio/playlists/demo/README.txt"),
    )?;
    let canonical = descriptor_path(&root_fd, path)?;
    crate::config::load_from(&root_fd, path)?;
    Ok(RootPaths::new(canonical))
}

fn open_or_create_root(path: &Path) -> AppResult<OwnedFd> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(AppError::InitRefused {
                    path: path.to_path_buf(),
                    reason: "destination must be a real directory, not a symlink".into(),
                });
            }
            let fd = rustix::fs::open(
                path,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|error| AppError::io("open root", path, error.into()))?;
            reject_unrelated_contents(&fd, path)?;
            Ok(fd)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => create_root(path),
        Err(error) => Err(AppError::io("inspect destination", path, error)),
    }
}

fn create_root(path: &Path) -> AppResult<OwnedFd> {
    let name = final_normal_component(path).ok_or_else(|| AppError::InitRefused {
        path: path.to_path_buf(),
        reason: "destination must have one normal final path component".into(),
    })?;
    let parent = path
        .parent()
        .filter(|value| !value.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent_metadata = fs::symlink_metadata(parent)
        .map_err(|error| AppError::io("inspect parent", parent, error))?;
    if parent_metadata.file_type().is_symlink() || !parent_metadata.is_dir() {
        return Err(AppError::InitRefused {
            path: path.to_path_buf(),
            reason: "destination parent must be an existing real directory".into(),
        });
    }
    let parent_file = rustix::fs::open(
        parent,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| AppError::io("open parent", parent, error.into()))?;
    rustix::fs::mkdirat(&parent_file, &name, Mode::from_raw_mode(0o755))
        .map_err(|error| AppError::io("create root", path, error.into()))?;
    rustix::fs::openat(
        &parent_file,
        &name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| AppError::io("open new root", path, error.into()))
}

fn final_normal_component(path: &Path) -> Option<OsString> {
    match path.components().next_back()? {
        Component::Normal(name) => Some(name.to_os_string()),
        _ => None,
    }
}

fn reject_unrelated_contents(fd: &OwnedFd, path: &Path) -> AppResult<()> {
    const OWNED: &[&str] = &[
        "audio",
        "backups",
        "config.toml",
        "logs",
        "state",
        crate::locks::ROOT_LOCK,
    ];
    let mut entries =
        Dir::read_from(fd).map_err(|error| AppError::io("read destination", path, error.into()))?;
    for entry in &mut entries {
        let entry =
            entry.map_err(|error| AppError::io("read destination entry", path, error.into()))?;
        let name = entry.file_name().to_bytes();
        if name != b"." && name != b".." && !OWNED.iter().any(|owned| name == owned.as_bytes()) {
            return Err(AppError::InitRefused {
                path: path.to_path_buf(),
                reason: "non-empty destination contains an unrelated entry".into(),
            });
        }
    }
    Ok(())
}

fn ensure_dir<Fd: AsFd>(
    parent: Fd,
    name: &str,
    mode: u32,
    private: bool,
    path: &Path,
) -> AppResult<OwnedFd> {
    let created = match rustix::fs::mkdirat(&parent, name, Mode::from_raw_mode(mode)) {
        Ok(()) => true,
        Err(error) if error == rustix::io::Errno::EXIST => false,
        Err(error) => {
            return Err(AppError::io("create directory", path, error.into()));
        }
    };
    let fd = rustix::fs::openat(
        &parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| AppError::io("open directory", path, error.into()))?;
    if private {
        if created {
            rustix::fs::fchmod(&fd, Mode::from_raw_mode(mode))
                .map_err(|error| AppError::io("set directory permissions", path, error.into()))?;
        }
        verify_private(&fd, true, path.to_path_buf())?;
    }
    Ok(fd)
}

fn ensure_file<Fd: AsFd>(
    parent: Fd,
    name: &str,
    bytes: &[u8],
    mode: u32,
    private: bool,
    path: &Path,
) -> AppResult<()> {
    let created = rustix::fs::openat(
        &parent,
        name,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(mode),
    );
    match created {
        Ok(fd) => {
            rustix::fs::fchmod(&fd, Mode::from_raw_mode(mode))
                .map_err(|error| AppError::io("set file permissions", path, error.into()))?;
            let mut file = File::from(fd);
            file.write_all(bytes)
                .map_err(|error| AppError::io("write file", path, error))?;
            file.sync_all()
                .map_err(|error| AppError::io("sync file", path, error))?;
        }
        Err(error) if error == rustix::io::Errno::EXIST => {
            let fd = rustix::fs::openat(
                &parent,
                name,
                OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|open_error| AppError::io("open existing file", path, open_error.into()))?;
            if private {
                verify_private(&fd, false, path.to_path_buf())?;
            } else if !rustix::fs::FileType::from_raw_mode(
                rustix::fs::fstat(&fd)
                    .map_err(|stat_error| AppError::io("inspect file", path, stat_error.into()))?
                    .st_mode,
            )
            .is_file()
            {
                return Err(AppError::InitRefused {
                    path: path.to_path_buf(),
                    reason: "existing entry is not a regular file".into(),
                });
            }
        }
        Err(error) => return Err(AppError::io("create file", path, error.into())),
    }
    Ok(())
}

fn verify_root_owner<Fd: AsFd>(fd: Fd, path: &Path) -> AppResult<()> {
    let stat =
        rustix::fs::fstat(fd).map_err(|error| AppError::io("inspect root", path, error.into()))?;
    if stat.st_uid != getuid().as_raw() {
        return Err(AppError::InitRefused {
            path: path.to_path_buf(),
            reason: "destination is not owned by the current user".into(),
        });
    }
    Ok(())
}

fn verify_private<Fd: AsFd>(fd: Fd, directory: bool, path: PathBuf) -> AppResult<()> {
    let stat = rustix::fs::fstat(fd)
        .map_err(|error| AppError::io("inspect private storage", &path, error.into()))?;
    let kind = rustix::fs::FileType::from_raw_mode(stat.st_mode);
    let expected_kind = if directory {
        kind.is_dir()
    } else {
        kind.is_file()
    };
    let permissions = stat.st_mode & 0o777;
    let expected_mode = if directory { 0o700 } else { 0o600 };
    if !expected_kind || stat.st_uid != getuid().as_raw() || permissions != expected_mode {
        return Err(AppError::InitRefused {
            path,
            reason: format!(
                "private storage must be current-user owned and mode {expected_mode:o}; found mode {permissions:o}"
            ),
        });
    }
    Ok(())
}

fn descriptor_path(fd: &OwnedFd, display: &Path) -> AppResult<PathBuf> {
    fs::read_link(format!("/proc/self/fd/{}", fd.as_raw_fd()))
        .map_err(|error| AppError::io("resolve initialized root", display, error))
}
