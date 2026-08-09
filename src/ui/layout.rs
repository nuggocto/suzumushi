// SPDX-License-Identifier: Apache-2.0

//! Stable panel geometry for supported terminal sizes.

use ratatui::layout::{Constraint, Direction, Layout, Rect};

/// Rectangles for the four panels and one bottom status line.
pub struct PanelLayout {
    pub library: Rect,
    pub now_playing: Rect,
    pub queue: Rect,
    pub art: Rect,
    pub status: Rect,
}

#[must_use]
pub fn panels(area: Rect) -> PanelLayout {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(30),
            Constraint::Percentage(40),
            Constraint::Percentage(30),
        ])
        .split(vertical[0]);
    let middle = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(5), Constraint::Min(1)])
        .split(columns[1]);
    PanelLayout {
        library: columns[0],
        now_playing: middle[0],
        queue: columns[2],
        art: middle[1],
        status: vertical[1],
    }
}
