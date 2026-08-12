// SPDX-License-Identifier: Apache-2.0

//! Compact keyboard help and structured status rendering.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::{AppState, ColorMode, StatusKind};
use crate::display::bounded_text;

pub fn render(frame: &mut Frame<'_>, area: Rect, app: &AppState) {
    let lines = if app.search.active {
        let query_bytes = usize::from(area.width.saturating_sub(10)).saturating_mul(4);
        vec![
            Line::from(vec![
                Span::raw(" Search: "),
                Span::raw(bounded_text(&app.search.query, query_bytes)),
            ]),
            Line::from(vec![
                Span::raw(" Enter add · Esc close · Ctrl+c quit  "),
                notice_span(app),
            ]),
        ]
    } else {
        let keys = match app.focus {
            crate::app::Focus::Library => " Tab focus · / search · ↑/↓ move · Enter open/add  ",
            crate::app::Focus::Player => {
                " Tab focus · ←/→ seek · -/+ volume · m mute · Enter play/pause  "
            }
            crate::app::Focus::Queue => {
                " Tab focus · ↑/↓ select · J/K reorder · d remove · c clear  "
            }
        };
        let playback_keys = match app.focus {
            crate::app::Focus::Library => {
                "q · ? · Space play/pause · s stop · p previous · n next · x shuffle · r repeat"
            }
            crate::app::Focus::Player | crate::app::Focus::Queue => {
                "q · ? · Space/↵ play/pause · s stop · p previous · n next · x shuffle · r repeat"
            }
        };
        vec![
            Line::from(vec![Span::raw(keys), notice_span(app)]),
            Line::raw(playback_keys),
        ]
    };
    frame.render_widget(Paragraph::new(lines), area);
}

fn notice_span(app: &AppState) -> Span<'_> {
    let label = match app.status_kind {
        StatusKind::Info => "",
        StatusKind::Warning => "Warning: ",
        StatusKind::Error => "Error: ",
    };
    let style = match (app.color_mode, app.status_kind) {
        (ColorMode::Terminal, StatusKind::Warning) => Style::default().fg(Color::Yellow),
        (ColorMode::Terminal, StatusKind::Error) => Style::default().fg(Color::Red),
        (ColorMode::Mono, StatusKind::Warning | StatusKind::Error) => {
            Style::default().add_modifier(Modifier::BOLD)
        }
        (_, StatusKind::Info) => Style::default(),
    };
    Span::styled(format!("{label}{}", app.status_message), style)
}
