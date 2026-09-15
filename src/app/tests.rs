// SPDX-License-Identifier: Apache-2.0

use std::path::PathBuf;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::browser::{contains_ascii_case_insensitive, rebuild_ascii_case_prefix};
use super::{
    AppState, BrowserRow, ColorMode, Focus, PlaybackIntent, PlaybackStatus, RepeatMode, StatusKind,
    reserve_startup_open_files,
};
use crate::audio::{AudioEvent, AudioFormat, AudioPosition};
use crate::config::Config;
use crate::input::AppAction;
use crate::model::{
    FileIdentity, MediaAsset, Playlist, PlaylistId, ScanCounters, ScanIndex, SearchFields,
    TrackEntry, TrackEntryId, TrackEntrySource, TrackTags,
};

const STEREO_48_KHZ: AudioFormat = AudioFormat {
    sample_rate: 48_000,
    channels: 2,
};

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

fn fixture_index() -> ScanIndex {
    let playlist_id = PlaylistId(30);
    ScanIndex {
        generation: 7,
        complete: true,
        assets: fixture_assets(),
        entries: fixture_entries(playlist_id),
        playlists: vec![Playlist {
            id: playlist_id,
            name: "Favorites".into(),
            path: PathBuf::from("playlists/Favorites"),
            entry_count: 2,
        }],
        warnings: Vec::new(),
        counters: ScanCounters {
            encountered_entries: 7,
            ..ScanCounters::default()
        },
    }
}

fn fixture_index_with_distinct_context_assets() -> ScanIndex {
    let mut index = fixture_index();
    for entry_index in 2..4 {
        let mut asset = index.assets[entry_index - 2].clone();
        asset.file_identity.inode = 100 + entry_index as u64;
        index.entries[entry_index].asset_index = index.assets.len();
        index.assets.push(asset);
    }
    index
}

fn fixture_assets() -> Vec<MediaAsset> {
    let asset = |inode, artist: &str, album: &str, title: &str| MediaAsset {
        tags: TrackTags {
            artist: Some(artist.into()),
            album_artist: None,
            album: Some(album.into()),
            title: Some(title.into()),
        },
        file_identity: FileIdentity {
            device: 1,
            inode,
            size: 100,
            modified_seconds: 10,
            modified_nanoseconds: 0,
        },
    };
    vec![
        asset(1, "Calm Artist", "Campaign One", "Night Song"),
        asset(2, "Still Artist", "Quiet Album", "Quiet Song"),
    ]
}

fn fixture_entries(playlist_id: PlaylistId) -> Vec<TrackEntry> {
    let entry = |id, asset_index, path: &str, source| TrackEntry {
        id: TrackEntryId(id),
        asset_index,
        display_path: PathBuf::from(path),
        source,
        search: SearchFields {
            filename: PathBuf::from(path)
                .file_stem()
                .expect("fixture filename")
                .to_string_lossy()
                .into_owned(),
            relative_path: path.into(),
        },
    };
    vec![
        entry(
            10,
            0,
            "library/JDR/Campaign One/night.mp3",
            TrackEntrySource::LibraryFile,
        ),
        entry(
            20,
            1,
            "library/Music/quiet.flac",
            TrackEntrySource::LibraryFile,
        ),
        entry(
            40,
            0,
            "playlists/Favorites/night.mp3",
            TrackEntrySource::PlaylistCopy {
                playlist: playlist_id,
            },
        ),
        entry(
            50,
            1,
            "playlists/Favorites/quiet.flac",
            TrackEntrySource::PlaylistCopy {
                playlist: playlist_id,
            },
        ),
    ]
}

