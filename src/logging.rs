// SPDX-License-Identifier: Apache-2.0

//! Bounded private file logging with app-owned byte rotation.

use std::fs::File;
use std::io::{self, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use rustix::fd::AsFd;
use rustix::fs::{AtFlags, FileType, Mode, OFlags};
use rustix::process::getuid;
use tracing_subscriber::fmt::MakeWriter;

use crate::config::{LoggingConfig, MAX_LOG_FILES};
use crate::errors::{AppError, AppResult};

const CURRENT_LOG: &str = "suzumushi.log";
const TRUNCATED: &[u8] = b" [truncated]\n";

/// Keeps the logging worker owned until every queued record is flushed.
pub struct LoggingGuard {
    worker: Option<JoinHandle<()>>,
    queue: Arc<LogQueue>,
    write_failed: Arc<AtomicBool>,
    current_path: PathBuf,
}

impl LoggingGuard {
    /// Waits for queued records before reporting queue and writer failures.
    #[must_use]
    pub fn finish(mut self) -> LoggingReport {
        self.shutdown();
        LoggingReport {
            dropped_records: self.queue.dropped_records(),
            write_failed: self.write_failed.load(Ordering::Acquire),
            current_path: self.current_path.clone(),
        }
    }

    fn shutdown(&mut self) {
        self.queue.close();
        if self
            .worker
            .take()
            .is_some_and(|worker| worker.join().is_err())
        {
            self.write_failed.store(true, Ordering::Release);
        }
    }
}

impl Drop for LoggingGuard {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Final state of the bounded logging pipeline.
pub struct LoggingReport {
    pub dropped_records: usize,
    write_failed: bool,
    current_path: PathBuf,
}

impl LoggingReport {
    /// Returns an application error when the background writer lost data.
    #[must_use]
    pub fn writer_error(&self) -> Option<AppError> {
        self.write_failed.then(|| {
            AppError::io(
                "write log",
                &self.current_path,
                io::Error::other("background log writer failed"),
            )
        })
    }
}

/// Installs one process-wide file-only subscriber beneath a verified root.
///
/// # Errors
///
/// Returns an error for unsafe storage, rotation failure, or an existing global
/// tracing subscriber.
pub fn initialize(root: &Path, config: &LoggingConfig) -> AppResult<LoggingGuard> {
    let root_fd = rustix::fs::open(
        root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| AppError::io("open root for logging", root, error.into()))?;
    initialize_from(&root_fd, root, config)
}

/// Installs logging beneath an already pinned session root.
///
/// # Errors
///
/// Returns an error for unsafe storage, rotation failure, or an existing global
/// tracing subscriber.
pub fn initialize_from<Fd: AsFd>(
    root_fd: Fd,
    root: &Path,
    config: &LoggingConfig,
) -> AppResult<LoggingGuard> {
    let writer = RotatingWriter::open_from(root_fd, root, config.max_file_bytes, config.max_files)?;
    let current_path = writer.current_path();
    let write_failed = Arc::new(AtomicBool::new(false));
    let (queue, receiver) = LogQueue::new(config.queue_capacity, Arc::clone(&write_failed));
    let worker = spawn_log_worker(writer, receiver, Arc::clone(&write_failed), &current_path)?;
    let bounded = BoundedMakeWriter {
        queue: Arc::clone(&queue),
        max_record_bytes: config.max_record_bytes,
    };
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_target(false)
        .without_time()
        .with_writer(bounded)
        .finish();
    let guard = LoggingGuard {
        worker: Some(worker),
        queue,
        write_failed,
        current_path,
    };
    if let Err(error) = tracing::subscriber::set_global_default(subscriber) {
        let _ = guard.finish();
        return Err(AppError::InvalidConfig(format!(
            "cannot install file logging: {error}"
        )));
    }
    Ok(guard)
}

pub(crate) fn report_dropped_records(writer: &mut impl Write, count: usize) -> io::Result<()> {
    writeln!(writer, "warning: {count} diagnostic records were dropped")
}

struct LogQueue {
    sender: Mutex<Option<SyncSender<Vec<u8>>>>,
    dropped_records: AtomicUsize,
    write_failed: Arc<AtomicBool>,
}

impl LogQueue {
    fn new(capacity: usize, write_failed: Arc<AtomicBool>) -> (Arc<Self>, Receiver<Vec<u8>>) {
        let (sender, receiver) = mpsc::sync_channel(capacity);
        (
            Arc::new(Self {
                sender: Mutex::new(Some(sender)),
                dropped_records: AtomicUsize::new(0),
                write_failed,
            }),
            receiver,
        )
    }

    fn enqueue(&self, record: Vec<u8>) {
        let sender = self
            .sender
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(sender) = sender.as_ref() else {
            self.write_failed.store(true, Ordering::Release);
            return;
        };
        match sender.try_send(record) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                let _ = self.dropped_records.fetch_update(
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                    |count| Some(count.saturating_add(1)),
                );
            }
            Err(TrySendError::Disconnected(_)) => {
                self.write_failed.store(true, Ordering::Release);
            }
        }
    }

    fn close(&self) {
        self.sender
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }

    fn dropped_records(&self) -> usize {
        self.dropped_records.load(Ordering::Relaxed)
    }
}

