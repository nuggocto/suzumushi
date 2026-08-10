// SPDX-License-Identifier: Apache-2.0

//! Original fixed-size Suzu character frames for the Player.

use std::time::Duration;

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use crate::app::{ColorMode, PlaybackStatus};

const FRAME_HEIGHT: usize = 8;
const FRAME_INTERVAL_MILLIS: u128 = 200;

#[derive(Clone, Copy)]
struct ArtLine {
    before_bell: &'static str,
    bell: &'static str,
    after_bell: &'static str,
}

impl ArtLine {
    const fn body(text: &'static str) -> Self {
        Self {
            before_bell: text,
            bell: "",
            after_bell: "",
        }
    }

    const fn bell(before_bell: &'static str, bell: &'static str, after_bell: &'static str) -> Self {
        Self {
            before_bell,
            bell,
            after_bell,
        }
    }
}

type Frame = [ArtLine; FRAME_HEIGHT];

const RESTING_FRAME: Frame = [
    ArtLine::body("       ╲       ╱"),
    ArtLine::body("        ╲ ▄▄▄ ╱"),
    ArtLine::body("        ▐▀• •▀▌"),
    ArtLine::body("      ▄▄█  ▴  █▄▄"),
    ArtLine::bell("     ╱  ▐█▄▄▄█▌  ╲──", "╮", ""),
    ArtLine::bell("        ▐█ █ █▌     ", "│", ""),
    ArtLine::bell("       ▄▀  █  ▀▄   ", "▟█▙", ""),
    ArtLine::bell("      ▀   ▀ ▀   ▀   ", "▀", ""),
];

const DANCE_LEFT: Frame = [
    ArtLine::body("     ╲           ╱"),
    ArtLine::body("      ╲   ▄▄▄   ╱"),
    ArtLine::body("       ╲ ▐▀^ ^▀▌"),
    ArtLine::body("    ╲   ▄█  ▴  █▄"),
    ArtLine::bell("     ╲▄▀▐█▄▄▄█▌ ▀──", "╮", ""),
    ArtLine::bell("        ▐█ █ █▌    ", "│", ""),
    ArtLine::bell("      ▄▀  █  █ ▀▄ ", "▟█▙", ""),
    ArtLine::bell("     ▀   ▀    ▀    ", "▀", ""),
];

const DANCE_UP: Frame = [
    ArtLine::body("        ╲   ╱"),
    ArtLine::body("     ╲   ╲▄╱   ╱"),
    ArtLine::body("      ╲ ▐▀^ ^▀▌ ╱"),
    ArtLine::body("       ╲█  ▴  █╱"),
    ArtLine::bell("        ▐█▄▄▄█▌───", "╮", ""),
    ArtLine::bell("       ▄▀█ █ █▀▄  ", "│", ""),
    ArtLine::bell("     ▄▀  ▀   ▀  ▀▄", "▟█▙", ""),
    ArtLine::bell("    *             * ", "▀", ""),
];

const DANCE_RIGHT: Frame = [
    ArtLine::body("       ╲           ╱"),
    ArtLine::body("        ╲   ▄▄▄   ╱"),
    ArtLine::body("         ▐▀^ ^▀▌ ╱"),
    ArtLine::body("         ▄█  ▴  █▄   ╱"),
    ArtLine::bell("     ", "╭", "──▀ ▐█▄▄▄█▌▀▄╱"),
    ArtLine::bell("     ", "│", "    ▐█ █ █▌"),
    ArtLine::bell("    ", "▟█▙", " ▄▀ █  █  ▀▄"),
    ArtLine::bell("     ", "▀", "     ▀    ▀   ▀"),
];

const DANCE_DOWN: Frame = [
    ArtLine::body("        ╲       ╱"),
    ArtLine::body("         ╲ ▄▄▄ ╱"),
    ArtLine::body("         ▐▀• •▀▌"),
    ArtLine::bell("     ▄▄▄▄█  ▿  █▄▄▄▄──", "╮", ""),
    ArtLine::bell("    ╱    ▐█▄▄▄█▌      ", "│", ""),
    ArtLine::bell("         ▐█ █ █▌     ", "▟█▙", ""),
    ArtLine::bell("      ▄▄▀  █ █  ▀▄▄   ", "▀", ""),
    ArtLine::body("     ▀    ▀   ▀    ▀"),
];

