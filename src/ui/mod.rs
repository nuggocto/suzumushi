// SPDX-License-Identifier: Apache-2.0

//! Calm library, player, and queue interface with visible keyboard focus.

pub mod layout;
pub mod status;

use std::os::unix::ffi::OsStrExt;

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph, Wrap};

use crate::app::{AppState, BrowserRow, ColorMode, Focus, PlaybackStatus, normal_component};
use crate::display::{bounded_text, terminal_safe};

const MIN_WIDTH: u16 = 80;
const MIN_HEIGHT: u16 = 24;
const MAX_ROW_TEXT_BYTES: usize = 4_096;

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
    render_library(frame, panels.library, app);
    render_player(frame, panels.player, app);
    render_queue(frame, panels.queue, app);
    status::render(frame, panels.status, app);
}

fn render_library(frame: &mut Frame<'_>, area: Rect, app: &AppState) {
    let block = panel_block("Library", Focus::Library, app);
    let row_count = app.library_row_count();
    if row_count == 0 {
        let message = if app.search.active {
            "No matches"
        } else {
            "Library is empty"
        };
        frame.render_widget(Paragraph::new(message).block(block), area);
        return;
    }

    let visible = usize::from(area.height.saturating_sub(2)).max(1);
    let max_row_bytes = row_text_max_bytes(area);
    let offset = centered_offset(app.library_selection, row_count, visible);
    let end = offset.saturating_add(visible).min(row_count);
    let items: Vec<_> = (offset..end)
        .filter_map(|position| app.library_row(position))
        .map(|row| ListItem::new(library_row_text(app, row, max_row_bytes)))
        .collect();
    let mut state = ListState::default();
    state.select(Some(app.library_selection.saturating_sub(offset)));
    let list = List::new(items)
        .block(block)
        .highlight_style(selection_style(app, Focus::Library));
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_player(frame: &mut Frame<'_>, area: Rect, app: &AppState) {
    let max_bytes = row_text_max_bytes(area);
    let title = app.player_title(max_bytes);
    let creator = app.player_creator(max_bytes);
    let state = match app.playback_status {
        PlaybackStatus::Stopped => "Stopped",
        PlaybackStatus::Loading => "Loading",
        PlaybackStatus::Playing => "Playing",
        PlaybackStatus::Paused => "Paused",
        PlaybackStatus::Error => "Error",
    };
    let position = app.playback_position();
    let duration = app.playback_duration();
    let timeline = player_timeline(
        position,
        duration,
        usize::from(area.width.saturating_sub(2)),
    );
    let volume = if app.muted {
        format!("Muted ({}%)", app.volume_percent)
    } else {
        format!("Volume {}%", app.volume_percent)
    };
    let modes = format!(
        "Shuffle {}  Repeat {}",
        if app.shuffle { "on" } else { "off" },
        app.repeat.label(),
    );
    let format = app.playback_format().map_or_else(String::new, |format| {
        format!("\n{} Hz  {} ch", format.sample_rate, format.channels)
    });
    let creator = if creator.is_empty() {
        String::new()
    } else {
        format!("\n{creator}")
    };
    let content =
        format!("{title}{creator}\n\n{timeline}\n\nState: {state}\n{volume}\n{modes}{format}");
    frame.render_widget(
        Paragraph::new(content)
            .alignment(Alignment::Center)
            .block(panel_block("Player", Focus::Player, app)),
        area,
    );
}

fn player_timeline(
    position: std::time::Duration,
    duration: Option<std::time::Duration>,
    width: usize,
) -> String {
    let position_text = format_time(position);
    let duration_text = duration.map_or_else(|| "--:--".into(), format_time);
    let times = format!("{position_text} / {duration_text}");
    let bar_width = width
        .saturating_sub(times.len().saturating_add(4))
        .clamp(4, 28);
    format!("{}  {times}", progress_bar(position, duration, bar_width))
}

fn format_time(duration: std::time::Duration) -> String {
    let seconds = duration.as_secs();
    let minutes = seconds / 60;
    let seconds = seconds % 60;
    format!("{minutes}:{seconds:02}")
}

fn progress_bar(
    position: std::time::Duration,
    duration: Option<std::time::Duration>,
    width: usize,
) -> String {
    let filled = duration.map_or(0, |duration| {
        if duration.is_zero() {
            0
        } else {
            let numerator = position.as_millis().min(duration.as_millis());
            usize::try_from(numerator.saturating_mul(width as u128) / duration.as_millis())
                .unwrap_or(width)
                .min(width)
        }
    });
    format!("[{}{}]", "=".repeat(filled), "-".repeat(width - filled))
}

fn render_queue(frame: &mut Frame<'_>, area: Rect, app: &AppState) {
    let block = panel_block("Queue", Focus::Queue, app);
    if app.queue.is_empty() {
        frame.render_widget(Paragraph::new("Queue is empty").block(block), area);
        return;
    }

    let visible = usize::from(area.height.saturating_sub(2)).max(1);
    let max_row_bytes = row_text_max_bytes(area);
    let offset = centered_offset(app.queue_selection, app.queue.len(), visible);
    let end = offset.saturating_add(visible).min(app.queue.len());
    let items: Vec<_> = app.queue[offset..end]
        .iter()
        .enumerate()
        .map(|(visible_index, item)| {
            let position = offset + visible_index + 1;
            ListItem::new(format!(
                "{position}. {}",
                app.queue_item_title(*item, max_row_bytes)
            ))
        })
        .collect();
    let mut state = ListState::default();
    state.select(Some(app.queue_selection.saturating_sub(offset)));
    let list = List::new(items)
        .block(block)
        .highlight_style(selection_style(app, Focus::Queue));
    frame.render_stateful_widget(list, area, &mut state);
}

fn library_row_text(app: &AppState, row: BrowserRow, max_row_bytes: usize) -> String {
    match row {
        BrowserRow::LibraryRoot => "Library/".into(),
        BrowserRow::PlaylistsRoot => "Playlists/".into(),
        BrowserRow::Folder {
            entry_index,
            component_index,
            indent,
        } => {
            let name = app
                .entry(entry_index)
                .and_then(|entry| normal_component(&entry.display_path, component_index))
                .map_or_else(
                    || "Missing folder".into(),
                    |name| terminal_safe(name.as_bytes(), max_row_bytes),
                );
            format!("{}{name}/", indentation(indent))
        }
        BrowserRow::Playlist { playlist_index } => {
            app.index.playlists.get(playlist_index).map_or_else(
                || "  Missing playlist/".into(),
                |playlist| {
                    let name = bounded_text(&playlist.name, max_row_bytes);
                    format!("  {name}/")
                },
            )
        }
        BrowserRow::Track {
            entry_index,
            indent,
        } => format!(
            "{}- {}",
            indentation(indent),
            app.entry_title_bounded(entry_index, max_row_bytes)
        ),
    }
}

fn indentation(depth: usize) -> String {
    "  ".repeat(depth.min(16))
}

fn row_text_max_bytes(area: Rect) -> usize {
    usize::from(area.width.saturating_sub(2))
        .saturating_mul(4)
        .min(MAX_ROW_TEXT_BYTES)
}

fn centered_offset(selection: usize, len: usize, visible: usize) -> usize {
    selection
        .saturating_sub(visible / 2)
        .min(len.saturating_sub(visible))
}

fn panel_block<'a>(title: &'a str, focus: Focus, app: &AppState) -> Block<'a> {
    let selected = app.focus == focus;
    Block::default()
        .title(title)
        .title_alignment(Alignment::Center)
        .title_style(focus_style(app, selected))
        .borders(Borders::ALL)
        .border_type(if selected {
            BorderType::Double
        } else {
            BorderType::Plain
        })
}

fn focus_style(app: &AppState, selected: bool) -> Style {
    if !selected {
        return Style::default();
    }
    let style = Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED);
    if app.color_mode == ColorMode::Terminal {
        style.fg(Color::Cyan)
    } else {
        style
    }
}

fn selection_style(app: &AppState, focus: Focus) -> Style {
    focus_style(app, app.focus == focus)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Position;

    use crate::app::AppState;
    use crate::config::Config;
    use crate::model::{ScanCounters, ScanIndex};

    fn empty_index() -> ScanIndex {
        ScanIndex {
            generation: 1,
            complete: true,
            assets: Vec::new(),
            entries: Vec::new(),
            playlists: Vec::new(),
            warnings: Vec::new(),
            counters: ScanCounters::default(),
        }
    }

    fn snapshot(width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let app = AppState::new(&Config::default(), empty_index()).expect("app state");
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

    #[test]
    fn supported_width_keeps_long_elapsed_and_duration_text_visible() {
        let timeline =
            super::player_timeline(Duration::from_mins(10), Some(Duration::from_mins(20)), 38);

        assert!(timeline.len() <= 38, "{timeline:?}");
        assert!(timeline.ends_with("10:00 / 20:00"), "{timeline:?}");
    }
}