fn spawn_log_worker<W: Write + Send + 'static>(
    writer: W,
    receiver: Receiver<Vec<u8>>,
    write_failed: Arc<AtomicBool>,
    display: &Path,
) -> AppResult<JoinHandle<()>> {
    thread::Builder::new()
        .name("suzumushi-log".into())
        .spawn(move || run_log_worker(writer, &receiver, &write_failed))
        .map_err(|error| AppError::io("spawn log worker", display, error))
}

fn run_log_worker<W: Write>(
    mut writer: W,
    receiver: &Receiver<Vec<u8>>,
    write_failed: &AtomicBool,
) {
    while let Ok(record) = receiver.recv() {
        if write_failed.load(Ordering::Acquire) {
            continue;
        }
        if writer
            .write_all(&record)
            .and_then(|()| writer.flush())
            .is_err()
        {
            write_failed.store(true, Ordering::Release);
        }
    }
    if !write_failed.load(Ordering::Acquire) && writer.flush().is_err() {
        write_failed.store(true, Ordering::Release);
    }
}

#[derive(Clone)]
struct BoundedMakeWriter {
    queue: Arc<LogQueue>,
    max_record_bytes: usize,
}

impl<'a> MakeWriter<'a> for BoundedMakeWriter {
    type Writer = BoundedRecordWriter;

