// SPDX-License-Identifier: Apache-2.0

//! Calm library, player, and queue interface with visible keyboard focus.

pub mod layout;
mod mascot;
pub mod status;

use std::os::unix::ffi::OsStrExt;
use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};

use crate::app::{AppState, BrowserRow, ColorMode, Focus, PlaybackStatus, normal_component};
use crate::display::{bounded_text, terminal_safe};

const MIN_WIDTH: u16 = 80;
const MIN_HEIGHT: u16 = 24;
const MAX_ROW_TEXT_BYTES: usize = 4_096;

pub fn render(frame: &mut Frame<'_>, app: &AppState, animation_time: Duration) {
    let area = frame.area();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        frame.render_widget(
            Paragraph::new(format!(
                "Suzumushi needs at least 80x24.\nCurrent size: {}x{}.\nResize the terminal or press q to quit.",
                area.width, area.height
            ))
            .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    if app.help_visible() {
        render_help(frame, area, app);
        return;
    }

    let panels = layout::panels(area);
    render_library(frame, panels.library, app);
    render_player(frame, panels.player, app, animation_time);
    render_queue(frame, panels.queue, app);
    status::render(frame, panels.status, app);
}

fn render_library(frame: &mut Frame<'_>, area: Rect, app: &AppState) {
    let block = panel_block("Library", Focus::Library, app);
    let row_count = app.library_row_count();
    if app.search.active && !app.search.query.is_empty() && row_count == 0 {
        frame.render_widget(
            Paragraph::new("No matches\nTry artist, title, filename, or path.")
                .wrap(Wrap { trim: false })
                .block(block),
            area,
        );
        return;
    }
    if app.index.entries.is_empty() {
        frame.render_widget(
            Paragraph::new("Library is empty\nAdd audio below audio/library/.\nPress ? for help.")
                .wrap(Wrap { trim: false })
                .block(block),
            area,
        );
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

fn render_player(frame: &mut Frame<'_>, area: Rect, app: &AppState, animation_time: Duration) {
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
    let mut content = Vec::with_capacity(18);
    content.push(Line::from(title));
    if !creator.is_empty() {
        content.push(Line::from(creator));
    }
    content.push(Line::default());
    content.extend(mascot::lines(
        app.playback_status,
        animation_time,
        app.color_mode,
    ));
    content.push(spectrum_line(app));
    content.push(Line::from(timeline));
    content.push(Line::default());
    content.push(Line::styled(
        format!("State: {state}"),
        playback_state_style(app, app.playback_status),
    ));
    content.push(Line::from(volume));
    content.push(Line::from(modes));
    if let Some(format) = app.playback_format() {
        content.push(Line::from(format!(
            "{} Hz  {} ch",
            format.sample_rate, format.channels
        )));
    }
    let available_lines = usize::from(area.height.saturating_sub(2));
    let top_padding = available_lines.saturating_sub(content.len()) / 2;
    if top_padding > 0 {
        let mut centered = Vec::with_capacity(content.len() + top_padding);
        centered.resize_with(top_padding, Line::default);
        centered.append(&mut content);
        content = centered;
    }
    frame.render_widget(
        Paragraph::new(content)
            .alignment(Alignment::Center)
            .block(panel_block("Player", Focus::Player, app)),
        area,
    );
}

fn spectrum_line(app: &AppState) -> Line<'static> {
    const GLYPHS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    if app.playback_status != PlaybackStatus::Playing {
        return Line::default();
    }
    let levels = app.audio_spectrum_levels();
    let mut bars = String::with_capacity(levels.len().saturating_mul(4));
    for (index, level) in levels.into_iter().enumerate() {
        if index != 0 {
            bars.push(' ');
        }
        bars.push(GLYPHS[usize::from(level)]);
    }
    let style = if app.color_mode == ColorMode::Terminal {
        Style::default().fg(Color::Green)
    } else {
        Style::default()
    };
    Line::styled(bars, style)
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
        frame.render_widget(
            Paragraph::new("Queue is empty\nSelect a track and press Enter.")
                .wrap(Wrap { trim: false })
                .block(block),
            area,
        );
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

fn render_help(frame: &mut Frame<'_>, area: Rect, app: &AppState) {
    let heading = if app.color_mode == ColorMode::Terminal {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().add_modifier(Modifier::BOLD)
    };
    let lines = vec![
        Line::styled("Navigation", heading),
        Line::raw("  Tab or Shift+Tab     focus Library, Player, or Queue"),
        Line::raw("  Up or Down, j or k   move the selection"),
        Line::raw("  Home or End, g or G  jump to the first or last item"),
        Line::default(),
        Line::styled("Library and search", heading),
        Line::raw("  /                     search artist, title, filename, and path"),
        Line::raw("  Enter                 toggle folders; add tracks or playlists"),
        Line::raw("  Esc                   close search"),
        Line::default(),
        Line::styled("Queue", heading),
        Line::raw("  J or K                move the selected item down or up"),
        Line::raw("  d or Delete           remove item    c clear Queue"),
        Line::default(),
        Line::styled("Playback", heading),
        Line::raw("  Space or Enter outside Library  play or pause"),
        Line::raw("  s                     stop    p previous    n next"),
        Line::raw("  Left or Right          seek back or forward five seconds"),
        Line::raw("  - or +                 volume down or up    m mute"),
        Line::raw("  x                     shuffle    r repeat"),
        Line::default(),
        Line::raw("  ? or Esc               close help    q quit    Ctrl+c quit anywhere"),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title("Suzumushi Help")
                    .title_alignment(Alignment::Center)
                    .borders(Borders::ALL),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn library_row_text(app: &AppState, row: BrowserRow, max_row_bytes: usize) -> String {
    match row {
        BrowserRow::LibraryRoot => "Library/".into(),
        BrowserRow::PlaylistsRoot => "Playlists/".into(),
        BrowserRow::Folder {
            entry_index,
            component_index,
            indent,
            collapsed,
        } => {
            let name = app
                .entry(entry_index)
                .and_then(|entry| normal_component(&entry.display_path, component_index))
                .map_or_else(
                    || "Missing folder".into(),
                    |name| terminal_safe(name.as_bytes(), max_row_bytes),
                );
            let marker = if collapsed { "▸" } else { "▾" };
            format!("{}{marker} {name}/", indentation(indent))
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
    let marker = if selected { "> " } else { "" };
    let style = if selected {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    Block::default()
        .title(format!(" {marker}{title} "))
        .title_alignment(Alignment::Left)
        .title_style(style)
        .borders(Borders::ALL)
        .border_style(style)
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

fn playback_state_style(app: &AppState, status: PlaybackStatus) -> Style {
    let style = match status {
        PlaybackStatus::Loading | PlaybackStatus::Paused => Style::default().fg(Color::Yellow),
        PlaybackStatus::Playing => Style::default().fg(Color::Green),
        PlaybackStatus::Error => Style::default().fg(Color::Red),
        PlaybackStatus::Stopped => Style::default(),
    };
    if app.color_mode == ColorMode::Terminal {
        style
    } else if status == PlaybackStatus::Error {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Position;
    use ratatui::style::Modifier;

    use crate::app::AppState;
    use crate::audio::AudioSpectrum;
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

    fn rendered(app: &AppState, width: u16, height: u16, animation_time: Duration) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| super::render(frame, app, animation_time))
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

    fn snapshot(width: u16, height: u16) -> String {
        let app = AppState::new(&Config::default(), empty_index()).expect("app state");
        rendered(&app, width, height, Duration::ZERO)
    }

    #[test]
    fn supported_terminal_layouts_are_stable() {
        insta::assert_snapshot!("terminal_80x24", snapshot(80, 24));
        insta::assert_snapshot!("terminal_120x32", snapshot(120, 32));
    }

    #[test]
    fn focused_panel_uses_a_title_marker_and_uniform_border_shape() {
        let mut app = AppState::new(&Config::default(), empty_index()).expect("app state");

        let library = rendered(&app, 80, 24, Duration::ZERO);
        assert!(library.contains("┌ > Library "), "{library}");
        assert!(library.contains("┌ Player "), "{library}");
        assert!(!library.contains('╔'), "{library}");

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| super::render(frame, &app, Duration::ZERO))
            .expect("draw UI");
        let buffer = terminal.backend().buffer();
        assert!(
            buffer[(0, 0)].modifier.contains(Modifier::BOLD),
            "the focused Library border is bold"
        );
        assert!(
            !buffer[(20, 0)].modifier.contains(Modifier::BOLD),
            "the inactive Player border is not bold"
        );

        app.apply(crate::input::AppAction::FocusNext);
        let player = rendered(&app, 80, 24, Duration::ZERO);
        assert!(player.contains("┌ Library "), "{player}");
        assert!(player.contains("┌ > Player "), "{player}");
        assert!(!player.contains(" > Library "), "{player}");
    }

    #[test]
    fn help_is_complete_at_the_minimum_supported_size() {
        let mut app = AppState::new(&Config::default(), empty_index()).expect("app state");
        app.apply(crate::input::AppAction::ToggleHelp);

        insta::assert_snapshot!(
            "terminal_help_80x24",
            rendered(&app, 80, 24, Duration::ZERO)
        );
    }

    #[test]
    fn undersized_terminal_has_a_keyboard_accessible_fallback() {
        insta::assert_snapshot!("terminal_small", snapshot(40, 10));
    }

    #[test]
    fn player_animation_follows_playback_state() {
        let mut app = AppState::new(&Config::default(), empty_index()).expect("app state");
        let resting = rendered(&app, 80, 24, Duration::from_millis(200));
        assert!(resting.contains("▐▀• •▀▌"), "{resting}");

        app.playback_status = crate::app::PlaybackStatus::Playing;
        let dancing = rendered(&app, 80, 24, Duration::from_millis(200));
        assert!(dancing.contains("▐▀^ ^▀▌"), "{dancing}");
        assert!(!dancing.contains("▐▀• •▀▌"), "{dancing}");

        app.playback_status = crate::app::PlaybackStatus::Paused;
        let paused = rendered(&app, 80, 24, Duration::from_millis(200));
        assert!(paused.contains("▐▀• •▀▌"), "{paused}");
        assert!(!paused.contains("▐▀^ ^▀▌"), "{paused}");
    }

    #[test]
    fn player_spectrum_moves_only_while_audio_is_playing() {
        let mut app = AppState::new(&Config::default(), empty_index()).expect("app state");
        app.playback_status = crate::app::PlaybackStatus::Playing;
        app.audio_spectrum(AudioSpectrum::new([
            0, 1, 2, 3, 4, 5, 6, 7, 7, 6, 5, 4, 3, 2, 1, 0,
        ]));

        let playing = rendered(&app, 80, 24, Duration::ZERO);
        assert!(
            playing.contains("▁ ▂ ▃ ▄ ▅ ▆ ▇ █ █ ▇ ▆ ▅ ▄ ▃ ▂ ▁"),
            "{playing}"
        );

        app.playback_status = crate::app::PlaybackStatus::Paused;
        let paused = rendered(&app, 80, 24, Duration::ZERO);
        assert!(
            !paused.contains("▁ ▂ ▃ ▄ ▅ ▆ ▇ █ █ ▇ ▆ ▅ ▄ ▃ ▂ ▁"),
            "{paused}"
        );
    }

    #[test]
    fn loading_warning_and_error_states_are_named_in_text() {
        let mut app = AppState::new(&Config::default(), empty_index()).expect("app state");
        app.playback_status = crate::app::PlaybackStatus::Loading;
        let loading = rendered(&app, 80, 24, Duration::ZERO);
        assert!(loading.contains("State: Loading"), "{loading}");

        app.playback_status = crate::app::PlaybackStatus::Error;
        app.status_kind = crate::app::StatusKind::Error;
        app.status_message = "Output device unavailable".into();
        let error = rendered(&app, 80, 24, Duration::ZERO);
        assert!(error.contains("State: Error"), "{error}");
        assert!(error.contains("Error: Output device"), "{error}");

        app.playback_status = crate::app::PlaybackStatus::Stopped;
        app.status_kind = crate::app::StatusKind::Warning;
        app.status_message = "Scan incomplete".into();
        let warning = rendered(&app, 80, 24, Duration::ZERO);
        assert!(warning.contains("Warning: Scan"), "{warning}");
    }

    #[test]
    fn player_footer_names_enter_playback_and_separates_bindings() {
        let mut app = AppState::new(&Config::default(), empty_index()).expect("app state");
        app.apply(crate::input::AppAction::FocusNext);

        let player = rendered(&app, 80, 24, Duration::ZERO);

        assert!(player.contains("Enter play/pause"), "{player}");
        assert!(player.contains("p previous · n next"), "{player}");
        assert!(!player.contains("n/p skip"), "{player}");
        assert!(player.contains(" · "), "{player}");
    }

    #[test]
    fn supported_width_keeps_long_elapsed_and_duration_text_visible() {
        let timeline =
            super::player_timeline(Duration::from_mins(10), Some(Duration::from_mins(20)), 38);

        assert!(timeline.len() <= 38, "{timeline:?}");
        assert!(timeline.ends_with("10:00 / 20:00"), "{timeline:?}");
    }
}
