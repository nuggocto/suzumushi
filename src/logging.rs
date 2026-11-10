// SPDX-License-Identifier: Apache-2.0

//! The session log: `logs/suzumushi.log`, one timestamped line per record.
//!
//! The app logs a handful of lines per session, so records are written straight
//! to the file under a mutex. When the file would pass [`MAX_FILE_BYTES`] it is
//! renamed to `suzumushi.log.1`, replacing the previous archive.

use std::fmt::{self, Write as _};
use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use rustix::fd::AsFd;
use rustix::fs::{AtFlags, FileType, Mode, OFlags};
use rustix::process::getuid;

use crate::errors::{AppError, AppResult};

const CURRENT_LOG: &str = "suzumushi.log";
const ARCHIVE_LOG: &str = "suzumushi.log.1";
/// Archives that releases before 1.3 kept beside the current one.
const RETIRED_ARCHIVES: [&str; 3] = ["suzumushi.log.2", "suzumushi.log.3", "suzumushi.log.4"];
const MAX_FILE_BYTES: u64 = 1_048_576;
const MAX_MESSAGE_BYTES: usize = 4_096;

static LOG: Mutex<Option<Logger>> = Mutex::new(None);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Level {
    Info,
    Warn,
    Error,
}

impl Level {
    const fn label(self) -> &'static str {
        match self {
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
        }
    }
}

pub(crate) fn info(message: fmt::Arguments<'_>) {
    write(Level::Info, message);
}

pub(crate) fn warn(message: fmt::Arguments<'_>) {
    write(Level::Warn, message);
}

pub(crate) fn error(message: fmt::Arguments<'_>) {
    write(Level::Error, message);
}

/// Appends one record when a session log is open, and does nothing otherwise.
fn write(level: Level, message: fmt::Arguments<'_>) {
    let mut log = LOG.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(logger) = log.as_mut() {
        logger.write(level, message, SystemTime::now());
    }
}

/// Owns the open session log. Finishing it closes the file.
#[must_use = "the log closes when the guard is finished or dropped"]
pub struct LoggingGuard(());

impl LoggingGuard {
    /// Closes the log and returns the first write failure, if any record was lost.
    #[must_use]
    pub fn finish(self) -> Option<String> {
        let logger = LOG.lock().unwrap_or_else(PoisonError::into_inner).take();
        std::mem::forget(self);
        logger.and_then(|logger| {
            logger.failure.map(|error| {
                format!(
                    "could not write {}: {error}",
                    logger.directory_path.join(CURRENT_LOG).display()
                )
            })
        })
    }
}

impl Drop for LoggingGuard {
    fn drop(&mut self) {
        LOG.lock().unwrap_or_else(PoisonError::into_inner).take();
    }
}

/// Opens the session log beneath an already pinned root.
///
/// # Errors
///
/// Returns an error when `logs` or the log file is not private to the current
/// user, or cannot be opened.
pub fn initialize_from<Fd: AsFd>(root_fd: Fd, root: &Path) -> AppResult<LoggingGuard> {
    let logger = Logger::open(root_fd, root, MAX_FILE_BYTES)?;
    *LOG.lock().unwrap_or_else(PoisonError::into_inner) = Some(logger);
    Ok(LoggingGuard(()))
}

struct Logger {
    directory: File,
    directory_path: PathBuf,
    file: File,
    bytes: u64,
    max_bytes: u64,
    failure: Option<io::Error>,
}

impl Logger {
    fn open<Fd: AsFd>(root_fd: Fd, root: &Path, max_bytes: u64) -> AppResult<Self> {
        let directory_path = root.join("logs");
        let directory = rustix::fs::openat(
            root_fd,
            "logs",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| AppError::io("open log directory", &directory_path, error.into()))?;
        let stat = rustix::fs::fstat(&directory).map_err(|error| {
            AppError::io("inspect log directory", &directory_path, error.into())
        })?;
        if !FileType::from_raw_mode(stat.st_mode).is_dir()
            || stat.st_uid != getuid().as_raw()
            || stat.st_mode & 0o777 != 0o700
        {
            return Err(AppError::InvalidRoot {
                path: directory_path,
                reason: "logs must be a current-user-owned directory with mode 0700".into(),
            });
        }
        let directory = File::from(directory);
        for name in RETIRED_ARCHIVES {
            match rustix::fs::unlinkat(&directory, name, AtFlags::empty()) {
                Ok(()) => {}
                Err(error) if error == rustix::io::Errno::NOENT => {}
                Err(error) => {
                    return Err(AppError::io(
                        "remove old log archive",
                        directory_path.join(name),
                        error.into(),
                    ));
                }
            }
        }
        let file = open_log_file(&directory)
            .map_err(|error| AppError::io("open log", directory_path.join(CURRENT_LOG), error))?;
        let bytes = rustix::fs::fstat(&file)
            .map_err(|error| {
                AppError::io(
                    "inspect log",
                    directory_path.join(CURRENT_LOG),
                    error.into(),
                )
            })?
            .st_size
            .unsigned_abs();
        Ok(Self {
            directory,
            directory_path,
            file,
            bytes,
            max_bytes,
            failure: None,
        })
    }

