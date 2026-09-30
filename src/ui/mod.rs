// SPDX-License-Identifier: Apache-2.0

//! Calm library, player, and queue interface with visible keyboard focus.

pub mod layout;
mod mascot;
mod palette;
mod rail;
mod stage;
pub mod status;

pub use palette::ColorDepth;
pub(crate) use stage::STAGE_FIELD_BYTES;
pub use stage::Stage;

use std::os::unix::ffi::OsStrExt;
use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};

use crate::app::{AppState, BrowserRow, ColorMode, Focus, PlaybackStatus, normal_component};
use crate::config::STAGE_MAX_ROWS;
use crate::display::{bounded_text, terminal_safe};

const MIN_WIDTH: u16 = 80;
const MIN_HEIGHT: u16 = 24;
const MAX_ROW_TEXT_BYTES: usize = 4_096;

/// Draws the whole interface; the Player also advances the stage animation.
pub fn render(frame: &mut Frame<'_>, app: &AppState, stage: &mut Stage, animation_time: Duration) {
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
    render_player(frame, panels.player, app, stage, animation_time);
    render_queue(frame, panels.queue, app);
    status::render(frame, panels.status, app);
}

fn render_library(frame: &mut Frame<'_>, area: Rect, app: &AppState) {
    let block = panel_block("Library", Focus::Library, app);
    let row_count = app.library_row_count();
    if app.search().active && !app.search().query.is_empty() && row_count == 0 {
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
    let offset = centered_offset(app.library_selection(), row_count, visible);
    let end = offset.saturating_add(visible).min(row_count);
    let items: Vec<_> = (offset..end)
        .filter_map(|position| app.library_row(position))
        .map(|row| ListItem::new(library_row_text(app, row, max_row_bytes)))
        .collect();
    let mut state = ListState::default();
    state.select(Some(app.library_selection().saturating_sub(offset)));
    let list = List::new(items)
        .block(block)
        .highlight_style(selection_style(app, Focus::Library));
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_player(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &AppState,
    stage: &mut Stage,
    animation_time: Duration,
) {
    let block = panel_block("Player", Focus::Player, app);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let max_bytes = row_text_max_bytes(area);
    let mut title = vec![Line::from(app.player_title(max_bytes))];
    let creator = app.player_creator(max_bytes);
    if !creator.is_empty() {
        title.push(Line::from(creator));
    }
    let footer = player_footer(app, stage.depth(), usize::from(inner.width), animation_time);

    // The stage fills the Player above the details: the meadow and title at
    // its foot, and any room above them as night sky. Only a Player taller
    // than the largest stage gets padding.
    let stage_rows = usize::from(inner.height)
        .saturating_sub(1 + footer.len())
        .clamp(title.len() + mascot::HEIGHT, STAGE_MAX_ROWS);
    let content = stage_rows + 1 + footer.len();
    let padding = usize::from(inner.height).saturating_sub(content) / 2;
    let [_, stage_area, _, footer_area] = Layout::vertical([
        Constraint::Length(to_rows(padding)),
        Constraint::Length(to_rows(stage_rows)),
        Constraint::Length(1),
        Constraint::Length(to_rows(footer.len())),
    ])
    .areas(inner);

    let suzu = mascot::lines(app.playback_status(), animation_time, app.color_mode);
    stage.render(
        frame.buffer_mut(),
        stage_area,
        &stage::StageFrame {
            status: app.playback_status(),
            levels: app.audio_spectrum_levels(),
            progress: track_progress(app.playback_position(), app.playback_duration()),
            color_mode: app.color_mode,
            title: &title,
            suzu: &suzu,
        },
        animation_time,
    );
    frame.render_widget(
        Paragraph::new(footer).alignment(Alignment::Center),
        footer_area,
    );
}

fn track_progress(position: Duration, duration: Option<Duration>) -> Option<f32> {
    duration
        .filter(|duration| !duration.is_zero())
        .map(|duration| (position.as_secs_f32() / duration.as_secs_f32()).clamp(0.0, 1.0))
}

fn player_footer(
    app: &AppState,
    depth: ColorDepth,
    width: usize,
    animation_time: Duration,
) -> Vec<Line<'static>> {
    let state = match app.playback_status() {
        PlaybackStatus::Stopped => "Stopped",
        PlaybackStatus::Loading => "Loading",
        PlaybackStatus::Playing => "Playing",
        PlaybackStatus::Paused => "Paused",
        PlaybackStatus::Error => "Error",
    };
    let volume = if app.muted {
        format!("Muted ({}%)", app.volume_percent)
    } else {
        format!("Volume {}%", app.volume_percent)
    };
    let mut footer = vec![
        player_timeline(
            app.playback_position(),
            app.playback_duration(),
            width,
            (app.color_mode == ColorMode::Terminal).then_some(depth),
            app.playback_status(),
            animation_time,
        ),
        Line::from(vec![
            Span::styled(
                format!("State: {state}"),
                playback_state_style(app, app.playback_status()),
            ),
            Span::raw(format!("   {volume}")),
        ]),
        Line::from(format!(
            "Shuffle {}  Repeat {}",
            if app.shuffle_enabled() { "on" } else { "off" },
            app.repeat.label(),
        )),
    ];
    if let Some(format) = app.playback_format() {
        footer.push(Line::from(format!(
            "{} Hz  {} ch",
            format.sample_rate, format.channels
        )));
    }
    footer
}

fn to_rows(count: usize) -> u16 {
    u16::try_from(count).unwrap_or(u16::MAX)
}

/// The track's progress as a firefly's path through the dew, then the time:
/// elapsed in the light's color, the total plain.
fn player_timeline(
    position: Duration,
    duration: Option<Duration>,
    width: usize,
    colors: Option<ColorDepth>,
    status: PlaybackStatus,
    animation_time: Duration,
) -> Line<'static> {
    let elapsed = format_time(position);
    let total = duration.map_or_else(|| "--:--".into(), format_time);
    let times_width = elapsed.len() + 3 + total.len();
    let rail_cells = width
        .saturating_sub(times_width.saturating_add(4))
        .clamp(4, rail::MAX_RAIL_CELLS);
    let progress = track_progress(position, duration);
    let mut spans = rail::spans(
        rail_cells,
        progress,
        status,
        animation_time.as_secs_f32(),
        colors,
    );
    spans.push(Span::raw("  "));
    match (colors, progress) {
        (Some(depth), Some(progress)) => {
            spans.push(Span::styled(
                elapsed,
                Style::default().fg(depth.color(palette::pastel(progress))),
            ));
            spans.push(Span::styled(" / ", Style::default().fg(Color::DarkGray)));
        }
        _ => spans.push(Span::raw(format!("{elapsed} / "))),
    }
    spans.push(Span::raw(total));
    Line::from(spans)
}

