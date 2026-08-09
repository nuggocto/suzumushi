// SPDX-License-Identifier: Apache-2.0

//! Bounded metadata parsing and helper-process isolation.

use std::borrow::Cow;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::os::fd::AsFd;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use lofty::config::ParseOptions;
use lofty::file::TaggedFileExt;
use lofty::probe::Probe;
use lofty::tag::{Accessor, ItemKey};
use rustix::process::{Resource, Rlimit, setrlimit};
use serde::{Deserialize, Serialize};

use crate::errors::{AppError, AppResult};
use crate::model::TrackTags;

const MAX_READ_BYTES: u64 = 8 * 1_048_576;
const MAX_READ_OPERATIONS: u64 = 4_096;
const MAX_HELPER_OUTPUT: usize = 65_536;
const HELPER_TIMEOUT: Duration = Duration::from_secs(3);

/// Metadata seam used to keep scanner tests deterministic.
pub trait MetadataReader {
    /// Reads bounded tags from one already verified regular descriptor.
    ///
    /// # Errors
    ///
    /// Returns a bounded diagnostic when parsing cannot safely produce tags.
    fn read(&mut self, file: &File) -> Result<TrackTags, String>;
}

/// Production reader that owns the kill-and-reap helper lifecycle.
#[derive(Debug, Default)]
pub struct HelperMetadataReader;

impl MetadataReader for HelperMetadataReader {
    fn read(&mut self, file: &File) -> Result<TrackTags, String> {
        parse_in_helper(file).map_err(|error| error.to_string())
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct HelperReply {
    tags: Option<TrackTags>,
    error: Option<String>,
}

/// Parses a verified descriptor in the isolated helper mode of this executable.
///
/// # Errors
///
/// Returns a helper error for spawn, timeout, crash, oversized/malformed reply,
/// parser refusal, or cleanup failure.
pub fn parse_in_helper(file: &File) -> AppResult<TrackTags> {
    let executable = std::env::current_exe()
        .map_err(|error| AppError::MetadataHelper(format!("cannot locate executable: {error}")))?;
    let mut command = Command::new(executable);
    command.arg("__metadata-helper");
    run_helper(command, file, HELPER_TIMEOUT)
}

fn run_helper(mut command: Command, file: &File, timeout: Duration) -> AppResult<TrackTags> {
    let mut child = command
        .stdin(Stdio::from(file.try_clone().map_err(|error| {
            AppError::MetadataHelper(format!("cannot duplicate media descriptor: {error}"))
        })?))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| AppError::MetadataHelper(format!("cannot start helper: {error}")))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| AppError::MetadataHelper("helper stdout is unavailable".into()))?;
    let reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.take(65_537).read_to_end(&mut bytes).map(|_| bytes)
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
            Ok(None) => {
                terminate_and_reap(&mut child, reader)?;
                return Err(AppError::MetadataHelper(
                    "helper timed out and was killed and reaped".into(),
                ));
            }
            Err(error) => {
                terminate_and_reap(&mut child, reader)?;
                return Err(AppError::MetadataHelper(format!(
                    "cannot wait for helper: {error}"
                )));
            }
        }
    };
    let output = reader
        .join()
        .map_err(|_| AppError::MetadataHelper("helper output reader panicked".into()))?
        .map_err(|error| AppError::MetadataHelper(format!("cannot read helper output: {error}")))?;
    if output.len() > MAX_HELPER_OUTPUT {
        return Err(AppError::MetadataHelper(
            "helper output exceeded 65536 bytes".into(),
        ));
    }
    if !status.success() {
        return Err(AppError::MetadataHelper(format!(
            "helper exited with {status}"
        )));
    }
    parse_reply(&output)
}

fn terminate_and_reap(
    child: &mut Child,
    reader: thread::JoinHandle<std::io::Result<Vec<u8>>>,
) -> AppResult<()> {
    let kill_error = child
        .kill()
        .err()
        .filter(|error| error.kind() != std::io::ErrorKind::InvalidInput);
    let wait_error = child.wait().err();
    let reader_error = match reader.join() {
        Ok(Ok(_)) => None,
        Ok(Err(error)) => Some(format!("output cleanup failed: {error}")),
        Err(_) => Some("output cleanup thread panicked".into()),
    };
    if kill_error.is_none() && wait_error.is_none() && reader_error.is_none() {
        return Ok(());
    }
    let mut details = Vec::new();
    if let Some(error) = kill_error {
        details.push(format!("kill failed: {error}"));
    }
    if let Some(error) = wait_error {
        details.push(format!("reap failed: {error}"));
    }
    if let Some(error) = reader_error {
        details.push(error);
    }
    Err(AppError::MetadataHelper(format!(
        "helper cleanup failed: {}",
        details.join("; ")
    )))
}

/// Runs the internal metadata helper, reading only its inherited standard input descriptor.
#[must_use]
pub fn helper_main() -> i32 {
    if let Err(error) = apply_helper_limits() {
        let _ = emit_reply(&HelperReply {
            tags: None,
            error: Some(error),
        });
        return 1;
    }
    let stdin = std::io::stdin();
    let fd = match rustix::io::dup(stdin.as_fd()) {
        Ok(fd) => fd,
        Err(error) => {
            let _ = emit_reply(&HelperReply {
                tags: None,
                error: Some(format!("cannot duplicate input descriptor: {error}")),
            });
            return 1;
        }
    };
    let file = File::from(fd);
    let reply = match std::panic::catch_unwind(|| parse_reader(file)) {
        Ok(Ok(tags)) => HelperReply {
            tags: Some(tags),
            error: None,
        },
        Ok(Err(error)) => HelperReply {
            tags: None,
            error: Some(error),
        },
        Err(_) => HelperReply {
            tags: None,
            error: Some("metadata parser panicked".into()),
        },
    };
    i32::from(emit_reply(&reply).is_err())
}