    fn make_writer(&'a self) -> Self::Writer {
        BoundedRecordWriter {
            queue: Arc::clone(&self.queue),
            bytes: Vec::with_capacity(self.max_record_bytes.min(1_024)),
            max_record_bytes: self.max_record_bytes,
            state: RecordState::Open,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RecordState {
    Open,
    Truncated,
    Emitted,
}

struct BoundedRecordWriter {
    queue: Arc<LogQueue>,
    bytes: Vec<u8>,
    max_record_bytes: usize,
    state: RecordState,
}

impl BoundedRecordWriter {
    fn emit(&mut self) {
        if self.state == RecordState::Emitted || self.bytes.is_empty() {
            return;
        }
        if self.state == RecordState::Truncated {
            mark_truncated(&mut self.bytes, self.max_record_bytes);
        }
        self.bytes.truncate(self.max_record_bytes);
        self.queue.enqueue(std::mem::take(&mut self.bytes));
        self.state = RecordState::Emitted;
    }
}

impl Write for BoundedRecordWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.state == RecordState::Emitted {
            return Ok(bytes.len());
        }
        let remaining = self.max_record_bytes.saturating_sub(self.bytes.len());
        let accepted = remaining.min(bytes.len());
        self.bytes.extend_from_slice(&bytes[..accepted]);
        if accepted < bytes.len() {
            self.state = RecordState::Truncated;
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.emit();
        Ok(())
    }
}

impl Drop for BoundedRecordWriter {
    fn drop(&mut self) {
        self.emit();
    }
}

struct RotatingWriter {
    directory: File,
    directory_path: PathBuf,
    file: File,
    bytes_written: usize,
    max_file_bytes: usize,
    max_files: usize,
}

impl RotatingWriter {
    fn open_from<Fd: AsFd>(
        root_fd: Fd,
        root: &Path,
        max_file_bytes: usize,
        max_files: usize,
    ) -> AppResult<Self> {
        let directory_path = root.join("logs");
        let directory_fd = rustix::fs::openat(
            root_fd,
            "logs",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| AppError::io("open log directory", &directory_path, error.into()))?;
        let directory_stat = rustix::fs::fstat(&directory_fd).map_err(|error| {
            AppError::io("inspect log directory", &directory_path, error.into())
        })?;
        if !FileType::from_raw_mode(directory_stat.st_mode).is_dir()
            || directory_stat.st_uid != getuid().as_raw()
            || directory_stat.st_mode & 0o777 != 0o700
        {
            return Err(AppError::Lock(
                "logs must be a current-user-owned directory with mode 0700".into(),
            ));
        }
        let directory = File::from(directory_fd);
        prune_archives(&directory, &directory_path, max_files)
            .map_err(|error| AppError::io("prune old log archives", &directory_path, error))?;
        let (file, bytes_written) = open_log_file(&directory, &directory_path, CURRENT_LOG)?;
        let mut writer = Self {
            directory,
            directory_path,
            file,
            bytes_written,
            max_file_bytes,
            max_files,
        };
        if writer.bytes_written >= writer.max_file_bytes {
            writer
                .rotate()
                .map_err(|error| AppError::io("rotate full log", writer.current_path(), error))?;
        }
        Ok(writer)
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.file.flush()?;
        verify_open_log(&self.file, &self.current_path())?;
        let current = open_existing_log(&self.directory, &self.directory_path, CURRENT_LOG)?
            .ok_or_else(|| io::Error::other("current log path disappeared before rotation"))?;
        let current_stat = rustix::fs::fstat(&current)?;
        let held_stat = rustix::fs::fstat(&self.file)?;
        if current_stat.st_dev != held_stat.st_dev || current_stat.st_ino != held_stat.st_ino {
            return Err(io::Error::other(
                "current log identity changed before rotation",
            ));
        }
        drop(current);

        if self.max_files == 1 {
            self.file.set_len(0)?;
            self.file.seek(SeekFrom::Start(0))?;
            self.bytes_written = 0;
            return Ok(());
        }

        for destination_index in (1..self.max_files).rev() {
            let source = if destination_index == 1 {
                CURRENT_LOG.to_owned()
            } else {
                archive_name(destination_index - 1)
            };
            if open_existing_log(&self.directory, &self.directory_path, &source)?.is_none() {
                continue;
            }
            let destination = archive_name(destination_index);
            if open_existing_log(&self.directory, &self.directory_path, &destination)?.is_some() {
                rustix::fs::unlinkat(&self.directory, &destination, AtFlags::empty())?;
            }
            rustix::fs::renameat(&self.directory, &source, &self.directory, &destination)?;
        }
        let (file, bytes_written) =
            create_log_file(&self.directory, &self.directory_path, CURRENT_LOG)?;
        self.file = file;
        self.bytes_written = bytes_written;
        Ok(())
    }

    fn current_path(&self) -> PathBuf {
        self.directory_path.join(CURRENT_LOG)
    }
}

impl Write for RotatingWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let projected = self
            .bytes_written
            .checked_add(bytes.len())
            .ok_or_else(|| io::Error::other("log byte count overflow"))?;
        if self.bytes_written > 0 && projected > self.max_file_bytes {
            self.rotate()?;
        }
        let written = self.file.write(bytes)?;
        self.bytes_written = self
            .bytes_written
            .checked_add(written)
            .ok_or_else(|| io::Error::other("log byte count overflow"))?;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

fn open_log_file(directory: &File, display: &Path, name: &str) -> AppResult<(File, usize)> {
    match create_log_file(directory, display, name) {
        Ok(value) => Ok(value),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let file = open_existing_log(directory, display, name)
                .map_err(|error| AppError::io("open existing log", display.join(name), error))?
                .ok_or_else(|| {
                    AppError::io(
                        "open existing log",
                        display.join(name),
                        io::Error::new(io::ErrorKind::NotFound, "log disappeared"),
                    )
                })?;
            let size = log_size(&file, &display.join(name))
                .map_err(|error| AppError::io("inspect existing log", display.join(name), error))?;
            Ok((file, size))
        }
        Err(error) => Err(AppError::io("create log", display.join(name), error)),
    }
}

fn create_log_file(directory: &File, display: &Path, name: &str) -> io::Result<(File, usize)> {
    let fd = rustix::fs::openat(
        directory,
        name,
        OFlags::WRONLY
            | OFlags::APPEND
            | OFlags::CREATE
            | OFlags::EXCL
            | OFlags::NOFOLLOW
            | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
    )?;
    rustix::fs::fchmod(&fd, Mode::from_raw_mode(0o600))?;
    let file = File::from(fd);
    verify_open_log(&file, &display.join(name))?;
    Ok((file, 0))
}

fn open_existing_log(directory: &File, display: &Path, name: &str) -> io::Result<Option<File>> {
    let fd = match rustix::fs::openat(
        directory,
        name,
        OFlags::WRONLY | OFlags::APPEND | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => fd,
        Err(error) if error == rustix::io::Errno::NOENT => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let file = File::from(fd);
    verify_open_log(&file, &display.join(name))?;
    Ok(Some(file))
}

fn verify_open_log(file: &File, display: &Path) -> io::Result<()> {
    let stat = rustix::fs::fstat(file)?;
    if !FileType::from_raw_mode(stat.st_mode).is_file()
        || stat.st_uid != getuid().as_raw()
        || stat.st_mode & 0o777 != 0o600
        || stat.st_nlink != 1
    {
        return Err(io::Error::other(format!(
            "{} must be a current-user-owned, single-link regular file with mode 0600",
            display.display()
        )));
    }
    Ok(())
}

fn log_size(file: &File, display: &Path) -> io::Result<usize> {
    let stat = rustix::fs::fstat(file)?;
    usize::try_from(stat.st_size).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} has an invalid size", display.display()),
        )
    })
}

