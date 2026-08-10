// SPDX-License-Identifier: Apache-2.0

//! Compact keyboard help and status rendering.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::AppState;

pub fn render(frame: &mut Frame<'_>, area: Rect, app: &AppState) {
    let lines = if app.search.active {
        vec![Line::from(vec![
            Span::raw(" Search: "),
            Span::raw(&app.search.query),
            Span::raw("  Enter add  Esc close  "),
            Span::raw(&app.status_message),
        ])]
    } else {
        let keys = match app.focus {
            crate::app::Focus::Library => " / search  Up/Down move  Enter add/play  ",
            crate::app::Focus::Player => " Left/Right seek  - or + volume  m mute  ",
            crate::app::Focus::Queue => " Up/Down select  J/K reorder  d remove  c clear  ",
        };
        vec![
            Line::from(vec![
                Span::raw(" q quit  Tab focus  "),
                Span::raw(keys),
                Span::raw(&app.status_message),
            ]),
            Line::raw(" Space play/pause  s stop  n next  p previous  x shuffle  r repeat"),
        ]
    };
    frame.render_widget(Paragraph::new(lines), area);
}