fn format_time(duration: Duration) -> String {
    let seconds = duration.as_secs();
    let minutes = seconds / 60;
    let seconds = seconds % 60;
    format!("{minutes}:{seconds:02}")
}

fn render_queue(frame: &mut Frame<'_>, area: Rect, app: &AppState) {
    let block = panel_block("Queue", Focus::Queue, app);
    if app.queue().is_empty() {
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
    let offset = centered_offset(app.queue_selection(), app.queue().len(), visible);
    let end = offset.saturating_add(visible).min(app.queue().len());
    let items: Vec<_> = app.queue()[offset..end]
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
    state.select(Some(app.queue_selection().saturating_sub(offset)));
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
            .draw(|frame| {
                let mut stage = super::Stage::new(super::ColorDepth::TrueColor);
                super::render(frame, app, &mut stage, animation_time);
            })
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
            .draw(|frame| {
                let mut stage = super::Stage::new(super::ColorDepth::TrueColor);
                super::render(frame, &app, &mut stage, Duration::ZERO);
            })
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

        app.set_playback_status(crate::app::PlaybackStatus::Playing);
        let dancing = rendered(&app, 80, 24, Duration::from_millis(200));
        assert!(dancing.contains("▐▀^ ^▀▌"), "{dancing}");
        assert!(!dancing.contains("▐▀• •▀▌"), "{dancing}");

        app.set_playback_status(crate::app::PlaybackStatus::Paused);
        let paused = rendered(&app, 80, 24, Duration::from_millis(200));
        assert!(paused.contains("▐▀• •▀▌"), "{paused}");
        assert!(!paused.contains("▐▀^ ^▀▌"), "{paused}");
    }

    #[test]
    fn the_meadow_grows_with_the_music_only_while_playing() {
        // Rows above Suzu's head hold only tall stalks and their lights.
        let sky = |app: &AppState| -> usize {
            let backend = TestBackend::new(80, 24);
            let mut terminal = Terminal::new(backend).expect("test terminal");
            let mut stage = super::Stage::new(super::ColorDepth::TrueColor);
            for step in 0..30 {
                terminal
                    .draw(|frame| {
                        super::render(frame, app, &mut stage, Duration::from_millis(step * 33));
                    })
                    .expect("draw UI");
            }
            let buffer = terminal.backend().buffer();
            (3..8)
                .flat_map(|y| (21..59).map(move |x| (x, y)))
                .filter(|position| buffer[*position].symbol() != " ")
                .count()
        };
        let mut app = AppState::new(&Config::default(), empty_index()).expect("app state");
        app.audio_spectrum(AudioSpectrum::new([230; crate::audio::SPECTRUM_BANDS]));

        app.set_playback_status(crate::app::PlaybackStatus::Stopped);
        assert_eq!(sky(&app), 0, "a stopped meadow rests on the ground");
        app.set_playback_status(crate::app::PlaybackStatus::Playing);
        assert!(sky(&app) > 20, "loud music grows tall stalks");
    }

    #[test]
    fn loading_warning_and_error_states_are_named_in_text() {
        let mut app = AppState::new(&Config::default(), empty_index()).expect("app state");
        app.set_playback_status(crate::app::PlaybackStatus::Loading);
        let loading = rendered(&app, 80, 24, Duration::ZERO);
        assert!(loading.contains("State: Loading"), "{loading}");

        app.set_playback_status(crate::app::PlaybackStatus::Error);
        app.status_kind = crate::app::StatusKind::Error;
        app.status_message = "Output device unavailable".into();
        let error = rendered(&app, 80, 24, Duration::ZERO);
        assert!(error.contains("State: Error"), "{error}");
        assert!(error.contains("Error: Output device"), "{error}");

        app.set_playback_status(crate::app::PlaybackStatus::Stopped);
        app.status_kind = crate::app::StatusKind::Warning;
        app.status_message = "Scan incomplete".into();
        let warning = rendered(&app, 80, 24, Duration::ZERO);
        assert!(warning.contains("Warning: Scan"), "{warning}");
    }

    #[test]
    fn supported_width_keeps_long_elapsed_and_duration_text_visible() {
        let timeline = super::player_timeline(
            Duration::from_mins(10),
            Some(Duration::from_mins(20)),
            38,
            None,
            crate::app::PlaybackStatus::Playing,
            Duration::ZERO,
        );
        let text: String = timeline
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();

        assert!(timeline.width() <= 38, "{text:?}");
        assert!(text.ends_with("10:00 / 20:00"), "{text:?}");
    }
}
