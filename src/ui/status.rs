// SPDX-License-Identifier: Apache-2.0

//! Compact keyboard help and status rendering.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::AppState;

pub fn render(frame: &mut Frame<'_>, area: Rect, app: &AppState) {
    let line = Line::from(vec![
        Span::raw(" q quit  Tab/Shift+Tab focus  Space action  "),
        Span::raw(app.status_message),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}