fn archive_name(index: usize) -> String {
    format!("{CURRENT_LOG}.{index}")
}

fn prune_archives(directory: &File, display: &Path, max_files: usize) -> io::Result<()> {
    for index in max_files..MAX_LOG_FILES {
        let name = archive_name(index);
        if open_existing_log(directory, display, &name)?.is_some() {
            rustix::fs::unlinkat(directory, &name, AtFlags::empty())?;
        }
    }
    Ok(())
}

fn mark_truncated(bytes: &mut Vec<u8>, limit: usize) {
    if limit >= TRUNCATED.len() {
        bytes.truncate(limit - TRUNCATED.len());
        bytes.extend_from_slice(TRUNCATED);
    } else {
        bytes.clear();
        bytes.extend_from_slice(&b"~\n"[..limit.min(2)]);
    }
}

#[cfg(test)]
mod tests {
    use std::io::{self, Write};
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;

    use super::{
        BoundedRecordWriter, LogQueue, LoggingGuard, RecordState, TRUNCATED, mark_truncated,
        report_dropped_records, spawn_log_worker,
    };

    #[test]
    fn oversized_records_keep_a_visible_marker_inside_the_bound() {
        let limit = 20;
        let mut record = b"a diagnostic that is too long".to_vec();
        mark_truncated(&mut record, limit);
        assert_eq!(record.len(), limit);
        assert!(record.ends_with(TRUNCATED));

        let mut tiny = b"long".to_vec();
        mark_truncated(&mut tiny, 1);
        assert_eq!(tiny, b"~");
    }

