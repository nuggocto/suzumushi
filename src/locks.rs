// SPDX-License-Identifier: Apache-2.0

//! Descriptor-relative root and per-user lock primitives.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use rustix::fd::{AsFd, OwnedFd};
use rustix::fs::{FlockOperation, Mode, OFlags};
use rustix::process::getuid;

use crate::errors::{AppError, AppResult};

pub(crate) const ROOT_LOCK: &str = ".suzumushi-root.lock";
const ACTIVE_LOCK: &str = "active-tui.lock";
const MAX_IDENTITY_BYTES: u64 = 256;

/// Exclusive lease for mutable app state under one canonical root.
#[derive(Debug)]
pub struct RootWriterLease {
    _file: File,
}

impl RootWriterLease {
    /// Attempts to acquire the root writer lease without blocking.
    ///
    /// # Errors
    ///
    /// Returns an error for an unsafe root/lock file or an already held lease.
    pub fn acquire(root: &Path) -> AppResult<Self> {
        let root_metadata = fs::symlink_metadata(root)
            .map_err(|error| AppError::io("inspect root", root, error))?;
        if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
            return Err(AppError::Lock(
                "root lock requires a real canonical directory".into(),
            ));
        }
        let root_fd = rustix::fs::open(
            root,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| AppError::io("open root", root, error.into()))?;
        Self::acquire_from(&root_fd, root)
    }

    pub(crate) fn acquire_from<Fd: AsFd>(root: Fd, display: &Path) -> AppResult<Self> {
        let file = open_private_lock(root, ROOT_LOCK, &display.join(ROOT_LOCK))?;
        acquire_and_record(file, "root writer").map(|file| Self { _file: file })
    }
}

/// Exclusive per-UID identity lease for the future mutable TUI.
#[derive(Debug)]
pub struct ActiveTuiLease {
    _file: File,
    /// Verified private runtime directory containing the lock.
    pub runtime_directory: PathBuf,
}

impl ActiveTuiLease {
    /// Acquires the per-user lease beneath the verified `XDG_RUNTIME_DIR`.
    ///
    /// # Errors
    ///
    /// Returns an error if runtime storage is absent/insecure or the lease is held.
    pub fn acquire() -> AppResult<Self> {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
        Self::acquire_in(runtime.as_deref())
    }

    /// Acquires the same lease with an injected runtime path for deterministic verification.
    ///
    /// # Errors
    ///
    /// Returns an error if runtime storage is absent/insecure or the lease is held.
    pub fn acquire_in(runtime: Option<&Path>) -> AppResult<Self> {
        let runtime = verified_runtime_dir(runtime)?;
        let runtime_fd = rustix::fs::open(
            &runtime,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| AppError::io("open runtime directory", &runtime, error.into()))?;
        let runtime_stat = rustix::fs::fstat(&runtime_fd).map_err(|error| {
            AppError::io("inspect opened runtime directory", &runtime, error.into())
        })?;
        if runtime_stat.st_uid != getuid().as_raw() || runtime_stat.st_mode & 0o077 != 0 {
            return Err(AppError::Lock(
                "opened XDG_RUNTIME_DIR is not current-user owned and private".into(),
            ));
        }
        let runtime_file = File::from(runtime_fd);
        let app_fd = ensure_private_app_dir(&runtime_file, &runtime)?;
        let app_path = runtime.join("suzumushi");
        let file = open_private_lock(&app_fd, ACTIVE_LOCK, &app_path.join(ACTIVE_LOCK))?;
        let file = acquire_and_record(file, "active TUI")?;
        Ok(Self {
            _file: file,
            runtime_directory: app_path,
        })
    }
}

fn verified_runtime_dir(runtime: Option<&Path>) -> AppResult<PathBuf> {
    let path = runtime.map(Path::to_path_buf).ok_or_else(|| {
        AppError::Lock("XDG_RUNTIME_DIR is not set; there is no /tmp fallback".into())
    })?;
    if !path.is_absolute() {
        return Err(AppError::Lock("XDG_RUNTIME_DIR must be absolute".into()));
    }
    let metadata = fs::symlink_metadata(&path)
        .map_err(|error| AppError::Lock(format!("cannot inspect XDG_RUNTIME_DIR: {error}")))?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.uid() != getuid().as_raw()
        || metadata.mode() & 0o077 != 0
    {
        return Err(AppError::Lock(
            "XDG_RUNTIME_DIR must be a real current-user-owned directory with no group/other access".into(),
        ));
    }
    Ok(path)
}