/// Exercises the same bounded adapter directly for fuzzing, without filesystem path lookup.
pub fn fuzz_parse(input: &[u8]) {
    let cursor = std::io::Cursor::new(input);
    let _ = std::panic::catch_unwind(|| parse_reader(cursor));
}

/// Exercises bounded helper reply framing for fuzzing.
pub fn fuzz_reply(input: &[u8]) {
    if input.len() <= MAX_HELPER_OUTPUT {
        let _ = parse_reply(input);
    }
}

fn parse_reply(output: &[u8]) -> AppResult<TrackTags> {
    let reply: HelperReply = serde_json::from_slice(output).map_err(|error| {
        AppError::MetadataHelper(format!("invalid bounded helper reply: {error}"))
    })?;
    match (reply.tags, reply.error) {
        (Some(tags), None) => Ok(tags),
        (None, Some(error)) => Err(AppError::MetadataHelper(error)),
        _ => Err(AppError::MetadataHelper(
            "helper reply has invalid state".into(),
        )),
    }
}

fn emit_reply(reply: &HelperReply) -> Result<(), String> {
    let bytes = serde_json::to_vec(&reply).map_err(|error| error.to_string())?;
    if bytes.len() > MAX_HELPER_OUTPUT {
        return Err("reply too large".into());
    }
    std::io::stdout()
        .write_all(&bytes)
        .map_err(|error| error.to_string())
}

fn apply_helper_limits() -> Result<(), String> {
    for (resource, value) in [
        (Resource::Cpu, 2),
        (Resource::As, 256 * 1_048_576),
        (Resource::Nofile, 16),
        (Resource::Core, 0),
        (Resource::Fsize, 1_048_576),
    ] {
        setrlimit(
            resource,
            Rlimit {
                current: Some(value),
                maximum: Some(value),
            },
        )
        .map_err(|error| format!("cannot apply helper resource limit: {error}"))?;
    }
    Ok(())
}

fn parse_reader<R: Read + Seek>(reader: R) -> Result<TrackTags, String> {
    let reader = BudgetedReader::new(reader)?;
    let tagged = Probe::new(BufReader::new(reader))
        .options(ParseOptions::new().read_properties(false))
        .guess_file_type()
        .map_err(|error| error.to_string())?
        .read()
        .map_err(|error| error.to_string())?;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag());
    Ok(tag.map_or_else(TrackTags::default, |tag| TrackTags {
        artist: tag.artist().map(Cow::into_owned),
        album_artist: tag.get_string(ItemKey::AlbumArtist).map(ToOwned::to_owned),
        album: tag.album().map(Cow::into_owned),
        title: tag.title().map(Cow::into_owned),
    }))
}

struct BudgetedReader<R> {
    inner: R,
    remaining_bytes: u64,
    remaining_operations: u64,
    length: u64,
}

impl<R: Read + Seek> BudgetedReader<R> {
    fn new(mut inner: R) -> Result<Self, String> {
        let current = inner.stream_position().map_err(|error| error.to_string())?;
        let length = inner
            .seek(SeekFrom::End(0))
            .map_err(|error| error.to_string())?;
        inner
            .seek(SeekFrom::Start(current))
            .map_err(|error| error.to_string())?;
        Ok(Self {
            inner,
            remaining_bytes: MAX_READ_BYTES,
            remaining_operations: MAX_READ_OPERATIONS,
            length,
        })
    }

    fn operation(&mut self) -> std::io::Result<()> {
        self.remaining_operations = self
            .remaining_operations
            .checked_sub(1)
            .ok_or_else(|| std::io::Error::other("metadata operation budget exhausted"))?;
        Ok(())
    }
}

impl<R: Read + Seek> Read for BudgetedReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.operation()?;
        if self.remaining_bytes == 0 {
            return Err(std::io::Error::other("metadata read budget exhausted"));
        }
        let allowed = buffer
            .len()
            .min(usize::try_from(self.remaining_bytes).unwrap_or(usize::MAX));
        let read = self.inner.read(&mut buffer[..allowed])?;
        self.remaining_bytes -= read as u64;
        Ok(read)
    }
}

impl<R: Read + Seek> Seek for BudgetedReader<R> {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        self.operation()?;
        let next = self.inner.seek(position)?;
        if next > self.length {
            return Err(std::io::Error::other("metadata seek escaped input length"));
        }
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::process::Command;
    use std::time::Duration;

    use super::run_helper;

    #[test]
    fn crashed_helper_is_reaped_and_reported() {
        let file = File::open("/dev/null").expect("open harmless input");
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "exit 7"]);

        let error =
            run_helper(command, &file, Duration::from_secs(1)).expect_err("crash must be visible");
        assert!(error.to_string().contains("helper exited with"));
    }

    #[test]
    fn uncooperative_helper_is_killed_and_reaped() {
        let file = File::open("/dev/null").expect("open harmless input");
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "while :; do :; done"]);

        let error = run_helper(command, &file, Duration::from_millis(20))
            .expect_err("timeout must be visible");
        assert!(
            error
                .to_string()
                .contains("helper timed out and was killed and reaped")
        );
    }
}