    #[test]
    fn record_writer_truncates_and_emits_only_once() {
        let failed = Arc::new(AtomicBool::new(false));
        let (queue, receiver) = LogQueue::new(2, failed);
        let mut writer = BoundedRecordWriter {
            queue: Arc::clone(&queue),
            bytes: Vec::new(),
            max_record_bytes: 20,
            state: RecordState::Open,
        };

        writer
            .write_all(&[b'x'; 32])
            .expect("bounded record accepts the formatter write");
        writer.flush().expect("first flush emits the record");
        writer
            .write_all(b"ignored after emission")
            .expect("later formatter writes remain harmless");
        writer.flush().expect("later flush remains idempotent");
        drop(writer);

        let record = receiver.try_recv().expect("one record was emitted");
        assert_eq!(record.len(), 20);
        assert!(record.ends_with(TRUNCATED));
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
    }

    #[test]
    fn background_failures_and_queue_drops_have_independent_reports() {
        struct BrokenWriter;

        impl Write for BrokenWriter {
            fn write(&mut self, _bytes: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("fixture write failure"))
            }

            fn flush(&mut self) -> io::Result<()> {
                Err(io::Error::other("fixture flush failure"))
            }
        }

        let failed = Arc::new(AtomicBool::new(false));
        let (queue, receiver) = LogQueue::new(1, Arc::clone(&failed));
        let worker = spawn_log_worker(
            BrokenWriter,
            receiver,
            Arc::clone(&failed),
            std::path::Path::new("fixture.log"),
        )
        .expect("spawn fixture worker");
        queue.enqueue(b"record".to_vec());
        queue.close();
        worker.join().expect("join fixture worker");
        assert!(failed.load(Ordering::Acquire));

        let mut warning = Vec::new();
        report_dropped_records(&mut warning, 3).expect("write direct warning");
        assert_eq!(warning, b"warning: 3 diagnostic records were dropped\n");
    }

    #[test]
    fn logging_guard_waits_for_its_worker_to_finish() {
        struct HeldWriter {
            entered: mpsc::SyncSender<()>,
            release: mpsc::Receiver<()>,
        }

        impl Write for HeldWriter {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.entered.send(()).expect("announce blocked write");
                self.release.recv().expect("release blocked write");
                Ok(bytes.len())
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let failed = Arc::new(AtomicBool::new(false));
        let (queue, receiver) = LogQueue::new(1, Arc::clone(&failed));
        let (entered_tx, entered_rx) = mpsc::sync_channel(0);
        let (release_tx, release_rx) = mpsc::sync_channel(0);
        let worker = spawn_log_worker(
            HeldWriter {
                entered: entered_tx,
                release: release_rx,
            },
            receiver,
            Arc::clone(&failed),
            std::path::Path::new("fixture.log"),
        )
        .expect("spawn held worker");
        queue.enqueue(b"record".to_vec());
        entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("worker starts writing");
        let guard = LoggingGuard {
            worker: Some(worker),
            queue,
            write_failed: failed,
            current_path: PathBuf::from("fixture.log"),
        };
        let (finished_tx, finished_rx) = mpsc::sync_channel(0);
        let finisher = std::thread::spawn(move || {
            let report = guard.finish();
            finished_tx.send(report).expect("report finish completion");
        });

        assert!(matches!(
            finished_rx.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        release_tx.send(()).expect("release worker");
        let report = finished_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("finish waits and then returns");
        assert!(report.writer_error().is_none());
        finisher.join().expect("join finisher");
    }
}