pub(super) fn lines(
    status: PlaybackStatus,
    elapsed: Duration,
    color_mode: ColorMode,
) -> Vec<Line<'static>> {
    let frame = frame(status, elapsed);
    let body_style = if color_mode == ColorMode::Terminal {
        Style::default().fg(Color::Green)
    } else {
        Style::default()
    };
    let bell_style = if color_mode == ColorMode::Terminal {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };
    frame
        .iter()
        .map(|line| {
            Line::from(vec![
                Span::styled(line.before_bell, body_style),
                Span::styled(line.bell, bell_style),
                Span::styled(line.after_bell, body_style),
            ])
        })
        .collect()
}

fn frame(status: PlaybackStatus, elapsed: Duration) -> &'static Frame {
    if status != PlaybackStatus::Playing {
        return &RESTING_FRAME;
    }
    match (elapsed.as_millis() / FRAME_INTERVAL_MILLIS) % 4 {
        0 => &DANCE_LEFT,
        1 => &DANCE_UP,
        2 => &DANCE_RIGHT,
        3 => &DANCE_DOWN,
        _ => unreachable!("four-frame animation phase is reduced modulo four"),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DANCE_DOWN, DANCE_LEFT, DANCE_RIGHT, DANCE_UP, FRAME_HEIGHT, Frame, RESTING_FRAME, frame,
        lines,
    };
    use crate::app::{ColorMode, PlaybackStatus};
    use ratatui::style::{Color, Style};
    use std::time::Duration;

    fn plain(frame: &Frame) -> String {
        frame
            .iter()
            .map(|line| format!("{}{}{}", line.before_bell, line.bell, line.after_bell))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn non_playing_states_always_return_to_the_resting_frame() {
        let resting = plain(&RESTING_FRAME);
        for status in [
            PlaybackStatus::Stopped,
            PlaybackStatus::Loading,
            PlaybackStatus::Paused,
            PlaybackStatus::Error,
        ] {
            assert_eq!(plain(frame(status, Duration::from_secs(913))), resting);
        }
    }

    #[test]
    fn playing_advances_at_the_fixed_interval_and_loops() {
        assert_eq!(
            plain(frame(PlaybackStatus::Playing, Duration::ZERO)),
            plain(&DANCE_LEFT)
        );
        assert_eq!(
            plain(frame(PlaybackStatus::Playing, Duration::from_millis(200))),
            plain(&DANCE_UP)
        );
        assert_eq!(
            plain(frame(PlaybackStatus::Playing, Duration::from_millis(400))),
            plain(&DANCE_RIGHT)
        );
        assert_eq!(
            plain(frame(PlaybackStatus::Playing, Duration::from_millis(600))),
            plain(&DANCE_DOWN)
        );
        assert_eq!(
            plain(frame(PlaybackStatus::Playing, Duration::from_millis(800))),
            plain(&DANCE_LEFT)
        );
    }

    #[test]
    fn every_frame_stays_inside_the_supported_player() {
        for frame in [
            &RESTING_FRAME,
            &DANCE_LEFT,
            &DANCE_UP,
            &DANCE_RIGHT,
            &DANCE_DOWN,
        ] {
            assert_eq!(frame.len(), FRAME_HEIGHT);
            for line in frame {
                let width = line.before_bell.chars().count()
                    + line.bell.chars().count()
                    + line.after_bell.chars().count();
                assert!(width <= 30, "{width} cells: {}", plain(frame));
            }
        }
    }

    #[test]
    fn colors_follow_the_terminal_or_leave_the_art_unstyled() {
        let colored = lines(PlaybackStatus::Stopped, Duration::ZERO, ColorMode::Terminal);
        assert!(
            colored
                .iter()
                .flat_map(|line| &line.spans)
                .all(|span| { matches!(span.style.fg, Some(Color::Green | Color::Yellow)) })
        );
        assert!(
            colored
                .iter()
                .flat_map(|line| &line.spans)
                .any(|span| { !span.content.is_empty() && span.style.fg == Some(Color::Yellow) })
        );

        let mono = lines(PlaybackStatus::Stopped, Duration::ZERO, ColorMode::Mono);
        assert!(
            mono.iter()
                .flat_map(|line| &line.spans)
                .all(|span| span.style == Style::default())
        );
    }

    #[test]
    fn suzu_dance_cycle_is_stable() {
        let sheet = [
            ("RESTING", &RESTING_FRAME),
            ("DANCE LEFT", &DANCE_LEFT),
            ("DANCE UP", &DANCE_UP),
            ("DANCE RIGHT", &DANCE_RIGHT),
            ("DANCE DOWN", &DANCE_DOWN),
        ]
        .into_iter()
        .map(|(name, frame)| format!("{name}\n{}", plain(frame)))
        .collect::<Vec<_>>()
        .join("\n\n");

        insta::assert_snapshot!("suzu_dance_cycle", sheet);
    }
}
