// SPDX-License-Identifier: Apache-2.0

//! Bounded terminal image capability discovery.

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, Timespec, poll};

use crate::errors::{AppError, AppResult};

const QUERY_TIMEOUT: Duration = Duration::from_millis(150);
const MAX_RESPONSE_BYTES: usize = 4_096;
const DEVICE_ATTRIBUTES_QUERY: &[u8] = b"\x1b[c";

/// Image protocol recorded for the current terminal session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageProtocol {
    Disabled,
    Fallback,
    Kitty,
    Sixel,
    Iterm2,
}

impl ImageProtocol {
    /// Calm, bounded label used by the placeholder UI and logs.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Fallback => "fallback",
            Self::Kitty => "kitty",
            Self::Sixel => "sixel",
            Self::Iterm2 => "iterm2",
        }
    }
}

/// Result of one bounded startup probe.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProbeResult {
    pub protocol: ImageProtocol,
    pub timed_out: bool,
    pub cancelled: bool,
}

/// Queries the active terminal without forwarding escape traffic through multiplexers.
///
/// # Errors
///
/// Returns an I/O error if the fixed query cannot be written or terminal input
/// cannot be polled safely.
pub fn query(enabled: bool) -> AppResult<ProbeResult> {
    if !enabled {
        return Ok(fallback(ImageProtocol::Disabled));
    }
    if std::env::var_os("TMUX").is_some() || std::env::var_os("ZELLIJ").is_some() {
        return Ok(fallback(ImageProtocol::Fallback));
    }
    if let Some(protocol) = environment_protocol() {
        return Ok(fallback(protocol));
    }

    let mut output = std::io::stdout().lock();
    output
        .write_all(DEVICE_ATTRIBUTES_QUERY)
        .and_then(|()| output.flush())
        .map_err(|error| AppError::io("query image capability", "terminal", error))?;

    let mut input = std::io::stdin().lock();
    let started = Instant::now();
    let mut response = Vec::with_capacity(256);
    loop {
        let remaining = QUERY_TIMEOUT.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            if !response.is_empty() {
                return Err(incomplete_reply_error());
            }
            return Ok(ProbeResult {
                protocol: ImageProtocol::Fallback,
                timed_out: true,
                cancelled: false,
            });
        }
        let timeout = Timespec::try_from(remaining)
            .map_err(|error| AppError::InvalidConfig(format!("invalid probe timeout: {error}")))?;
        let mut descriptors = [PollFd::new(&input, PollFlags::IN)];
        match poll(&mut descriptors, Some(&timeout)) {
            Ok(0) => continue,
            Ok(_) if descriptors[0].revents().contains(PollFlags::IN) => {}
            Ok(_)
                if descriptors[0]
                    .revents()
                    .intersects(PollFlags::ERR | PollFlags::HUP | PollFlags::NVAL) =>
            {
                return Ok(fallback(ImageProtocol::Fallback));
            }
            Ok(_) => continue,
            Err(error) if error == rustix::io::Errno::INTR => continue,
            Err(error) => {
                return Err(AppError::io(
                    "poll image capability",
                    "terminal",
                    error.into(),
                ));
            }
        }

        let remaining_bytes = MAX_RESPONSE_BYTES.saturating_sub(response.len());
        if remaining_bytes == 0 {
            return Err(AppError::io(
                "read image capability",
                "terminal",
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "terminal capability reply exceeded 4096 bytes",
                ),
            ));
        }
        let mut chunk = [0_u8; 256];
        let chunk_limit = remaining_bytes.min(chunk.len());
        let read = input
            .read(&mut chunk[..chunk_limit])
            .map_err(|error| AppError::io("read image capability", "terminal", error))?;
        if read == 0 {
            return Ok(fallback(ImageProtocol::Fallback));
        }
        response.extend_from_slice(&chunk[..read]);
        if response.contains(&b'q') || response.contains(&3) {
            return Ok(ProbeResult {
                protocol: ImageProtocol::Fallback,
                timed_out: false,
                cancelled: true,
            });
        }
        if let Some(protocol) = parse_response(&response) {
            return Ok(fallback(protocol));
        }
    }
}

/// Parses only the bounded protocol replies Suzumushi sends at startup.
#[must_use]
pub fn parse_response(response: &[u8]) -> Option<ImageProtocol> {
    let mut offset = 0;
    while let Some(start) = find_from(response, b"\x1b[?", offset) {
        let body_start = start + 3;
        let Some(end_offset) = response[body_start..].iter().position(|byte| *byte == b'c') else {
            break;
        };
        let body = &response[body_start..body_start + end_offset];
        if !body
            .iter()
            .all(|byte| byte.is_ascii_digit() || *byte == b';')
        {
            offset = body_start + end_offset + 1;
            continue;
        }
        if body
            .split(|byte| *byte == b';')
            .any(|parameter| parameter == b"4")
        {
            return Some(ImageProtocol::Sixel);
        }
        return Some(ImageProtocol::Fallback);
    }
    None
}

/// Exercises the bounded response parser for fuzzing.
pub fn fuzz_parse(response: &[u8]) {
    let _ = parse_response(&response[..response.len().min(MAX_RESPONSE_BYTES)]);
}

fn fallback(protocol: ImageProtocol) -> ProbeResult {
    ProbeResult {
        protocol,
        timed_out: false,
        cancelled: false,
    }
}

fn environment_protocol() -> Option<ImageProtocol> {
    let term_program = std::env::var_os("TERM_PROGRAM");
    if term_program.as_deref() == Some(std::ffi::OsStr::new("iTerm.app")) {
        return Some(ImageProtocol::Iterm2);
    }
    if std::env::var_os("KITTY_WINDOW_ID").is_some()
        || std::env::var_os("TERM").is_some_and(|value| value == "xterm-kitty")
        || term_program.as_deref() == Some(std::ffi::OsStr::new("WezTerm"))
        || term_program.as_deref() == Some(std::ffi::OsStr::new("ghostty"))
    {
        return Some(ImageProtocol::Kitty);
    }
    None
}

fn incomplete_reply_error() -> AppError {
    AppError::io(
        "read image capability",
        "terminal",
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "terminal capability reply did not finish before the deadline",
        ),
    )
}

fn find_from(haystack: &[u8], needle: &[u8], offset: usize) -> Option<usize> {
    haystack
        .get(offset..)?
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|position| offset + position)
}

#[cfg(test)]
mod tests {
    use super::{ImageProtocol, parse_response};

    #[test]
    fn capability_replies_are_bounded_and_specific() {
        assert_eq!(parse_response(b"noise\x1b_Gi=31;OK"), None);
        assert_eq!(
            parse_response(b"\x1b[?1;2;4;6c"),
            Some(ImageProtocol::Sixel)
        );
        assert_eq!(
            parse_response(b"\x1b[?1;2;6c"),
            Some(ImageProtocol::Fallback)
        );
        assert_eq!(parse_response(b"\x1b[?1;2;4;6"), None);
        assert_eq!(parse_response(b"\x1b[?1;x;4c"), None);
        assert_eq!(parse_response(b"\x1b_Gi=31;ENOTSUP\x1b\\"), None);
    }
}
