// SPDX-License-Identifier: Apache-2.0

//! Stable panel geometry for supported terminal sizes.

use ratatui::layout::{Constraint, Direction, Layout, Rect};

/// Rectangles for the three panels and one bottom status line.
pub struct PanelLayout {
    pub library: Rect,
    pub player: Rect,
    pub queue: Rect,
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
    PanelLayout {
        library: columns[0],
        player: columns[1],
        queue: columns[2],
        status: vertical[1],
    }
}
