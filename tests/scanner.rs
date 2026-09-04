// SPDX-License-Identifier: Apache-2.0

use std::ffi::OsString;
use std::fs::{self, File};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{MetadataExt, symlink};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::time::Duration;

use suzumushi::config::Config;
use suzumushi::init::initialize;
use suzumushi::metadata::MetadataReader;
use suzumushi::model::{ScanWarningCode, TrackEntrySource, TrackTags};
use suzumushi::scan::{
    ScanObserver, ScanOptions, SecureOpenMode, reserve_replacement, scan_with_components,
};
use tempfile::TempDir;

#[derive(Default)]
struct FakeMetadata {
    tags: TrackTags,
    attempts: usize,
}

impl MetadataReader for FakeMetadata {
    fn read(&mut self, _file: &File) -> Result<TrackTags, String> {
        self.attempts += 1;
        Ok(self.tags.clone())
    }
}

#[derive(Default)]
struct Noop;
impl ScanObserver for Noop {}

type Configure = Box<dyn Fn(&mut Config)>;
type CounterCase = (&'static str, Configure, Configure);

fn retained_max_path_bytes(index: &suzumushi::model::ScanIndex) -> usize {
    index
        .entries
        .iter()
        .map(|entry| entry.display_path.as_os_str().as_bytes().len())
        .chain(
            index
                .playlists
                .iter()
                .map(|playlist| playlist.path.as_os_str().as_bytes().len()),
        )
        .chain(
            index
                .warnings
                .iter()
                .map(|warning| warning.path.as_os_str().as_bytes().len()),
        )
        .max()
        .expect("the baseline retains paths")
}

fn retained_counter_cases(baseline: &suzumushi::model::ScanIndex) -> Vec<CounterCase> {
    let media_files = baseline.counters.media_files;
    let entries = baseline.counters.encountered_entries;
    let playlists = baseline.playlists.len();
    let playlist_entries = baseline
        .playlists
        .iter()
        .map(|value| value.entries.len())
        .max()
        .expect("at least the demo playlist");
    let symlinks = baseline.counters.symlink_resolutions;
    let parsers = baseline.counters.parser_attempts;
    let path_bytes = baseline.counters.path_bytes;
    let max_path_bytes = retained_max_path_bytes(baseline);
    let metadata_bytes = baseline.counters.metadata_bytes;
    let warning_bytes = baseline.counters.warning_bytes;
    let index_bytes = baseline.counters.index_bytes;
    let open_files = baseline.counters.open_files_high_water;

    vec![
        (
            "media files",
            Box::new(move |config| config.scan.max_files = media_files),
            Box::new(move |config| config.scan.max_files = media_files - 1),
        ),
        (
            "encountered entries",
            Box::new(move |config| config.scan.max_entries = entries),
            Box::new(move |config| config.scan.max_entries = entries - 1),
        ),
        (
            "depth",
            Box::new(|config| config.scan.max_depth = 2),
            Box::new(|config| config.scan.max_depth = 1),
        ),
        (
            "playlists",
            Box::new(move |config| config.scan.max_playlists = playlists),
            Box::new(move |config| config.scan.max_playlists = playlists - 1),
        ),
        (
            "playlist entries",
            Box::new(move |config| {
                config.scan.max_entries_per_playlist = playlist_entries;
            }),
            Box::new(move |config| {
                config.scan.max_entries_per_playlist = playlist_entries - 1;
            }),
        ),
        (
            "symlink resolutions",
            Box::new(move |config| config.scan.max_symlink_resolutions = symlinks),
            Box::new(move |config| config.scan.max_symlink_resolutions = symlinks - 1),
        ),
        (
            "parser attempts",
            Box::new(move |config| config.scan.max_parser_attempts = parsers),
            Box::new(move |config| config.scan.max_parser_attempts = parsers - 1),
        ),
        (
            "path bytes",
            Box::new(move |config| config.scan.max_path_bytes = max_path_bytes),
            Box::new(move |config| config.scan.max_path_bytes = max_path_bytes - 1),
        ),
        (
            "total path bytes",
            Box::new(move |config| config.scan.max_total_path_bytes = path_bytes),
            Box::new(move |config| config.scan.max_total_path_bytes = path_bytes - 1),
        ),
        (
            "metadata bytes",
            Box::new(move |config| config.scan.max_total_metadata_bytes = metadata_bytes),
            Box::new(move |config| config.scan.max_total_metadata_bytes = metadata_bytes - 1),
        ),
        (
            "warning bytes",
            Box::new(move |config| config.scan.max_warning_bytes = warning_bytes),
            Box::new(move |config| config.scan.max_warning_bytes = warning_bytes - 1),
        ),
        (
            "index bytes",
            Box::new(move |config| config.scan.max_index_bytes = index_bytes),
            Box::new(move |config| config.scan.max_index_bytes = index_bytes - 1),
        ),
        (
            "open files",
            Box::new(move |config| config.runtime.max_open_files = open_files),
            Box::new(move |config| config.runtime.max_open_files = open_files - 1),
        ),
    ]
}

fn root() -> (TempDir, PathBuf) {
    let temp = TempDir::new().expect("temporary directory");
    let paths = initialize(&temp.path().join("root")).expect("initialize root");
    (temp, paths.root)
}

fn scan(
    root: &Path,
    config: &Config,
    reader: &mut dyn MetadataReader,
) -> suzumushi::model::ScanIndex {
    scan_with_components(root, config, 7, ScanOptions::default(), reader, &mut Noop)
        .expect("scan succeeds")
}

#[test]
fn one_pass_finds_context_metadata_and_symlinks() {
    let (_temp, root) = root();
    let library = root.join("audio/library");
    let playlist = root.join("audio/playlists/focus");
    fs::create_dir(&playlist).expect("playlist");
    fs::write(library.join("10 original.mp3"), b"media").expect("library media");
    fs::write(library.join("2 copied.flac"), b"copy").expect("copied media");
    symlink(
        library.join("10 original.mp3"),
        playlist.join("10 link.mp3"),
    )
    .expect("playlist symlink");
    fs::copy(
        library.join("2 copied.flac"),
        playlist.join("2 copied.flac"),
    )
    .expect("playlist copy");

    let tags = TrackTags {
        artist: Some("Calm Artist".into()),
        album_artist: Some("Album Artist".into()),
        album: Some("Quiet Album".into()),
        title: Some("Night Song".into()),
    };
    let mut reader = FakeMetadata { tags, attempts: 0 };
    let index = scan(&root, &Config::default(), &mut reader);

    assert!(index.complete);
    assert_eq!(index.generation, 7);
    assert_eq!(index.entries.len(), 4);
    assert_eq!(
        index.assets.len(),
        3,
        "one symlink must reuse its canonical asset"
    );
    assert_eq!(
        reader.attempts, 3,
        "metadata is parsed once per canonical asset"
    );
    let focus = index
        .playlists
        .iter()
        .find(|value| value.name == "focus")
        .expect("focus playlist");
    assert_eq!(focus.entries.len(), 2);
    let playlist_paths: Vec<_> = focus
        .entries
        .iter()
        .map(|id| {
            index
                .entries
                .iter()
                .find(|entry| entry.id == *id)
                .expect("playlist entry model")
                .display_path
                .as_path()
        })
        .collect();
    assert_eq!(
        playlist_paths,
        [
            Path::new("playlists/focus/2 copied.flac"),
            Path::new("playlists/focus/10 link.mp3")
        ],
        "playlist entries use natural order, not creation order"
    );
    assert!(
        index
            .entries
            .iter()
            .any(|entry| matches!(entry.source, TrackEntrySource::PlaylistSymlink { .. }))
    );
    assert!(index.entries.iter().all(|entry| {
        index
            .asset_for_entry(entry)
            .and_then(|asset| asset.tags.artist.as_deref())
            == Some("Calm Artist")
    }));
    assert!(
        index
            .entries
            .iter()
            .all(|entry| !entry.search.filename.is_empty())
    );
    assert_eq!(
        index.entries[0].display_path,
        Path::new("library/2 copied.flac"),
        "natural order places 2 before 10"
    );
}

#[test]
fn hard_links_share_one_asset_and_keep_its_identity_across_names() {
    let (_temp, root) = root();
    let library_file = root.join("audio/library/original.mp3");
    let playlist = root.join("audio/playlists/focus");
    fs::create_dir(&playlist).expect("playlist");
    fs::write(&library_file, b"media").expect("library media");
    fs::hard_link(&library_file, playlist.join("copy.mp3")).expect("playlist hard link");

    let mut reader = FakeMetadata::default();
    let first = scan(&root, &Config::default(), &mut reader);

    assert_eq!(first.entries.len(), 2);
    assert_eq!(first.assets.len(), 1, "hard links name one inode");
    assert_eq!(reader.attempts, 1, "metadata is parsed once per inode");
    assert!(
        first
            .entries
            .iter()
            .all(|entry| entry.asset_index == first.entries[0].asset_index)
    );
    let asset_identity = first.assets[0].file_identity;

    fs::remove_file(&library_file).expect("remove one hard-link name");
    let mut reader = FakeMetadata::default();
    let second = scan(&root, &Config::default(), &mut reader);

    assert_eq!(second.entries.len(), 1);
    assert_eq!(second.assets.len(), 1);
    assert_eq!(
        second.assets[0].file_identity, asset_identity,
        "the inode identity is stable"
    );
}

#[test]
fn unsupported_aac_and_m4a_stay_out_while_oga_remains_discoverable() {
    let (_temp, root) = root();
    let library = root.join("audio/library");
    fs::write(library.join("supported.oga"), b"media").expect("supported Ogg fixture");
    fs::write(library.join("unsupported.aac"), b"media").expect("unsupported AAC fixture");
    fs::write(library.join("unsupported.m4a"), b"media").expect("unsupported M4A fixture");

    let mut reader = FakeMetadata::default();
    let index = scan(&root, &Config::default(), &mut reader);

    assert_eq!(index.entries.len(), 1);
    assert_eq!(
        index.entries[0].display_path,
        Path::new("library/supported.oga")
    );
    assert_eq!(
        reader.attempts, 1,
        "unsupported files must not reach metadata parsing"
    );
    assert!(index.warnings.iter().any(|warning| {
        warning.code == ScanWarningCode::UnsupportedExtension
            && warning.path == Path::new("library/unsupported.aac")
    }));
    assert!(index.warnings.iter().any(|warning| {
        warning.code == ScanWarningCode::UnsupportedExtension
            && warning.path == Path::new("library/unsupported.m4a")
    }));
}

#[test]
fn hidden_audio_policy_applies_to_files_symlinks_and_directories() {
    let (_temp, root) = root();
    let library = root.join("audio/library");
    fs::write(library.join("visible.mp3"), b"visible").expect("visible media");
    fs::write(library.join(".hidden.mp3"), b"hidden").expect("hidden media");
    symlink(
        library.join("visible.mp3"),
        library.join(".hidden-link.mp3"),
    )
    .expect("hidden symlink");
    fs::create_dir(library.join(".hidden-folder")).expect("hidden folder");
    fs::write(library.join(".hidden-folder/song.mp3"), b"nested").expect("nested hidden media");

    let mut reader = FakeMetadata::default();
    let hidden = scan(&root, &Config::default(), &mut reader);
    assert_eq!(hidden.entries.len(), 1);

    let mut config = Config::default();
    config.scan.ignore_hidden_audio = false;
    let mut reader = FakeMetadata::default();
    let visible = scan(&root, &config, &mut reader);
    assert_eq!(visible.entries.len(), 4);
}

#[test]
fn hidden_directory_at_audio_root_does_not_consume_descendant_budgets() {
    let (_temp, root) = root();
    let mut reader = FakeMetadata::default();
    let baseline = scan(&root, &Config::default(), &mut reader);

    let hidden = root.join("audio/.git");
    fs::create_dir(&hidden).expect("hidden directory");
    fs::write(hidden.join("index"), b"ignored").expect("hidden descendant");

    let mut config = Config::default();
    config.scan.max_files = 1;
    config.scan.max_entries = baseline.counters.encountered_entries + 1;
    let mut reader = FakeMetadata::default();
    let index = scan(&root, &config, &mut reader);

    assert!(index.complete, "{:?}", index.warnings);
    assert_eq!(
        index.counters.encountered_entries,
        baseline.counters.encountered_entries + 1
    );
    assert_eq!(
        index.counters.directory_enumerations,
        baseline.counters.directory_enumerations
    );
}

#[test]
fn unrelated_directory_at_audio_root_does_not_consume_descendant_budgets() {
    let (_temp, root) = root();
    let mut reader = FakeMetadata::default();
    let baseline = scan(&root, &Config::default(), &mut reader);

    let unrelated = root.join("audio/notes");
    fs::create_dir(&unrelated).expect("unrelated directory");
    fs::write(unrelated.join("ignored.mp3"), b"ignored").expect("unrelated descendant");

    let mut config = Config::default();
    config.scan.max_files = 1;
    config.scan.max_entries = baseline.counters.encountered_entries + 1;
    let mut reader = FakeMetadata::default();
    let index = scan(&root, &config, &mut reader);

    assert!(index.complete, "{:?}", index.warnings);
    assert!(index.entries.is_empty());
    assert_eq!(
        index.counters.encountered_entries,
        baseline.counters.encountered_entries + 1
    );
    assert_eq!(
        index.counters.directory_enumerations,
        baseline.counters.directory_enumerations
    );
}

#[test]
fn external_broken_magic_and_directory_symlinks_are_bounded_warnings() {
    let (temp, root) = root();
    let playlist = root.join("audio/playlists/links");
    fs::create_dir(&playlist).expect("playlist");
    let external = temp.path().join("external.mp3");
    fs::write(&external, b"external").expect("external media");
    symlink(&external, playlist.join("external.mp3")).expect("external symlink");
    symlink(temp.path().join("missing.mp3"), playlist.join("broken.mp3")).expect("broken symlink");
    symlink("/proc/self/fd/0", playlist.join("magic.mp3")).expect("magic link");
    symlink(root.join("audio/library"), playlist.join("directory.mp3")).expect("directory link");

    let mut reader = FakeMetadata::default();
    let index = scan(&root, &Config::default(), &mut reader);
    let external_entry = index
        .entries
        .iter()
        .find(|entry| entry.display_path == Path::new("playlists/links/external.mp3"))
        .expect("external entry");
    let external_asset = index
        .asset_for_entry(external_entry)
        .expect("external asset");
    let external_metadata = fs::metadata(&external).expect("external metadata");
    assert_eq!(external_asset.file_identity.device, external_metadata.dev());
    assert_eq!(external_asset.file_identity.inode, external_metadata.ino());
    assert!(
        index
            .warnings
            .iter()
            .any(|warning| warning.code == ScanWarningCode::BrokenSymlink)
    );
    assert!(
        index
            .warnings
            .iter()
            .any(|warning| warning.code == ScanWarningCode::UnsupportedSecureOpen)
    );
    assert!(
        index
            .warnings
            .iter()
            .any(|warning| warning.code == ScanWarningCode::IgnoredDirectorySymlink)
    );
}

#[test]
fn unsupported_secure_open_has_no_weaker_fallback() {
    let (_temp, root) = root();
    let playlist = root.join("audio/playlists/links");
    fs::create_dir(&playlist).expect("playlist");
    let target = root.join("audio/library/song.mp3");
    fs::write(&target, b"media").expect("target");
    symlink(&target, playlist.join("song.mp3")).expect("symlink");
    let mut reader = FakeMetadata::default();
    let mut observer = Noop;
    let index = scan_with_components(
        &root,
        &Config::default(),
        1,
        ScanOptions {
            secure_open: SecureOpenMode::Unsupported,
            ..ScanOptions::default()
        },
        &mut reader,
        &mut observer,
    )
    .expect("bounded unsupported result");
    assert_eq!(
        index.entries.len(),
        1,
        "direct library file remains visible"
    );
    assert!(
        index
            .warnings
            .iter()
            .any(|warning| warning.code == ScanWarningCode::UnsupportedSecureOpen)
    );
}

struct SubstituteLibrary {
    root: PathBuf,
    changed: bool,
}

impl ScanObserver for SubstituteLibrary {
    fn directory_opened(&mut self, relative: &Path) {
        if relative == Path::new("library") && !self.changed {
            self.changed = true;
            let library = self.root.join("audio/library");
            fs::rename(&library, self.root.join("audio/pinned-library"))
                .expect("rename pinned directory");
            fs::create_dir(&library).expect("substitute directory");
            fs::write(library.join("redirected.mp3"), b"wrong tree").expect("substitute media");
        }
    }
}

#[test]
fn parent_substitution_cannot_redirect_a_pinned_enumeration() {
    let (_temp, root) = root();
    fs::write(root.join("audio/library/original.mp3"), b"original").expect("original media");
    let mut reader = FakeMetadata::default();
    let mut observer = SubstituteLibrary {
        root: root.clone(),
        changed: false,
    };
    let index = scan_with_components(
        &root,
        &Config::default(),
        1,
        ScanOptions::default(),
        &mut reader,
        &mut observer,
    )
    .expect("scan pinned directory");
    assert!(observer.changed);
    assert!(
        index
            .entries
            .iter()
            .any(|entry| entry.display_path == Path::new("library/original.mp3"))
    );
    assert!(
        !index
            .entries
            .iter()
            .any(|entry| entry.display_path == Path::new("library/redirected.mp3"))
    );
}

struct RetargetSymlink {
    link: PathBuf,
    replacement: PathBuf,
    changed: bool,
}

impl ScanObserver for RetargetSymlink {
    fn before_symlink_open(&mut self, relative: &Path) {
        if relative == Path::new("playlists/links/song.mp3") && !self.changed {
            self.changed = true;
            fs::remove_file(&self.link).expect("remove old symlink");
            symlink(&self.replacement, &self.link).expect("retarget symlink");
        }
    }
}

#[test]
fn retargeted_symlink_uses_only_the_verified_open_descriptor() {
    let (_temp, root) = root();
    let playlist = root.join("audio/playlists/links");
    fs::create_dir(&playlist).expect("playlist");
    let first = root.join("audio/library/first.mp3");
    let replacement = root.join("audio/library/replacement.mp3");
    fs::write(&first, b"first").expect("first target");
    fs::write(&replacement, b"replacement").expect("replacement target");
    let link = playlist.join("song.mp3");
    symlink(&first, &link).expect("initial symlink");
    let mut observer = RetargetSymlink {
        link,
        replacement: replacement.clone(),
        changed: false,
    };
    let mut reader = FakeMetadata::default();
    let index = scan_with_components(
        &root,
        &Config::default(),
        1,
        ScanOptions::default(),
        &mut reader,
        &mut observer,
    )
    .expect("retargeted scan");
    assert!(observer.changed);
    let entry = index
        .entries
        .iter()
        .find(|entry| entry.display_path == Path::new("playlists/links/song.mp3"))
        .expect("symlink entry");
    let asset = index.asset_for_entry(entry).expect("symlink asset");
    let replacement_metadata = fs::metadata(replacement).expect("replacement metadata");
    assert_eq!(asset.file_identity.device, replacement_metadata.dev());
    assert_eq!(asset.file_identity.inode, replacement_metadata.ino());
}

#[test]
fn metadata_time_budget_skips_tags_without_stopping_discovery() {
    let (_temp, root) = root();
    fs::write(root.join("audio/library/one.mp3"), b"one").expect("first media");
    fs::write(root.join("audio/library/two.mp3"), b"two").expect("second media");
    let mut reader = FakeMetadata::default();
    let mut observer = Noop;

    let index = scan_with_components(
        &root,
        &Config::default(),
        1,
        ScanOptions {
            metadata_time_budget: Duration::ZERO,
            ..ScanOptions::default()
        },
        &mut reader,
        &mut observer,
    )
    .expect("bounded metadata scan");

    assert_eq!(reader.attempts, 0);
    assert_eq!(index.entries.len(), 2);
    assert!(!index.complete);
    assert_eq!(
        index
            .warnings
            .iter()
            .filter(|warning| warning.code == ScanWarningCode::LimitReached)
            .count(),
        1
    );
    assert!(
        index
            .assets
            .iter()
            .all(|asset| asset.tags == TrackTags::default())
    );
}

#[test]
fn invalid_path_bytes_are_unambiguous_and_non_regular_entries_never_panic() {
    let (_temp, root) = root();
    let invalid = OsString::from_vec(b"bad-\xff.mp3".to_vec());
    fs::write(root.join("audio/library").join(invalid), b"media").expect("invalid UTF-8 fixture");
    fs::write(root.join("audio/library/bad-\\xFF.mp3"), b"media").expect("literal escape fixture");
    let _socket =
        UnixListener::bind(root.join("audio/library/socket.mp3")).expect("socket fixture");
    symlink("/dev/null", root.join("audio/library/device.mp3")).expect("non-regular target");
    fs::write(root.join("audio/library/notes.xyz"), b"unsupported")
        .expect("unsupported extension fixture");
    let mut reader = FakeMetadata::default();
    let index = scan(&root, &Config::default(), &mut reader);
    assert_eq!(index.entries.len(), 2);
    assert!(
        index
            .entries
            .iter()
            .any(|entry| entry.search.filename == r"bad-\xFF")
    );
    assert!(
        index
            .entries
            .iter()
            .any(|entry| entry.search.filename == r"bad-\\xFF")
    );
    assert!(
        index
            .warnings
            .iter()
            .filter(|warning| warning.code == ScanWarningCode::NonRegularTarget)
            .count()
            >= 2
    );
    assert!(
        index
            .warnings
            .iter()
            .any(|warning| warning.code == ScanWarningCode::UnsupportedExtension)
    );
}

#[test]
fn files_directly_below_playlists_are_not_playlist_folders() {
    let (_temp, root) = root();
    fs::write(root.join("audio/playlists/not-a-playlist.mp3"), b"media")
        .expect("misplaced media fixture");
    let mut reader = FakeMetadata::default();
    let index = scan(&root, &Config::default(), &mut reader);

    assert!(index.entries.is_empty());
    assert!(
        !index
            .playlists
            .iter()
            .any(|value| value.name == "not-a-playlist.mp3")
    );
    assert!(
        index
            .warnings
            .iter()
            .any(|warning| warning.code == ScanWarningCode::UnsupportedExtension)
    );
}

#[test]
fn enumeration_exhaustion_preserves_admitted_tracks_within_other_limits() {
    let (_temp, root) = root();
    for name in ["a.wav", "b.wav", "c.wav"] {
        fs::write(root.join("audio/library").join(name), b"fake media").expect("media");
    }
    // Every name has the same cost, so this does not depend on readdir ordering.
    for path_limit in [false, true] {
        let mut config = Config::default();
        config.scan.max_files = 2;
        if path_limit {
            config.scan.max_total_path_bytes =
                "library".len() + "playlists".len() + 2 * "library/a.wav".len();
        } else {
            config.scan.max_entries = 4;
        }
        let mut reader = FakeMetadata::default();
        let index = scan(&root, &config, &mut reader);
        assert!(!index.complete);
        assert_eq!(index.entries.len(), 2, "path limit: {path_limit}");
        assert_eq!(reader.attempts, 2);
        assert!(index.counters.encountered_entries <= config.scan.max_entries);
        assert!(index.counters.path_bytes <= config.scan.max_total_path_bytes);

        config.scan.max_files = 1;
        let limited = scan(&root, &config, &mut FakeMetadata::default());
        assert_eq!(limited.entries.len(), 1, "media cap still applies");
    }
}

#[test]
fn scan_counters_stop_at_limit_and_report_limit_plus_one() {
    let (_temp, root) = root();
    fs::write(root.join("audio/library/a.mp3"), b"a").expect("first media");
    fs::write(root.join("audio/library/b.mp3"), b"b").expect("second media");
    let playlist = root.join("audio/playlists/second");
    fs::create_dir(&playlist).expect("second playlist");
    fs::write(playlist.join("c.mp3"), b"c").expect("playlist media");
    fs::write(playlist.join("d.mp3"), b"d").expect("second playlist media");
    symlink(
        root.join("missing-one.mp3"),
        playlist.join("broken-one.mp3"),
    )
    .expect("first broken link");
    symlink(
        root.join("missing-two.mp3"),
        playlist.join("broken-two.mp3"),
    )
    .expect("second broken link");

    let cases: Vec<Configure> = vec![
        Box::new(|config| {
            config.scan.max_files = 1;
        }),
        Box::new(|config| {
            config.scan.max_entries = 3;
            config.scan.max_files = 1;
        }),
        Box::new(|config| {
            config.scan.max_depth = 1;
        }),
        Box::new(|config| {
            config.scan.max_playlists = 1;
        }),
        Box::new(|config| {
            config.scan.max_entries_per_playlist = 1;
        }),
        Box::new(|config| {
            config.scan.max_parser_attempts = 1;
        }),
        Box::new(|config| {
            config.scan.max_path_bytes = 8;
        }),
        Box::new(|config| {
            config.scan.max_total_path_bytes = 8;
        }),
        Box::new(|config| {
            config.scan.max_index_bytes = 128;
        }),
        Box::new(|config| {
            config.scan.max_symlink_resolutions = 1;
        }),
        Box::new(|config| {
            config.scan.max_metadata_field_bytes = 16;
            config.scan.max_total_metadata_bytes = 1;
        }),
        Box::new(|config| {
            config.scan.max_warning_bytes = 1;
        }),
        Box::new(|config| {
            config.runtime.max_open_files = 1;
        }),
    ];
    for configure in cases {
        let mut config = Config::default();
        configure(&mut config);
        let mut reader = FakeMetadata {
            tags: TrackTags {
                title: Some("metadata".into()),
                ..TrackTags::default()
            },
            attempts: 0,
        };
        let index = scan(&root, &config, &mut reader);
        assert!(
            !index.complete,
            "limit-plus-one scan must be visibly partial"
        );
        assert!(index.counters.open_files_high_water <= config.runtime.max_open_files);
    }
}

#[test]
fn retained_counters_accept_the_exact_limit_and_refuse_one_more() {
    let (_temp, root) = root();
    let library = root.join("audio/library");
    fs::write(library.join("a.mp3"), b"a").expect("first media");
    fs::create_dir(library.join("nested")).expect("nested directory");
    fs::write(library.join("nested/song.mp3"), b"song").expect("nested media");
    let playlist = root.join("audio/playlists/focus");
    fs::create_dir(&playlist).expect("playlist");
    fs::copy(library.join("a.mp3"), playlist.join("2 copy.mp3")).expect("playlist copy");
    symlink(library.join("a.mp3"), playlist.join("10 link.mp3")).expect("playlist symlink");
    symlink(library.join("missing.mp3"), playlist.join("broken.mp3")).expect("broken symlink");

    let tags = TrackTags {
        title: Some("meta".into()),
        ..TrackTags::default()
    };
    let mut baseline_reader = FakeMetadata {
        tags: tags.clone(),
        attempts: 0,
    };
    let baseline = scan(&root, &Config::default(), &mut baseline_reader);
    assert!(baseline.complete);
    for (name, exact, lower) in retained_counter_cases(&baseline) {
        let mut config = Config::default();
        exact(&mut config);
        let mut reader = FakeMetadata {
            tags: tags.clone(),
            attempts: 0,
        };
        assert!(
            scan(&root, &config, &mut reader).complete,
            "exact {name} limit must complete"
        );

        let mut config = Config::default();
        lower(&mut config);
        let mut reader = FakeMetadata {
            tags: tags.clone(),
            attempts: 0,
        };
        assert!(
            !scan(&root, &config, &mut reader).complete,
            "{name} limit plus one must be partial"
        );
    }
}

#[test]
fn symlink_parser_metadata_warning_and_mount_counters_are_bounded() {
    let (_temp, root) = root();
    let playlist = root.join("audio/playlists/links");
    fs::create_dir(&playlist).expect("playlist");
    let target = root.join("audio/library/a.mp3");
    fs::write(&target, b"a").expect("media");
    symlink(&target, playlist.join("a.mp3")).expect("first symlink");
    symlink(&target, playlist.join("b.mp3")).expect("second symlink");
    let mut config = Config::default();
    config.scan.max_symlink_resolutions = 1;
    let mut reader = FakeMetadata {
        tags: TrackTags {
            title: Some("x".repeat(32)),
            ..TrackTags::default()
        },
        attempts: 0,
    };
    config.scan.max_metadata_field_bytes = 8;
    let index = scan(&root, &config, &mut reader);
    assert!(!index.complete);
    assert!(index.warnings.iter().any(
        |warning| warning.message.contains("max_symlink_resolutions")
            || warning.code == ScanWarningCode::MetadataRead
    ));

    let mut reader = FakeMetadata::default();
    let mut observer = Noop;
    let mounted = scan_with_components(
        &root,
        &Config::default(),
        1,
        ScanOptions {
            root_device_override: Some(u64::MAX),
            ..ScanOptions::default()
        },
        &mut reader,
        &mut observer,
    )
    .expect("simulated mount scan");
    assert!(mounted.counters.mount_decisions > 0);
    assert!(
        mounted
            .warnings
            .iter()
            .any(|warning| warning.code == ScanWarningCode::CrossMountSkipped)
    );
}

#[test]
fn active_and_replacement_indexes_share_one_process_budget() {
    let mut config = Config::default();
    reserve_replacement(&config, config.scan.max_index_bytes)
        .expect("reviewed default partition fits");
    config.runtime.process_memory_budget_bytes =
        config.scan.max_index_bytes * 2 + 8 * 1_048_576 - 1;
    assert!(reserve_replacement(&config, config.scan.max_index_bytes).is_err());
}
