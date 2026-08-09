// SPDX-License-Identifier: Apache-2.0

//! Calm placeholder interface with focus visible without color.

pub mod layout;
pub mod status;

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};

use crate::app::{AppState, ColorMode, Focus};

const MIN_WIDTH: u16 = 80;
const MIN_HEIGHT: u16 = 24;

pub fn render(frame: &mut Frame<'_>, app: &AppState) {
    let area = frame.area();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        frame.render_widget(
            Paragraph::new(
                "Suzumushi needs at least 80x24.\nResize the terminal or press q to quit.",
            )
            .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }

    let panels = layout::panels(area);
    panel(frame, panels.library, "Library", Focus::Library, app);
    panel(
        frame,
        panels.now_playing,
        "Now Playing",
        Focus::NowPlaying,
        app,
    );
    panel(frame, panels.queue, "Queue", Focus::Queue, app);
    panel(frame, panels.art, "Art", Focus::Art, app);
    status::render(frame, panels.status, app);
}

fn panel(frame: &mut Frame<'_>, area: Rect, title: &str, focus: Focus, app: &AppState) {
    let selected = app.focus == focus;
    let mut style = Style::default();
    if selected {
        style = style.add_modifier(Modifier::BOLD | Modifier::REVERSED);
        if app.color_mode == ColorMode::Terminal {
            style = style.fg(Color::Cyan);
        }
    }
    let block = Block::default()
        .title(title)
        .title_alignment(Alignment::Center)
        .title_style(style)
        .borders(Borders::ALL)
        .border_type(if selected {
            BorderType::Double
        } else {
            BorderType::Plain
        });
    frame.render_widget(Paragraph::new("Placeholder").block(block), area);
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Position;

    use crate::app::AppState;
    use crate::config::Config;

    fn snapshot(width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let app = AppState::new(&Config::default());
        terminal
            .draw(|frame| super::render(frame, &app))
            .expect("draw UI");
        let buffer = terminal.backend().buffer();
        let mut output = String::new();
        for y in 0..height {
            for x in 0..width {
                let cell = buffer
                    .cell(Position::new(x, y))
                    .expect("snapshot cell must exist");
                output.push_str(cell.symbol());
            }
            while output.ends_with(' ') {
                output.pop();
            }
            output.push('\n');
        }
        output
    }

    #[test]
    fn supported_terminal_layouts_are_stable() {
        insta::assert_snapshot!("terminal_80x24", snapshot(80, 24));
        insta::assert_snapshot!("terminal_120x32", snapshot(120, 32));
    }

    #[test]
    fn undersized_terminal_has_a_keyboard_accessible_fallback() {
        insta::assert_snapshot!("terminal_small", snapshot(40, 10));
    }
}