fn ensure_private_app_dir(runtime: &File, runtime_path: &Path) -> AppResult<OwnedFd> {
    let created = match rustix::fs::mkdirat(runtime, "suzumushi", Mode::from_raw_mode(0o700)) {
        Ok(()) => true,
        Err(error) if error == rustix::io::Errno::EXIST => false,
        Err(error) => {
            return Err(AppError::io(
                "create runtime app directory",
                runtime_path.join("suzumushi"),
                error.into(),
            ));
        }
    };
    let fd = rustix::fs::openat(
        runtime,
        "suzumushi",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| {
        AppError::io(
            "open runtime app directory",
            runtime_path.join("suzumushi"),
            error.into(),
        )
    })?;
    if created {
        rustix::fs::fchmod(&fd, Mode::from_raw_mode(0o700)).map_err(|error| {
            AppError::io(
                "set runtime app directory permissions",
                runtime_path.join("suzumushi"),
                error.into(),
            )
        })?;
    }
    let stat = rustix::fs::fstat(&fd).map_err(|error| {
        AppError::io(
            "inspect runtime app directory",
            runtime_path.join("suzumushi"),
            error.into(),
        )
    })?;
    if stat.st_uid != getuid().as_raw() || stat.st_mode & 0o777 != 0o700 {
        return Err(AppError::Lock(
            "XDG_RUNTIME_DIR/suzumushi must be current-user owned and mode 0700".into(),
        ));
    }
    Ok(fd)
}

fn open_private_lock<Fd: rustix::fd::AsFd>(
    parent: Fd,
    name: &str,
    display: &Path,
) -> AppResult<File> {
    let fd = rustix::fs::openat(
        parent,
        name,
        OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
    )
    .map_err(|error| AppError::io("open lock", display, error.into()))?;
    let stat = rustix::fs::fstat(&fd)
        .map_err(|error| AppError::io("inspect lock", display, error.into()))?;
    if !rustix::fs::FileType::from_raw_mode(stat.st_mode).is_file()
        || stat.st_uid != getuid().as_raw()
        || stat.st_mode & 0o777 != 0o600
        || stat.st_nlink != 1
    {
        return Err(AppError::Lock(format!(
            "{} must be a current-user-owned, single-link regular file with mode 0600",
            display.display()
        )));
    }
    Ok(File::from(fd))
}

fn acquire_and_record(mut file: File, label: &str) -> AppResult<File> {
    if let Err(error) = rustix::fs::flock(&file, FlockOperation::NonBlockingLockExclusive) {
        let mut holder = String::new();
        let _ = (&mut file)
            .take(MAX_IDENTITY_BYTES)
            .read_to_string(&mut holder);
        let detail = if holder.trim().is_empty() {
            "unknown holder"
        } else {
            holder.trim()
        };
        return Err(AppError::Lock(format!(
            "{label} lease is already held ({detail}): {error}"
        )));
    }
    file.set_len(0)
        .map_err(|error| AppError::Lock(format!("cannot reset {label} identity: {error}")))?;
    file.seek(SeekFrom::Start(0))
        .map_err(|error| AppError::Lock(format!("cannot seek {label} identity: {error}")))?;
    let identity = format!(
        "pid={} start_ticks={}\n",
        std::process::id(),
        process_start_ticks()?
    );
    file.write_all(identity.as_bytes())
        .map_err(|error| AppError::Lock(format!("cannot write {label} identity: {error}")))?;
    file.sync_data()
        .map_err(|error| AppError::Lock(format!("cannot sync {label} identity: {error}")))?;
    Ok(file)
}

fn process_start_ticks() -> AppResult<u64> {
    let stat = fs::read_to_string("/proc/self/stat")
        .map_err(|error| AppError::Lock(format!("cannot read process identity: {error}")))?;
    let close = stat
        .rfind(')')
        .ok_or_else(|| AppError::Lock("malformed process identity".into()))?;
    stat[close + 1..]
        .split_whitespace()
        .nth(19)
        .ok_or_else(|| AppError::Lock("process start ticks are missing".into()))?
        .parse()
        .map_err(|error| AppError::Lock(format!("invalid process start ticks: {error}")))
}
