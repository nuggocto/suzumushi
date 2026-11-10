// SPDX-License-Identifier: Apache-2.0

//! The per-root writer lock.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use rustix::fd::AsFd;
use rustix::fs::{FlockOperation, Mode, OFlags};
use rustix::process::getuid;

use crate::errors::{AppError, AppResult};

pub(crate) const ROOT_LOCK: &str = ".suzumushi-root.lock";
const MAX_IDENTITY_BYTES: u64 = 256;

/// Exclusive lease for mutable app state under one canonical root.
#[derive(Debug)]
pub struct RootWriterLease {
    file: File,
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
        acquire_and_record(file, "root writer").map(|file| Self { file })
    }
}

impl Drop for RootWriterLease {
    fn drop(&mut self) {
        // Release before close so a concurrent same-process handoff cannot
        // observe the advisory lock after this owner starts dropping it.
        let _ = rustix::fs::flock(&self.file, FlockOperation::Unlock);
    }
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