fn app_with_two_queued_tracks() -> AppState {
    let mut app = AppState::new(&Config::default(), fixture_index()).expect("app state");
    select_entry(&mut app, 0);
    app.apply(AppAction::Activate)
        .expect("idle Enter starts playback");
    select_entry(&mut app, 1);
    assert!(app.apply(AppAction::Activate).is_none());
    app
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn select_entry(app: &mut AppState, entry_index: usize) {
    app.browser.selection = (0..app.library_row_count())
        .position(|position| {
            matches!(
                app.library_row(position),
                Some(BrowserRow::Track {
                    entry_index: candidate,
                    ..
                }) if candidate == entry_index
            )
        })
        .expect("fixture entry has a visible browser row");
}

#[test]
fn focus_cycles_both_directions() {
    let mut app = AppState::new(&Config::default(), empty_index()).expect("app state");
    assert_eq!(app.focus, Focus::Library);
    app.apply(AppAction::FocusNext);
    assert_eq!(app.focus, Focus::Player);
    app.apply(AppAction::FocusNext);
    assert_eq!(app.focus, Focus::Queue);
    app.apply(AppAction::FocusNext);
    assert_eq!(app.focus, Focus::Library);
    app.apply(AppAction::FocusPrevious);
    assert_eq!(app.focus, Focus::Queue);
    app.apply(AppAction::FocusPrevious);
    assert_eq!(app.focus, Focus::Player);
    app.apply(AppAction::FocusPrevious);
    assert_eq!(app.focus, Focus::Library);
}

#[test]
fn help_is_modal_and_closes_without_triggering_hidden_actions() {
    let mut app = AppState::new(&Config::default(), empty_index()).expect("app state");

    app.apply(AppAction::ToggleHelp);
    assert!(app.help_visible());
    app.key(key(KeyCode::Char('x')), Duration::ZERO);
    assert!(
        !app.queue.shuffle,
        "hidden controls stay inactive while help is open"
    );

    app.key(key(KeyCode::Esc), Duration::ZERO);
    assert!(!app.help_visible());
    assert_eq!(app.status_kind, StatusKind::Info);
    assert_eq!(app.status_message, "Help closed");
}

#[test]
fn missing_creator_metadata_is_omitted() {
    let mut index = fixture_index();
    index.assets[0].tags.artist = None;
    index.assets[0].tags.album_artist = None;
    let mut app = AppState::new(&Config::default(), index).expect("app state");
    app.apply(AppAction::Activate)
        .expect("untagged fixture starts");

    assert_eq!(app.player_creator(80), "");
    assert_eq!(app.state_identity(80).1, "");
}

#[test]
fn mono_is_selected_without_relying_on_color_for_focus() {
    assert_eq!(
        ColorMode::resolve("terminal", false, false),
        ColorMode::Terminal
    );
    assert_eq!(ColorMode::resolve("mono", false, false), ColorMode::Mono);
    assert_eq!(ColorMode::resolve("terminal", true, false), ColorMode::Mono);
    assert_eq!(ColorMode::resolve("terminal", false, true), ColorMode::Mono);
}

#[test]
fn terminal_open_file_reservation_accepts_the_exact_peak() {
    assert!(reserve_startup_open_files(6).is_ok());
    assert!(reserve_startup_open_files(5).is_err());
}

#[test]
fn browser_rows_preserve_nested_folders_and_folder_playlists() {
    let app = AppState::new(&Config::default(), fixture_index()).expect("app state");

    assert!(matches!(app.browser.rows[0], BrowserRow::LibraryRoot));
    assert!(matches!(
        app.browser.rows[1],
        BrowserRow::Folder {
            entry_index: 0,
            component_index: 1,
            indent: 1,
            collapsed: false
        }
    ));
    assert!(matches!(
        app.browser.rows[2],
        BrowserRow::Folder {
            entry_index: 0,
            component_index: 2,
            indent: 2,
            collapsed: false
        }
    ));
    assert!(
        app.browser
            .rows
            .iter()
            .any(|row| matches!(row, BrowserRow::Playlist { playlist_index: 0 }))
    );
    assert_eq!(app.browser.selection, 3, "the first track starts selected");
}

#[test]
fn folder_activation_hides_descendants_without_hiding_search_results() {
    let mut app = AppState::new(&Config::default(), fixture_index()).expect("app state");
    let expanded_rows = app.library_row_count();
    app.browser.selection = (0..expanded_rows)
        .position(|position| {
            matches!(
                app.library_row(position),
                Some(BrowserRow::Folder {
                    entry_index: 0,
                    component_index: 1,
                    ..
                })
            )
        })
        .expect("fixture JDR folder is visible");

    app.apply(AppAction::Activate);

    assert_eq!(app.library_row_count(), expanded_rows - 2);
    assert_eq!(app.status_message, "Folder collapsed");

    app.apply(AppAction::SearchOpen);
    for character in "Night".chars() {
        app.key(key(KeyCode::Char(character)), Duration::ZERO);
    }
    assert!(
        (0..app.library_row_count()).any(|position| {
            matches!(
                app.library_row(position),
                Some(BrowserRow::Track { entry_index: 0, .. })
            )
        }),
        "search ignores temporary folder presentation state"
    );

    app.key(key(KeyCode::Esc), Duration::ZERO);
    assert_eq!(app.library_row_count(), expanded_rows - 2);
    app.apply(AppAction::Activate);
    assert_eq!(app.library_row_count(), expanded_rows);
    assert_eq!(app.status_message, "Folder expanded");
}

#[test]
fn nested_folder_collapse_survives_parent_toggle() {
    let mut app = AppState::new(&Config::default(), fixture_index()).expect("app state");
    let expanded_rows = app.library_row_count();
    app.browser.selection = (0..expanded_rows)
        .position(|position| {
            matches!(
                app.library_row(position),
                Some(BrowserRow::Folder {
                    entry_index: 0,
                    component_index: 2,
                    ..
                })
            )
        })
        .expect("fixture campaign folder is visible");
    app.apply(AppAction::Activate);
    assert_eq!(app.library_row_count(), expanded_rows - 1);

    app.browser.selection = (0..app.library_row_count())
        .position(|position| {
            matches!(
                app.library_row(position),
                Some(BrowserRow::Folder {
                    entry_index: 0,
                    component_index: 1,
                    ..
                })
            )
        })
        .expect("fixture JDR folder is visible");
    app.apply(AppAction::Activate);
    app.apply(AppAction::Activate);

    assert_eq!(app.library_row_count(), expanded_rows - 1);
    let campaign_position = (0..app.library_row_count())
        .position(|position| {
            matches!(
                app.library_row(position),
                Some(BrowserRow::Folder {
                    entry_index: 0,
                    component_index: 2,
                    collapsed: true,
                    ..
                })
            )
        })
        .expect("collapsed campaign folder is visible again");
    assert!(
        !(0..app.library_row_count()).any(|position| {
            matches!(
                app.library_row(position),
                Some(BrowserRow::Track { entry_index: 0, .. })
            )
        }),
        "expanding a parent preserves its child's collapsed state"
    );

    app.browser.selection = campaign_position;
    app.apply(AppAction::Activate);
    assert_eq!(app.library_row_count(), expanded_rows);
}

#[test]
fn browser_refuses_an_index_that_exceeds_its_recorded_scan_bound() {
    let mut index = fixture_index();
    index.counters.encountered_entries -= 1;

    let error = AppState::new(&Config::default(), index)
        .expect_err("browser must not grow beyond its reserved bound");

    assert!(error.to_string().contains("scanner-derived row bound"));
}

#[test]
fn search_is_bounded_and_matches_metadata() {
    let mut config = Config::default();
    config.search.max_results = 1;
    config.search.max_query_bytes = 8;
    let mut app = AppState::new(&config, fixture_index()).expect("app state");

    app.key(key(KeyCode::Char('/')), Duration::ZERO);
    for character in "ARTIST".chars() {
        app.key(key(KeyCode::Char(character)), Duration::ZERO);
    }
    assert_eq!(
        app.browser.search.results,
        [0],
        "result count is capped at one"
    );

    app.browser.search.query.clear();
    for character in "campaignX".chars() {
        app.key(key(KeyCode::Char(character)), Duration::ZERO);
    }
    assert_eq!(app.browser.search.query, "campaign");
    assert_eq!(app.status_message, "Search query limit reached");

    app.key(key(KeyCode::Esc), Duration::ZERO);
    assert!(!app.browser.search.active);
    assert!(app.browser.search.query.is_empty());
    assert!(app.browser.search.results.is_empty());
}

#[test]
fn search_independently_matches_filename_and_relative_path() {
    let mut index = fixture_index();
    for entry in &mut index.entries {
        entry.search.filename = "unrelated-file".into();
        entry.search.relative_path = "unrelated/path".into();
    }
    for asset in &mut index.assets {
        asset.tags = TrackTags {
            artist: Some("unrelated metadata".into()),
            album_artist: None,
            album: None,
            title: None,
        };
    }
    index.entries[0].search.filename = "filename-only-token".into();
    index.entries[1].search.relative_path = "folder/path-only-token.flac".into();
    let mut app = AppState::new(&Config::default(), index).expect("app state");

    app.key(key(KeyCode::Char('/')), Duration::ZERO);
    for character in "filename-only-token".chars() {
        app.key(key(KeyCode::Char(character)), Duration::ZERO);
    }
    assert_eq!(app.browser.search.results, [0]);

    app.key(
        KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
        Duration::ZERO,
    );
    for character in "path-only-token".chars() {
        app.key(key(KeyCode::Char(character)), Duration::ZERO);
    }
    assert_eq!(app.browser.search.results, [1]);
}

#[test]
fn q_is_search_text_while_ctrl_c_is_global() {
    let mut app = AppState::new(&Config::default(), fixture_index()).expect("app state");

    app.key(key(KeyCode::Char('/')), Duration::ZERO);
    app.key(key(KeyCode::Char('q')), Duration::ZERO);
    assert_eq!(app.browser.search.query, "q");
    assert!(!app.should_quit);

    app.key(
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        Duration::ZERO,
    );
    assert!(app.should_quit);
}

#[test]
fn ascii_case_search_stays_linear_on_repetitive_near_matches() {
    let haystack = vec![b'a'; 16_384];
    let mut needle = vec![b'a'; 1_024];
    *needle.last_mut().expect("non-empty needle") = b'b';
    let mut prefix_table = Vec::new();
    rebuild_ascii_case_prefix(&needle, &mut prefix_table);

    let result = contains_ascii_case_insensitive(&haystack, &needle, &prefix_table);

    assert!(!result.found);
    assert!(
        result.comparisons <= haystack.len().saturating_mul(2),
        "{} comparisons exceeded the linear bound",
        result.comparisons
    );
}

#[test]
fn activating_a_playlist_queues_its_tracks_as_one_mutation() {
    let mut app = AppState::new(&Config::default(), fixture_index()).expect("app state");
    app.browser.selection = 1;
    app.apply(AppAction::Activate);
    app.browser.selection = (0..app.library_row_count())
        .position(|position| {
            matches!(
                app.library_row(position),
                Some(BrowserRow::Playlist { playlist_index: 0 })
            )
        })
        .expect("fixture playlist remains visible after collapsing an earlier folder");

    let start = app.apply(AppAction::Activate);

    assert_eq!(app.queue.items().len(), 2);
    assert_eq!(app.queue.items()[0].entry_id, TrackEntryId(40));
    assert_eq!(app.queue.items()[1].entry_id, TrackEntryId(50));
    assert_eq!(app.queue.generation(), 1);
    assert!(
        matches!(start, Some(PlaybackIntent::Load { item, .. }) if item.entry_id == TrackEntryId(40))
    );
    assert_eq!(app.status_message, "Loading: Night Song");
    app.advance_time(Duration::from_secs(30));
    assert_eq!(
        app.status_message, "Loading: Night Song",
        "the current loading state is not a transient action notice"
    );

    let mut config = Config::default();
    config.queue.max_items = 1;
    config.queue.max_bytes = 32;
    let mut limited = AppState::new(&config, fixture_index()).expect("limited app state");
    limited.browser.selection = (0..limited.library_row_count())
        .position(|position| {
            matches!(
                limited.library_row(position),
                Some(BrowserRow::Playlist { playlist_index: 0 })
            )
        })
        .expect("fixture playlist has a visible browser row");
    limited.apply(AppAction::Activate);
    assert!(
        limited.queue.items().is_empty(),
        "a rejected playlist stays atomic"
    );
    assert_eq!(limited.queue.generation(), 0);
    assert_eq!(limited.status_message, "Queue limit reached");
    select_entry(&mut limited, 0);
    limited.apply(AppAction::Activate);
    assert_eq!(
        limited.queue.items().len(),
        1,
        "a capacity rejection rolls back playlist asset marks"
    );
}

#[test]
fn duplicate_audio_activation_does_not_mutate_the_queue() {
    let mut app = AppState::new(&Config::default(), fixture_index()).expect("app state");
    select_entry(&mut app, 0);
    app.apply(AppAction::Activate);
    select_entry(&mut app, 2);

    let duplicate = app.apply(AppAction::Activate);

    assert!(duplicate.is_none());
    assert_eq!(app.queue.items().len(), 1);
    assert_eq!(app.queue.generation(), 1);
    assert_eq!(app.status_message, "Audio already in Queue");

    app.focus = Focus::Queue;
    app.apply(AppAction::QueueRemove);
    app.focus = Focus::Library;
    select_entry(&mut app, 2);
    app.apply(AppAction::Activate);
    assert_eq!(
        app.queue.items().len(),
        1,
        "removed audio can be queued again"
    );
    assert_eq!(app.queue.items()[0].entry_id, TrackEntryId(40));
}

#[test]
fn playlist_with_queued_audio_is_rejected_atomically() {
    let mut app = AppState::new(&Config::default(), fixture_index()).expect("app state");
    select_entry(&mut app, 1);
    app.apply(AppAction::Activate);
    app.browser.selection = (0..app.library_row_count())
        .position(|position| {
            matches!(
                app.library_row(position),
                Some(BrowserRow::Playlist { playlist_index: 0 })
            )
        })
        .expect("fixture playlist has a visible browser row");

    let duplicate = app.apply(AppAction::Activate);

    assert!(duplicate.is_none());
    assert_eq!(app.queue.items().len(), 1);
    assert_eq!(app.queue.generation(), 1);
    assert_eq!(app.status_message, "Audio already in Queue");

    select_entry(&mut app, 0);
    app.apply(AppAction::Activate);
    assert_eq!(
        app.queue.items().len(),
        2,
        "playlist preflight rolls back its marks"
    );
}

#[test]
fn queue_mutations_are_bounded_and_advance_generation_once() {
    let mut config = Config::default();
    config.queue.max_items = 2;
    config.queue.max_bytes = 64;
    let mut app =
        AppState::new(&config, fixture_index_with_distinct_context_assets()).expect("app state");

    select_entry(&mut app, 0);
    app.apply(AppAction::Activate);
    select_entry(&mut app, 1);
    app.apply(AppAction::Activate);
    select_entry(&mut app, 2);
    app.apply(AppAction::Activate);
    assert_eq!(app.queue.items().len(), 2);
    assert_eq!(app.queue.generation(), 2);
    assert_eq!(app.status_message, "Queue limit reached");
    assert_eq!(app.queue.items()[0].entry_id, TrackEntryId(10));
    assert_eq!(app.queue.items()[0].scan_generation, 7);
    assert_eq!(app.queue.items()[1].entry_id, TrackEntryId(20));

    app.focus = Focus::Queue;
    app.apply(AppAction::QueueMoveDown);
    assert_eq!(app.queue.items()[0].entry_id, TrackEntryId(20));
    assert_eq!(app.queue.selection, 1);
    assert_eq!(app.queue.generation(), 3);

    app.apply(AppAction::QueueRemove);
    assert_eq!(app.queue.items().len(), 1);
    assert_eq!(app.queue.generation(), 4);
    app.apply(AppAction::QueueClear);
    assert!(app.queue.items().is_empty());
    assert_eq!(app.queue.generation(), 5);
    app.apply(AppAction::QueueClear);
    assert_eq!(
        app.queue.generation(),
        5,
        "an empty clear is not a mutation"
    );

    app.focus = Focus::Library;
    select_entry(&mut app, 1);
    app.apply(AppAction::Activate);
    assert_eq!(
        app.queue.items().len(),
        1,
        "cleared audio can be queued again"
    );
    assert_eq!(app.queue.generation(), 6);
}

#[test]
fn playback_generations_and_queue_navigation_stay_in_the_app_loop() {
    let mut app = AppState::new(&Config::default(), fixture_index()).expect("app state");
    select_entry(&mut app, 0);
    let first = app
        .apply(AppAction::Activate)
        .expect("idle activation starts the selected track");
    select_entry(&mut app, 1);
    app.apply(AppAction::Activate);

    let PlaybackIntent::Load {
        generation: first_generation,
        item: first_item,
        ..
    } = first
    else {
        panic!("play must load the selected queue item");
    };
    assert_eq!(first_generation, 1);
    assert_eq!(first_item.entry_id, TrackEntryId(10));
    assert_eq!(app.playback.status, PlaybackStatus::Loading);

    app.audio_event(AudioEvent::Started {
        generation: first_generation,
        timeline_revision: 1,
        format: STEREO_48_KHZ,
        duration: Some(Duration::from_secs(10)),
        position: Duration::ZERO,
    });
    assert_eq!(app.playback.status, PlaybackStatus::Playing);
    assert!(matches!(
        app.apply(AppAction::PlayPause),
        Some(PlaybackIntent::Pause { generation: 1 })
    ));
    app.audio_event(AudioEvent::Paused { generation: 1 });
    assert_eq!(app.playback.status, PlaybackStatus::Paused);
    assert!(matches!(
        app.apply(AppAction::PlayPause),
        Some(PlaybackIntent::Resume { generation: 1 })
    ));

    let second = app.apply(AppAction::Next).expect("load next track");
    assert!(matches!(
        second,
        PlaybackIntent::Load {
            generation: 2,
            item,
            ..
        } if item.entry_id == TrackEntryId(20)
    ));
    app.audio_event(AudioEvent::Finished { generation: 1 });
    assert_eq!(
        app.playback.status,
        PlaybackStatus::Loading,
        "a stale completion cannot replace the newer load"
    );

    let previous = app
        .apply(AppAction::Previous)
        .expect("return to first track");
    assert!(matches!(
        previous,
        PlaybackIntent::Load {
            generation: 3,
            item,
            ..
        } if item.entry_id == TrackEntryId(10)
    ));
    let automatic = app
        .audio_event(AudioEvent::Finished { generation: 3 })
        .expect("completion advances to the next queue item");
    assert!(matches!(
        automatic,
        PlaybackIntent::Load {
            generation: 4,
            item,
            ..
        } if item.entry_id == TrackEntryId(20)
    ));
    assert!(
        app.audio_event(AudioEvent::Finished { generation: 4 })
            .is_none()
    );
    assert_eq!(app.playback.status, PlaybackStatus::Stopped);
    assert_eq!(app.status_message, "Queue finished");
}

#[test]
fn removing_the_current_queue_item_keeps_the_next_position_deterministic() {
    let mut app = AppState::new(&Config::default(), fixture_index()).expect("app state");
    select_entry(&mut app, 0);
    app.apply(AppAction::Activate);
    select_entry(&mut app, 1);
    app.apply(AppAction::Activate);

    app.focus = Focus::Queue;
    app.queue.selection = 0;
    app.apply(AppAction::QueueRemove);
    let next = app
        .audio_event(AudioEvent::Finished { generation: 1 })
        .expect("removed current item advances to its successor");

    assert!(matches!(
        next,
        PlaybackIntent::Load { item, .. } if item.entry_id == TrackEntryId(20)
    ));
}

#[test]
fn removing_entries_before_an_absent_playing_item_preserves_its_successor() {
    let mut app = AppState::new(
        &Config::default(),
        fixture_index_with_distinct_context_assets(),
    )
    .expect("app state");
    for index in 0..4 {
        select_entry(&mut app, index);
        app.apply(AppAction::Activate);
    }
    app.apply(AppAction::Next);
    let successor = app.queue.items()[2].entry_id;
    app.focus = Focus::Queue;
    app.queue.selection = 1;
    app.apply(AppAction::QueueRemove);
    app.queue.selection = 0;
    app.apply(AppAction::QueueRemove);

    let next = app.audio_event(AudioEvent::Finished { generation: 2 });

    assert!(matches!(next, Some(PlaybackIntent::Load { item, .. })
        if item.entry_id == successor));
}

#[test]
fn stop_clears_a_failed_playback_without_waiting_for_a_dead_worker_track() {
    let mut app = AppState::new(&Config::default(), fixture_index()).expect("app state");
    select_entry(&mut app, 0);
    app.apply(AppAction::Activate);
    app.audio_event(AudioEvent::Failed {
        generation: 1,
        message: "fixture failure".into(),
    });

    assert!(app.apply(AppAction::Stop).is_none());
    assert_eq!(app.playback.status, PlaybackStatus::Stopped);
    assert_eq!(app.status_message, "Stopped");
}

#[test]
fn playback_controls_are_bounded_and_keep_position_in_app_state() {
    let mut app = AppState::new(&Config::default(), fixture_index()).expect("app state");
    select_entry(&mut app, 0);
    app.apply(AppAction::Activate)
        .expect("idle Enter starts playback");
    app.audio_event(AudioEvent::Started {
        generation: 1,
        timeline_revision: 1,
        format: STEREO_48_KHZ,
        duration: Some(Duration::from_secs(30)),
        position: Duration::ZERO,
    });
    app.audio_position(AudioPosition {
        generation: 1,
        timeline_revision: 1,
        position: Duration::from_secs(10),
        duration: Some(Duration::from_secs(30)),
    });

    assert!(matches!(
        app.apply(AppAction::SeekBackward),
        Some(PlaybackIntent::Seek { position, .. }) if position == Duration::from_secs(5)
    ));
    assert!(matches!(
        app.apply(AppAction::SeekForward),
        Some(PlaybackIntent::Seek { position, .. }) if position == Duration::from_secs(10)
    ));
    assert!(matches!(
        app.apply(AppAction::VolumeDown),
        Some(PlaybackIntent::SetGain {
            volume_percent: 95,
            muted: false,
            ..
        })
    ));
    assert!(matches!(
        app.apply(AppAction::ToggleMute),
        Some(PlaybackIntent::SetGain {
            volume_percent: 95,
            muted: true,
            ..
        })
    ));
}

#[test]
fn enter_toggles_playback_outside_the_library() {
    let mut app = AppState::new(&Config::default(), fixture_index()).expect("app state");
    select_entry(&mut app, 0);
    app.apply(AppAction::Activate)
        .expect("idle activation starts playback");
    app.audio_event(AudioEvent::Started {
        generation: 1,
        timeline_revision: 1,
        format: STEREO_48_KHZ,
        duration: Some(Duration::from_secs(30)),
        position: Duration::ZERO,
    });

    app.focus = Focus::Player;
    assert!(matches!(
        app.apply(AppAction::Activate),
        Some(PlaybackIntent::Pause { generation: 1 })
    ));
    app.audio_event(AudioEvent::Paused { generation: 1 });

    app.focus = Focus::Queue;
    assert!(matches!(
        app.apply(AppAction::Activate),
        Some(PlaybackIntent::Resume { generation: 1 })
    ));
}

#[test]
fn external_controls_use_exact_idempotent_app_actions() {
    let mut app = AppState::new(&Config::default(), fixture_index()).expect("app state");
    select_entry(&mut app, 0);
    app.apply(AppAction::Activate)
        .expect("idle Enter starts playback");
    let queue_instance = app.queue.items()[0].instance_id;
    app.audio_event(AudioEvent::Started {
        generation: 1,
        timeline_revision: 1,
        format: STEREO_48_KHZ,
        duration: Some(Duration::from_secs(90)),
        position: Duration::from_secs(20),
    });

    assert!(app.apply(AppAction::Play).is_none());
    assert!(matches!(
        app.apply(AppAction::Pause),
        Some(PlaybackIntent::Pause { generation: 1 })
    ));
    app.audio_event(AudioEvent::Paused { generation: 1 });
    assert!(app.apply(AppAction::Pause).is_none());
    assert!(matches!(
        app.apply(AppAction::Play),
        Some(PlaybackIntent::Resume { generation: 1 })
    ));

    app.muted = true;
    assert!(matches!(
        app.apply(AppAction::SetVolume(37)),
        Some(PlaybackIntent::SetGain {
            generation: 1,
            volume_percent: 37,
            muted: false,
        })
    ));
    assert_eq!(app.volume_percent, 37);
    assert!(!app.muted);

    app.apply(AppAction::SetShuffle(true));
    app.apply(AppAction::SetShuffle(true));
    assert!(app.queue.shuffle);
    app.apply(AppAction::RepeatOne);
    assert_eq!(app.repeat, RepeatMode::One);
    app.apply(AppAction::RepeatAll);
    assert_eq!(app.repeat, RepeatMode::All);
    app.apply(AppAction::RepeatOff);
    assert_eq!(app.repeat, RepeatMode::Off);

    assert!(matches!(
        app.apply(AppAction::SeekRelative {
            forward: false,
            distance: Duration::from_secs(7),
        }),
        Some(PlaybackIntent::Seek { position, .. })
            if position == Duration::from_secs(13)
    ));
    assert!(
        app.apply(AppAction::SeekAbsolute {
            playback_generation: 2,
            queue_instance,
            position: Duration::from_secs(30),
        })
        .is_none()
    );
}

#[test]
fn next_and_previous_preserve_paused_playback() {
    let mut app = app_with_two_queued_tracks();
    app.audio_event(AudioEvent::Started {
        generation: 1,
        timeline_revision: 1,
        format: STEREO_48_KHZ,
        duration: Some(Duration::from_secs(90)),
        position: Duration::from_secs(20),
    });
    app.audio_event(AudioEvent::Paused { generation: 1 });

    assert!(matches!(
        app.apply(AppAction::Next),
        Some(PlaybackIntent::Load {
            generation: 2,
            item,
            paused: true,
            ..
        }) if item.entry_id == TrackEntryId(20)
    ));
    app.audio_event(AudioEvent::Started {
        generation: 2,
        timeline_revision: 1,
        format: STEREO_48_KHZ,
        duration: Some(Duration::from_secs(80)),
        position: Duration::ZERO,
    });
    assert_eq!(app.playback.status, PlaybackStatus::Paused);

    assert!(matches!(
        app.apply(AppAction::Previous),
        Some(PlaybackIntent::Load {
            generation: 3,
            item,
            paused: true,
            ..
        }) if item.entry_id == TrackEntryId(10)
    ));
    app.audio_event(AudioEvent::Started {
        generation: 3,
        timeline_revision: 1,
        format: STEREO_48_KHZ,
        duration: Some(Duration::from_secs(90)),
        position: Duration::ZERO,
    });
    assert_eq!(app.playback.status, PlaybackStatus::Paused);
}

#[test]
fn seeking_to_the_end_preserves_pause_when_advancing_or_repeating() {
    for (repeat, expected_entry) in [
        (RepeatMode::Off, 20),
        (RepeatMode::All, 20),
        (RepeatMode::One, 10),
    ] {
        let mut app = AppState::new(&Config::default(), fixture_index()).expect("app state");
        select_entry(&mut app, 0);
        app.apply(AppAction::Activate).expect("start first track");
        select_entry(&mut app, 1);
        app.apply(AppAction::Activate);
        app.set_repeat(repeat);
        app.audio_event(AudioEvent::Started {
            generation: 1,
            timeline_revision: 1,
            format: AudioFormat {
                sample_rate: 48_000,
                channels: 2,
            },
            duration: Some(Duration::from_secs(10)),
            position: Duration::ZERO,
        });
        app.audio_event(AudioEvent::Paused { generation: 1 });
        let (playback_generation, queue_instance) =
            app.current_track_token().expect("current track");

        assert!(matches!(
            app.apply(AppAction::SeekAbsolute {
                playback_generation,
                queue_instance,
                position: Duration::from_secs(10),
            }),
            Some(PlaybackIntent::Seek { .. })
        ));
        let next = app.audio_event(AudioEvent::Finished { generation: 1 });

        assert!(
            matches!(next, Some(PlaybackIntent::Load { paused: true, item, .. })
            if item.entry_id == TrackEntryId(expected_entry)),
            "{repeat:?}: {next:?}"
        );
    }
}

#[test]
fn next_and_previous_only_select_tracks_while_stopped() {
    let mut app = app_with_two_queued_tracks();
    app.audio_event(AudioEvent::Stopped { generation: 1 });

    assert!(app.apply(AppAction::Next).is_none());
    assert_eq!(app.playback.status, PlaybackStatus::Stopped);
    assert_eq!(app.queue_position(), Some(2));
    assert!(app.apply(AppAction::Previous).is_none());
    assert_eq!(app.playback.status, PlaybackStatus::Stopped);
    assert_eq!(app.queue_position(), Some(1));
}

#[test]
fn relative_seek_beyond_the_end_advances_to_the_next_track() {
    let mut app = app_with_two_queued_tracks();
    app.audio_event(AudioEvent::Started {
        generation: 1,
        timeline_revision: 1,
        format: STEREO_48_KHZ,
        duration: Some(Duration::from_secs(10)),
        position: Duration::from_secs(8),
    });

    assert!(matches!(
        app.apply(AppAction::SeekRelative {
            forward: true,
            distance: Duration::from_secs(5),
        }),
        Some(PlaybackIntent::Load {
            generation: 2,
            item,
            ..
        }) if item.entry_id == TrackEntryId(20)
    ));
}

#[test]
fn absolute_seek_beyond_the_duration_is_ignored() {
    let mut app = AppState::new(&Config::default(), fixture_index()).expect("app state");
    select_entry(&mut app, 0);
    app.apply(AppAction::Activate)
        .expect("idle Enter starts playback");
    let queue_instance = app.queue.items()[0].instance_id;
    app.audio_event(AudioEvent::Started {
        generation: 1,
        timeline_revision: 1,
        format: STEREO_48_KHZ,
        duration: Some(Duration::from_secs(90)),
        position: Duration::from_secs(20),
    });

    assert!(
        app.apply(AppAction::SeekAbsolute {
            playback_generation: 1,
            queue_instance,
            position: Duration::from_secs(91),
        })
        .is_none()
    );
    assert_eq!(app.playback_position(), Duration::from_secs(20));
    assert!(matches!(
        app.apply(AppAction::SeekAbsolute {
            playback_generation: 1,
            queue_instance,
            position: Duration::from_secs(90),
        }),
        Some(PlaybackIntent::Seek { position, .. })
            if position == Duration::from_secs(90)
    ));
}

#[test]
fn fractional_seek_acknowledgements_allow_progress_to_continue() {
    let mut app = AppState::new(&Config::default(), fixture_index()).expect("app state");
    app.apply(AppAction::Activate);
    app.audio_event(AudioEvent::Started {
        generation: 1,
        timeline_revision: 1,
        format: STEREO_48_KHZ,
        duration: Some(Duration::from_mins(1)),
        position: Duration::ZERO,
    });
    app.audio_position(AudioPosition {
        generation: 1,
        timeline_revision: 1,
        position: Duration::from_nanos(256_020_833),
        duration: Some(Duration::from_mins(1)),
    });
    let intent = app.apply(AppAction::SeekForward);
    app.audio_event(AudioEvent::Seeked {
        generation: 1,
        timeline_revision: 2,
        position: Duration::from_micros(5_256_020),
    });
    app.audio_position(AudioPosition {
        generation: 1,
        timeline_revision: 2,
        position: Duration::from_secs(6),
        duration: Some(Duration::from_mins(1)),
    });

    assert_eq!(app.playback_position(), Duration::from_secs(6));
    assert!(matches!(intent, Some(PlaybackIntent::Seek { position, .. })
        if position == Duration::from_micros(5_256_020)));
    assert_eq!(app.seek_revision(), 1);
}

#[test]
fn repeated_seeks_keep_the_latest_target_until_it_is_acknowledged() {
    let mut app = AppState::new(&Config::default(), fixture_index()).expect("app state");
    select_entry(&mut app, 0);
    app.apply(AppAction::Activate)
        .expect("idle Enter starts playback");
    app.audio_event(AudioEvent::Started {
        generation: 1,
        timeline_revision: 1,
        format: STEREO_48_KHZ,
        duration: Some(Duration::from_secs(200)),
        position: Duration::from_secs(10),
    });

    let mut final_intent = None;
    for _ in 0..20 {
        final_intent = app.apply(AppAction::SeekForward);
    }
    assert!(matches!(
        final_intent,
        Some(PlaybackIntent::Seek { position, .. })
            if position == Duration::from_secs(110)
    ));

    app.audio_position(AudioPosition {
        generation: 1,
        timeline_revision: 1,
        position: Duration::from_secs(12),
        duration: Some(Duration::from_secs(200)),
    });
    app.audio_event(AudioEvent::Seeked {
        generation: 1,
        timeline_revision: 2,
        position: Duration::from_secs(15),
    });
    assert_eq!(app.playback_position(), Duration::from_secs(110));

    app.audio_event(AudioEvent::Seeked {
        generation: 1,
        timeline_revision: 3,
        position: Duration::from_secs(110),
    });
    app.audio_position(AudioPosition {
        generation: 1,
        timeline_revision: 3,
        position: Duration::from_secs(111),
        duration: Some(Duration::from_secs(200)),
    });
    assert_eq!(app.playback_position(), Duration::from_secs(111));
}

#[test]
fn timing_revisions_prevent_cross_lane_position_regressions() {
    let mut app = AppState::new(&Config::default(), fixture_index()).expect("app state");
    select_entry(&mut app, 0);
    app.apply(AppAction::Activate)
        .expect("idle Enter starts playback");
    app.audio_position(AudioPosition {
        generation: 1,
        timeline_revision: 1,
        position: Duration::from_secs(2),
        duration: Some(Duration::from_secs(30)),
    });

    app.audio_event(AudioEvent::Started {
        generation: 1,
        timeline_revision: 1,
        format: STEREO_48_KHZ,
        duration: Some(Duration::from_secs(30)),
        position: Duration::ZERO,
    });
    assert_eq!(app.playback_position(), Duration::from_secs(2));

    app.audio_position(AudioPosition {
        generation: 1,
        timeline_revision: 2,
        position: Duration::from_secs(12),
        duration: Some(Duration::from_secs(30)),
    });
    app.audio_event(AudioEvent::Seeked {
        generation: 1,
        timeline_revision: 2,
        position: Duration::from_secs(5),
    });
    app.audio_position(AudioPosition {
        generation: 1,
        timeline_revision: 1,
        position: Duration::from_secs(20),
        duration: Some(Duration::from_secs(30)),
    });
    assert_eq!(app.playback_position(), Duration::from_secs(12));

    app.audio_event(AudioEvent::Stopped { generation: 1 });
    app.audio_position(AudioPosition {
        generation: 1,
        timeline_revision: 2,
        position: Duration::from_secs(20),
        duration: Some(Duration::from_secs(30)),
    });
    assert_eq!(app.playback_position(), Duration::ZERO);
}

#[test]
fn shuffle_is_a_permutation_and_repeat_modes_choose_in_the_app_loop() {
    let mut app = AppState::new(&Config::default(), fixture_index()).expect("app state");
    app.queue.shuffle_seed = 1;
    select_entry(&mut app, 0);
    app.apply(AppAction::Activate);
    select_entry(&mut app, 1);
    app.apply(AppAction::Activate);
    select_entry(&mut app, 2);
    app.apply(AppAction::Activate);

    app.apply(AppAction::ToggleShuffle);
    let mut expected: Vec<_> = app
        .queue
        .items()
        .iter()
        .map(|item| item.instance_id)
        .collect();
    let mut actual = app.queue.shuffle_order.clone();
    expected.sort_unstable();
    actual.sort_unstable();
    assert_eq!(actual, expected);
    assert_eq!(
        app.queue.shuffle_order.first().copied(),
        app.playback.current.map(|item| item.instance_id),
        "enabling shuffle keeps the current track at the head of the round"
    );

    app.apply(AppAction::CycleRepeat);
    assert_eq!(app.repeat, RepeatMode::All);
    app.apply(AppAction::CycleRepeat);
    assert_eq!(app.repeat, RepeatMode::One);
    let current = app.current_queue_index().expect("current queue item");
    let repeated = app
        .audio_event(AudioEvent::Finished { generation: 1 })
        .expect("repeat one reloads the same queue item");
    assert!(matches!(
        repeated,
        PlaybackIntent::Load { item, .. } if item.instance_id == app.queue.items()[current].instance_id
    ));

    app.apply(AppAction::CycleRepeat);
    assert_eq!(app.repeat, RepeatMode::Off);
}

#[test]
fn saved_session_restores_available_queue_entries_at_position() {
    let mut app = AppState::new(&Config::default(), fixture_index()).expect("app state");
    let snapshot = crate::state::SessionSnapshot::new(vec![10, 999, 20, 10], 2, 123_400);

    app.restore_session(&snapshot).expect("restore session");

    assert_eq!(
        app.queue
            .items()
            .iter()
            .map(|item| item.entry_id)
            .collect::<Vec<_>>(),
        [TrackEntryId(10), TrackEntryId(20)]
    );
    assert_eq!(app.queue.selection, 1);
    assert_eq!(app.playback.status, PlaybackStatus::Stopped);
    assert_eq!(app.playback_position(), Duration::from_millis(123_400));
    assert!(
        app.status_message
            .contains("2 unavailable or duplicate tracks skipped")
    );

    assert!(matches!(
        app.apply(AppAction::PlayPause),
        Some(PlaybackIntent::Load {
            item,
            position,
            settings,
            ..
        }) if item.entry_id == TrackEntryId(20)
            && position == Duration::from_millis(123_400)
            && settings == crate::audio::PlaybackSettings::default()
    ));
}

#[test]
fn clearing_the_queue_removes_the_resumable_session() {
    let mut app = AppState::new(&Config::default(), fixture_index()).expect("app state");
    select_entry(&mut app, 0);
    app.apply(AppAction::Activate);
    assert!(app.session_snapshot().expect("session snapshot").is_some());

    app.focus = Focus::Queue;
    app.apply(AppAction::QueueClear);

    assert!(app.session_snapshot().expect("cleared snapshot").is_none());
}

#[test]
fn a_finished_track_resumes_from_the_beginning() {
    for duration in [Some(Duration::from_secs(10)), None] {
        let mut app = AppState::new(&Config::default(), fixture_index()).expect("app state");
        select_entry(&mut app, 0);
        app.apply(AppAction::Activate);
        app.audio_event(AudioEvent::Started {
            generation: 1,
            timeline_revision: 1,
            format: AudioFormat {
                sample_rate: 48_000,
                channels: 2,
            },
            duration,
            position: Duration::ZERO,
        });
        app.audio_position(AudioPosition {
            generation: 1,
            timeline_revision: 1,
            position: Duration::from_secs(7),
            duration,
        });
        app.audio_event(AudioEvent::Finished { generation: 1 });

        let snapshot = app
            .session_snapshot()
            .expect("session snapshot")
            .expect("saved queue");

        assert_eq!(snapshot.position_ms, 0);

        let replay = app.apply(AppAction::PlayPause);
        assert!(matches!(replay, Some(PlaybackIntent::Load { position, .. })
            if position == Duration::ZERO));
    }
}

#[test]
fn shuffled_playlist_starts_at_its_first_track_without_skipping_the_round() {
    let mut app = AppState::new(&Config::default(), fixture_index()).expect("app state");
    app.queue.shuffle = true;
    app.queue.shuffle_seed = 2;
    app.browser.selection = app
        .browser
        .rows
        .iter()
        .position(|row| matches!(row, BrowserRow::Playlist { playlist_index: 0 }))
        .expect("fixture playlist has a browser row");

    let first = app.apply(AppAction::Activate).expect("playlist starts");
    let PlaybackIntent::Load {
        generation,
        item: first,
        ..
    } = first
    else {
        panic!("playlist activation must load its first track");
    };
    assert_eq!(first.entry_id, TrackEntryId(40));
    assert_eq!(app.queue.shuffle_order.first(), Some(&first.instance_id));

    let second = app
        .audio_event(AudioEvent::Finished { generation })
        .expect("shuffle round retains the other playlist track");
    let PlaybackIntent::Load {
        generation,
        item: second,
        ..
    } = second
    else {
        panic!("the remaining playlist track must load");
    };
    assert_ne!(second.instance_id, first.instance_id);
    assert!(
        app.audio_event(AudioEvent::Finished { generation })
            .is_none()
    );
    assert_eq!(app.playback.status, PlaybackStatus::Stopped);
}

#[test]
fn queue_edits_preserve_the_played_shuffle_prefix() {
    let mut app = AppState::new(
        &Config::default(),
        fixture_index_with_distinct_context_assets(),
    )
    .expect("app state");
    app.queue.shuffle_seed = 1;
    for entry_index in 0..3 {
        select_entry(&mut app, entry_index);
        app.apply(AppAction::Activate);
    }
    app.apply(AppAction::ToggleShuffle);
    app.apply(AppAction::Next).expect("advance within shuffle");
    let cursor = app.queue.shuffle_cursor.expect("shuffle cursor");
    assert!(cursor > 0);
    let played_prefix = app.queue.shuffle_order[..=cursor].to_vec();

    select_entry(&mut app, 3);
    app.apply(AppAction::Activate);
    assert_eq!(
        &app.queue.shuffle_order[..=cursor],
        played_prefix.as_slice()
    );

    let order_before_reorder = app.queue.shuffle_order.clone();
    app.focus = Focus::Queue;
    app.queue.selection = 0;
    app.apply(AppAction::QueueMoveDown);
    assert_eq!(app.queue.shuffle_order, order_before_reorder);

    let unplayed = app.queue.shuffle_order[cursor + 1];
    app.queue.selection = app
        .queue
        .items()
        .iter()
        .position(|item| item.instance_id == unplayed)
        .expect("unplayed item remains queued");
    app.apply(AppAction::QueueRemove);
    assert_eq!(
        &app.queue.shuffle_order[..=cursor],
        played_prefix.as_slice()
    );

    let previous = app
        .apply(AppAction::Previous)
        .expect("shuffle history remains");
    assert!(matches!(
        previous,
        PlaybackIntent::Load { item, .. }
            if item.instance_id == played_prefix[played_prefix.len() - 2]
    ));
}
