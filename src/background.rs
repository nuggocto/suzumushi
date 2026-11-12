// SPDX-License-Identifier: Apache-2.0

//! Finds out whether the terminal draws on a dark or a light background.
//!
//! The terminal is asked for its background color (OSC 11) and, in the same
//! write, for its primary device attributes (DA1). Terminals answer in order,
//! and nearly every terminal answers DA1, so that reply ends the exchange: a
//! terminal that ignores OSC 11 costs one round trip, not a timeout. Without a
//! color, `COLORFGBG` decides, and failing that the background is taken to be
//! dark.

use std::io::{IsTerminal, Write};
use std::os::fd::{AsFd, BorrowedFd};
use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::io::Errno;

use crate::config::Background;

/// OSC 11 asks for the background color; DA1 follows as an end marker.
const QUERY: &[u8] = b"\x1b]11;?\x1b\\\x1b[c";
/// How long a terminal that answers neither question may delay startup.
const REPLY_TIMEOUT: Duration = Duration::from_millis(300);
/// Both replies together are well under this.
const MAX_REPLY_BYTES: usize = 512;
const BLACK: [u8; 3] = [0, 0, 0];
const WHITE: [u8; 3] = [0xFF, 0xFF, 0xFF];

/// The background color to choose the Player's colors for, when it is known.
///
/// `Auto` asks the terminal, so it must run in raw mode and before anything
/// else reads terminal input; otherwise the reply would arrive as keystrokes.
/// Keys typed during the few milliseconds of the exchange are discarded.
#[must_use]
pub fn resolve(choice: Background) -> Option<[u8; 3]> {
    match choice {
        Background::Dark => Some(BLACK),
        Background::Light => Some(WHITE),
        Background::Auto => ask_terminal().or_else(|| {
            std::env::var("COLORFGBG")
                .ok()
                .and_then(|value| from_colorfgbg(&value))
        }),
    }
}

/// Asks the terminal on standard input and output, the descriptors crossterm
/// reads keys from and draws to.
fn ask_terminal() -> Option<[u8; 3]> {
    let input = std::io::stdin();
    let mut output = std::io::stdout();
    if !input.is_terminal() || !output.is_terminal() {
        return None;
    }
    output.write_all(QUERY).and_then(|()| output.flush()).ok()?;
    let reply = read_reply(input.as_fd(), Instant::now() + REPLY_TIMEOUT);
    parse_background(&reply)
}

/// Reads until the DA1 reply arrives, the deadline passes, or input fails.
fn read_reply(input: BorrowedFd<'_>, deadline: Instant) -> Vec<u8> {
    let mut reply = Vec::new();
    let mut buffer = [0_u8; 128];
    while !has_device_attributes(&reply) && reply.len() < MAX_REPLY_BYTES {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let Ok(timeout) = Timespec::try_from(remaining) else {
            break;
        };
        if remaining.is_zero() {
            break;
        }
        match poll(&mut [PollFd::new(&input, PollFlags::IN)], Some(&timeout)) {
            Ok(0) => break,
            Ok(_) | Err(Errno::INTR) => {}
            Err(_) => break,
        }
        match rustix::io::read(input, &mut buffer) {
            Ok(0) => break,
            Ok(read) => reply.extend_from_slice(&buffer[..read]),
            Err(Errno::INTR | Errno::AGAIN) => {}
            Err(_) => break,
        }
    }
    reply
}

/// Whether `reply` holds a complete DA1 answer, `ESC [ ? <params> c`.
fn has_device_attributes(reply: &[u8]) -> bool {
    (0..reply.len()).any(|start| {
        reply[start..].strip_prefix(b"\x1b[?").is_some_and(|rest| {
            let params = rest
                .iter()
                .take_while(|byte| byte.is_ascii_digit() || **byte == b';')
                .count();
            rest.get(params) == Some(&b'c')
        })
    })
}

/// The color in an OSC 11 reply, `ESC ] 11 ; rgb:R/G/B` ended by BEL or ST.
fn parse_background(reply: &[u8]) -> Option<[u8; 3]> {
    const PREFIX: &[u8] = b"\x1b]11;";
    let start = reply
        .windows(PREFIX.len())
        .position(|window| window == PREFIX)?
        + PREFIX.len();
    let body = &reply[start..];
    let end = body
        .iter()
        .position(|byte| *byte == 0x07 || *byte == 0x1b)?;
    parse_color(std::str::from_utf8(&body[..end]).ok()?)
}

/// An X11 color spec as terminals report it: `rgb:` with three channels, or
/// `rgba:` with a fourth for opacity, each one to four hex digits.
fn parse_color(spec: &str) -> Option<[u8; 3]> {
    let (channels, count) = if let Some(rest) = spec.strip_prefix("rgb:") {
        (rest, 3)
    } else {
        (spec.strip_prefix("rgba:")?, 4)
    };
    let parts: Vec<&str> = channels.split('/').collect();
    if parts.len() != count {
        return None;
    }
    let mut rgb = [0_u8; 3];
    for (channel, hex) in rgb.iter_mut().zip(&parts) {
        *channel = scale_channel(hex)?;
    }
    Some(rgb)
}