    fn write(&mut self, level: Level, message: fmt::Arguments<'_>, now: SystemTime) {
        if self.failure.is_some() {
            return;
        }
        let line = format_record(level, message, now);
        let result = self.rotate_for(line.len()).and_then(|()| {
            self.file.write_all(line.as_bytes())?;
            self.bytes += line.len() as u64;
            Ok(())
        });
        if let Err(error) = result {
            self.failure = Some(error);
        }
    }

    fn rotate_for(&mut self, line_bytes: usize) -> io::Result<()> {
        if self.bytes == 0 || self.bytes + line_bytes as u64 <= self.max_bytes {
            return Ok(());
        }
        rustix::fs::renameat(&self.directory, CURRENT_LOG, &self.directory, ARCHIVE_LOG)?;
        self.file = open_log_file(&self.directory)?;
        self.bytes = 0;
        Ok(())
    }
}

/// Opens or creates the current log without following a link or waiting on a FIFO.
fn open_log_file(directory: &File) -> io::Result<File> {
    let fd = rustix::fs::openat(
        directory,
        CURRENT_LOG,
        OFlags::WRONLY
            | OFlags::APPEND
            | OFlags::CREATE
            | OFlags::NONBLOCK
            | OFlags::NOFOLLOW
            | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
    )?;
    let stat = rustix::fs::fstat(&fd)?;
    if !FileType::from_raw_mode(stat.st_mode).is_file()
        || stat.st_uid != getuid().as_raw()
        || stat.st_mode & 0o777 != 0o600
        || stat.st_nlink != 1
    {
        return Err(io::Error::other(
            "the log must be a current-user-owned, single-link regular file with mode 0600",
        ));
    }
    Ok(File::from(fd))
}

fn format_record(level: Level, message: fmt::Arguments<'_>, now: SystemTime) -> String {
    let mut text = String::new();
    let _ = text.write_fmt(message);
    let message = crate::display::terminal_safe(text.as_bytes(), MAX_MESSAGE_BYTES);
    let seconds = now
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let (year, month, day) = civil_from_days(seconds / 86_400);
    let time = seconds % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z {} {message}\n",
        time / 3_600,
        time % 3_600 / 60,
        time % 60,
        level.label(),
    )
}

/// Converts days since 1970-01-01 to a proleptic Gregorian date.
///
/// Howard Hinnant's `civil_from_days`, limited to dates after the epoch.
const fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let day_of_era = z % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + if month <= 2 { 1 } else { 0 };
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use super::{Level, Logger, civil_from_days, format_record};

    fn at(seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds)
    }

    #[test]
    fn records_carry_a_utc_timestamp_and_escape_control_bytes() {
        let line = format_record(
            Level::Warn,
            format_args!("audio output ring underrun: {} samples\nforged", 128),
            at(1_762_748_400),
        );
        assert_eq!(
            line,
            "2025-11-10T04:20:00Z WARN audio output ring underrun: 128 samples\\x0Aforged\n"
        );
    }

    #[test]
    fn calendar_conversion_handles_leap_days_and_century_rules() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        assert_eq!(civil_from_days(11_017), (2000, 3, 1));
        assert_eq!(civil_from_days(47_541), (2100, 3, 1));
    }

    #[test]
    fn a_full_log_moves_to_one_archive_and_old_archives_are_removed() {
        let root = tempfile::tempdir().expect("temporary root");
        let logs = root.path().join("logs");
        fs::create_dir(&logs).expect("log directory");
        fs::set_permissions(&logs, fs::Permissions::from_mode(0o700)).expect("private logs");
        for name in ["suzumushi.log.1", "suzumushi.log.2", "suzumushi.log.4"] {
            fs::write(logs.join(name), b"old\n").expect("seed archive");
        }
        let root_file = fs::File::open(root.path()).expect("open root");

        // Room for two 32- and 33-byte records, not three.
        let mut logger = Logger::open(&root_file, root.path(), 70).expect("open log");
        assert!(!logs.join("suzumushi.log.2").exists());
        assert!(!logs.join("suzumushi.log.4").exists());
        logger.write(Level::Info, format_args!("first"), at(0));
        logger.write(Level::Info, format_args!("second"), at(0));
        logger.write(Level::Info, format_args!("third"), at(0));
        assert!(logger.failure.is_none());

        let current = fs::read_to_string(logs.join("suzumushi.log")).expect("current log");
        let archive = fs::read_to_string(logs.join("suzumushi.log.1")).expect("archive");
        assert_eq!(current, "1970-01-01T00:00:00Z INFO third\n");
        assert_eq!(
            archive,
            "1970-01-01T00:00:00Z INFO first\n1970-01-01T00:00:00Z INFO second\n"
        );
        let mode = fs::metadata(logs.join("suzumushi.log"))
            .expect("log metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
