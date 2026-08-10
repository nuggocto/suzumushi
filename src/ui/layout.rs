// SPDX-License-Identifier: Apache-2.0

//! Stable panel geometry for supported terminal sizes.

use ratatui::layout::{Constraint, Direction, Layout, Rect};

/// Rectangles for the three panels and two bottom help/status lines.
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
        .constraints([Constraint::Min(1), Constraint::Length(2)])
        .split(area);
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(25),
            Constraint::Percentage(50),
            Constraint::Percentage(25),
        ])
        .split(vertical[0]);
    PanelLayout {
        library: columns[0],
        player: columns[1],
        queue: columns[2],
        status: vertical[1],
    }
}

#[cfg(test)]
mod tests {
    use super::{Rect, panels};

    #[test]
    fn player_receives_half_the_supported_width() {
        let layout = panels(Rect::new(0, 0, 80, 24));

        assert_eq!(layout.library.width, 20);
        assert_eq!(layout.player.width, 40);
        assert_eq!(layout.queue.width, 20);
    }
}