/// Scales a channel of one to four hex digits to eight bits.
fn scale_channel(hex: &str) -> Option<u8> {
    if hex.is_empty() || hex.len() > 4 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let value = u32::from_str_radix(hex, 16).ok()?;
    let max = (1_u32 << (4 * hex.len())) - 1;
    u8::try_from((value * 255 + max / 2) / max).ok()
}

/// `COLORFGBG` is `fg;bg` (sometimes `fg;default;bg`) in ANSI color numbers.
/// Backgrounds 0 to 6 and 8 are dark; 7 and 9 to 15 are light, as Vim reads it.
fn from_colorfgbg(value: &str) -> Option<[u8; 3]> {
    match value.rsplit(';').next()?.parse::<u8>().ok()? {
        0..=6 | 8 => Some(BLACK),
        7 | 9..=15 => Some(WHITE),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{from_colorfgbg, has_device_attributes, parse_background, read_reply};
    use std::time::{Duration, Instant};

    #[test]
    fn osc_11_replies_parse_in_every_channel_width_and_terminator() {
        for (reply, expected) in [
            (&b"\x1b]11;rgb:ffff/ffff/ffff\x1b\\"[..], [255, 255, 255]),
            (b"\x1b]11;rgb:fdfd/f6f6/e3e3\x07", [0xFD, 0xF6, 0xE3]),
            (b"\x1b]11;rgb:1e/1e/2e\x1b\\", [0x1E, 0x1E, 0x2E]),
            (b"\x1b]11;rgb:f/0/8\x07", [255, 0, 136]),
            (b"\x1b]11;rgb:800/000/fff\x07", [128, 0, 255]),
            (b"\x1b]11;rgba:0000/0000/0000/c000\x1b\\", [0, 0, 0]),
            // A key typed just before the reply does not hide it.
            (
                b"j\x1b]11;rgb:ffff/ffff/ffff\x1b\\\x1b[?62;22c",
                [255, 255, 255],
            ),
        ] {
            assert_eq!(parse_background(reply), Some(expected), "{reply:?}");
        }
    }

    #[test]
    fn malformed_or_missing_colors_are_unknown() {
        for reply in [
            &b""[..],
            b"\x1b[?62;22c",
            b"\x1b]11;rgb:ffff/ffff\x07",
            b"\x1b]11;rgb:ffff/ffff/ffff/ffff\x07",
            b"\x1b]11;rgb:fffff/0/0\x07",
            b"\x1b]11;rgb:zz/00/00\x07",
            b"\x1b]11;rgb://\x07",
            b"\x1b]11;#ffffff\x07",
            // Cut off before its terminator.
            b"\x1b]11;rgb:ffff/ffff/ffff",
        ] {
            assert_eq!(parse_background(reply), None, "{reply:?}");
        }
    }

    #[test]
    fn the_exchange_ends_at_a_complete_device_attributes_reply() {
        assert!(has_device_attributes(b"\x1b[?62;22c"));
        assert!(has_device_attributes(b"\x1b]11;rgb:0/0/0\x07\x1b[?6c"));
        assert!(has_device_attributes(b"\x1b[?c"));
        assert!(!has_device_attributes(b"\x1b[?62;22"));
        assert!(!has_device_attributes(b"\x1b[62c"));
        assert!(!has_device_attributes(b"\x1b]11;rgb:0/0/0\x07"));
    }

    #[test]
    fn colorfgbg_names_the_background_by_ansi_number() {
        assert_eq!(from_colorfgbg("15;0"), Some([0, 0, 0]));
        assert_eq!(from_colorfgbg("0;15"), Some([255, 255, 255]));
        assert_eq!(from_colorfgbg("0;default;7"), Some([255, 255, 255]));
        assert_eq!(from_colorfgbg("7;8"), Some([0, 0, 0]));
        assert_eq!(from_colorfgbg("15;default"), None);
        assert_eq!(from_colorfgbg(""), None);
        assert_eq!(from_colorfgbg("0;99"), None);
    }

    #[test]
    fn reading_stops_at_the_end_marker_and_at_the_deadline() {
        let (reader, mut writer) = std::io::pipe().expect("pipe");
        std::io::Write::write_all(&mut writer, b"\x1b]11;rgb:ffff/ffff/ffff\x1b\\\x1b[?62;22c")
            .expect("write reply");
        let reply = read_reply(
            std::os::fd::AsFd::as_fd(&reader),
            Instant::now() + Duration::from_secs(5),
        );
        assert_eq!(parse_background(&reply), Some([255, 255, 255]));

        // Silence: the read gives up at the deadline instead of blocking.
        let started = Instant::now();
        let reply = read_reply(
            std::os::fd::AsFd::as_fd(&reader),
            Instant::now() + Duration::from_millis(50),
        );
        assert_eq!(reply, Vec::<u8>::new());
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
