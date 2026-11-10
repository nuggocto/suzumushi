// SPDX-License-Identifier: Apache-2.0

//! User configuration and the compiled limits the rest of the app runs under.
//!
//! `config.toml` holds only choices a listener makes: the color theme, the first
//! focused view, and three scan behaviors. Every capacity limit is a compiled
//! constant. Tests lower them through the public [`Config`] fields.

use serde::Deserialize;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use rustix::fd::AsFd;
use rustix::fs::{FileType, Mode, OFlags};
use rustix::process::getuid;

use crate::errors::{AppError, AppResult};

/// Maximum accepted configuration file size.
pub const MAX_CONFIG_BYTES: usize = 65_536;

/// The Player's night meadow never draws more cells than this; wider or taller
/// panels center it.
pub(crate) const STAGE_MAX_COLUMNS: usize = 128;
pub(crate) const STAGE_MAX_ROWS: usize = 40;
/// Stalks grow in at most this many rows; taller stages add sky above.
pub(crate) const MEADOW_MAX_ROWS: usize = 16;

/// The default configuration written by `init`.
pub const DEFAULT_CONFIG: &str = r#"# "terminal" uses the terminal's colors; "mono" draws without color.
theme = "terminal"
# The pane focused at startup: "library" or "queue".
default_view = "library"

[scan]
# Scan folders on other filesystems mounted inside the audio folder.
cross_mounts = false
# Play file symlinks. Directory symlinks are always skipped.
follow_file_symlinks = true
# Skip audio files whose names start with a dot.
ignore_hidden_audio = true
"#;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    Terminal,
    Mono,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum DefaultView {
    #[default]
    Library,
    Queue,
}

#[derive(Clone, Debug, Default)]
pub struct Config {
    pub theme: Theme,
    pub default_view: DefaultView,
    pub scan: ScanConfig,
    pub search: SearchConfig,
    pub queue: QueueConfig,
    pub runtime: RuntimeConfig,
}

/// File-symlink behavior during a scan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SymlinkPolicy {
    Ignore,
    FollowFiles,
}

impl SymlinkPolicy {
    #[must_use]
    pub const fn follows_files(self) -> bool {
        matches!(self, Self::FollowFiles)
    }
}

#[derive(Clone, Debug)]
pub struct ScanConfig {
    pub max_files: usize,
    pub max_entries: usize,
    pub max_depth: usize,
    pub max_playlists: usize,
    pub max_entries_per_playlist: usize,
    pub max_symlink_resolutions: usize,
    pub max_parser_attempts: usize,
    pub cross_mounts: bool,
    pub max_path_bytes: usize,
    pub max_total_path_bytes: usize,
    pub max_metadata_field_bytes: usize,
    pub max_total_metadata_bytes: usize,
    pub max_warning_bytes: usize,
    pub max_index_bytes: usize,
    pub symlink_policy: SymlinkPolicy,
    pub ignore_hidden_audio: bool,
}

impl Default for ScanConfig {
    fn default() -> Self {
        Self {
            max_files: 50_000,
            max_entries: 100_000,
            max_depth: 16,
            max_playlists: 2_000,
            max_entries_per_playlist: 10_000,
            max_symlink_resolutions: 50_000,
            max_parser_attempts: 50_000,
            cross_mounts: false,
            max_path_bytes: 4_096,
            max_total_path_bytes: 24 * 1_048_576,
            max_metadata_field_bytes: 16_384,
            max_total_metadata_bytes: 48 * 1_048_576,
            max_warning_bytes: 8 * 1_048_576,
            max_index_bytes: 112 * 1_048_576,
            symlink_policy: SymlinkPolicy::FollowFiles,
            ignore_hidden_audio: true,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SearchConfig {
    pub max_results: usize,
    pub max_query_bytes: usize,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            max_results: 200,
            max_query_bytes: 4_096,
        }
    }
}

#[derive(Clone, Debug)]
pub struct QueueConfig {
    pub max_items: usize,
}

impl Default for QueueConfig {
    fn default() -> Self {
        Self { max_items: 10_000 }
    }
}

#[derive(Clone, Debug)]
pub struct RuntimeConfig {
    pub max_open_files: usize,
    pub status_text_max_bytes: usize,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            max_open_files: 64,
            status_text_max_bytes: 4_096,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ParsedConfig {
    #[serde(default)]
    theme: Theme,
    #[serde(default)]
    default_view: DefaultView,
    #[serde(default)]
    scan: ParsedScanConfig,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ParsedScanConfig {
    #[serde(default)]
    cross_mounts: bool,
    #[serde(default = "enabled")]
    follow_file_symlinks: bool,
    #[serde(default = "enabled")]
    ignore_hidden_audio: bool,
}

impl Default for ParsedScanConfig {
    fn default() -> Self {
        Self {
            cross_mounts: false,
            follow_file_symlinks: true,
            ignore_hidden_audio: true,
        }
    }
}

const fn enabled() -> bool {
    true
}

/// Top-level keys and tables that earlier releases wrote. They no longer change
/// anything, and are dropped so those files keep loading.
const RETIRED_TOP_LEVEL: &[&str] = &[
    "config_version",
    "input",
    "search",
    "queue",
    "runtime",
    "logging",
    // The unversioned 0.x prototype.
    "default_volume",
    "volume_step",
    "speed_min",
    "speed_max",
    "speed_step",
    "seek_short_seconds",
    "seek_long_seconds",
    "shuffle",
    "repeat",
    "queue_max_items",
    "queue_max_bytes",
    "queue_max_history_items",
    "artwork",
    "visualizer",
    "desktop",
    "tags",
    "mutation",
];

/// `[scan]` limits that version 1 config files carry. They are compiled now.
const RETIRED_SCAN: &[&str] = &[
    "max_files",
    "max_entries",
    "max_depth",
    "max_playlists",
    "max_entries_per_playlist",
    "max_symlink_resolutions",
    "max_parser_attempts",
    "max_path_bytes",
    "max_total_path_bytes",
    "max_metadata_field_bytes",
    "max_total_metadata_bytes",
    "max_warning_bytes",
    "max_index_bytes",
    "follow_directory_symlinks",
];

/// Parses a bounded TOML document.
///
/// # Errors
///
/// Returns [`AppError::InvalidConfig`] for oversized, malformed, or unknown input.
pub fn parse(input: &[u8]) -> AppResult<Config> {
    if input.len() > MAX_CONFIG_BYTES {
        return Err(AppError::InvalidConfig(format!(
            "config exceeds {MAX_CONFIG_BYTES} bytes"
        )));
    }
    let text = std::str::from_utf8(input)
        .map_err(|error| AppError::InvalidConfig(format!("config is not UTF-8: {error}")))?;
    let mut document: toml::Table =
        toml::from_str(text).map_err(|error| AppError::InvalidConfig(error.to_string()))?;
    remove_retired_fields(&mut document);
    let parsed: ParsedConfig = toml::Value::Table(document)
        .try_into()
        .map_err(|error| AppError::InvalidConfig(error.to_string()))?;
    Ok(Config {
        theme: parsed.theme,
        default_view: parsed.default_view,
        scan: ScanConfig {
            cross_mounts: parsed.scan.cross_mounts,
            symlink_policy: if parsed.scan.follow_file_symlinks {
                SymlinkPolicy::FollowFiles
            } else {
                SymlinkPolicy::Ignore
            },
            ignore_hidden_audio: parsed.scan.ignore_hidden_audio,
            ..ScanConfig::default()
        },
        ..Config::default()
    })
}

fn remove_retired_fields(document: &mut toml::Table) {
    for key in RETIRED_TOP_LEVEL {
        document.remove(*key);
    }
    // The prototype's playlists view became part of the library.
    if document.get("default_view").and_then(toml::Value::as_str) == Some("playlists") {
        document.insert("default_view".into(), toml::Value::String("library".into()));
    }
    if let Some(scan) = document.get_mut("scan").and_then(toml::Value::as_table_mut) {
        for key in RETIRED_SCAN {
            scan.remove(*key);
        }
    }
}

/// Loads `config.toml` from a root path without following the file target.
///
/// # Errors
///
/// Returns compiled defaults when the file is absent. Returns an error when an
/// existing file cannot be opened safely or does not parse.
pub fn load(root: &Path) -> AppResult<Config> {
    let root_file = rustix::fs::open(
        root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| AppError::io("open config root", root, error.into()))?;
    load_from(&root_file, root)
}

/// Loads config relative to an already pinned root descriptor.
///
/// # Errors
///
/// Returns an error for unsafe file identity, ownership, modes, size, or syntax.
pub fn load_from<Fd: AsFd>(root_file: Fd, root: &Path) -> AppResult<Config> {
    let fd = match rustix::fs::openat(
        root_file,
        "config.toml",
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => fd,
        Err(error) if error == rustix::io::Errno::NOENT => return Ok(Config::default()),
        Err(error) => {
            return Err(AppError::io(
                "open config",
                root.join("config.toml"),
                error.into(),
            ));
        }
    };
    let stat = rustix::fs::fstat(&fd)
        .map_err(|error| AppError::io("inspect config", root.join("config.toml"), error.into()))?;
    if !FileType::from_raw_mode(stat.st_mode).is_file()
        || stat.st_uid != getuid().as_raw()
        || stat.st_mode & 0o777 != 0o600
        || stat.st_nlink != 1
    {
        return Err(AppError::InvalidConfig(
            "config.toml must be a current-user-owned, single-link regular file with mode 0600"
                .into(),
        ));
    }
    let mut bytes = Vec::new();
    File::from(fd)
        .take((MAX_CONFIG_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| AppError::io("read config", root.join("config.toml"), error))?;
    parse(&bytes)
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_CONFIG, DefaultView, MAX_CONFIG_BYTES, SymlinkPolicy, Theme, parse};

    /// The file `init` wrote through release 1.2.2.
    const VERSION_ONE_CONFIG: &str = r#"config_version = 1
theme = "mono"
default_view = "queue"

[scan]
max_files = 50000
max_entries = 100000
max_depth = 16
max_playlists = 2000
max_entries_per_playlist = 10000
max_symlink_resolutions = 50000
max_parser_attempts = 50000
cross_mounts = true
max_path_bytes = 4096
max_total_path_bytes = 25165824
max_metadata_field_bytes = 16384
max_total_metadata_bytes = 50331648
max_warning_bytes = 8388608
max_index_bytes = 117440512
follow_file_symlinks = false
follow_directory_symlinks = false
ignore_hidden_audio = false

[search]
max_results = 200
max_query_bytes = 4096

[queue]
max_items = 10000
max_bytes = 1048576

[runtime]
process_memory_budget_bytes = 402653184
max_open_files = 64
status_text_max_bytes = 4096

[logging]
max_file_bytes = 10485760
max_files = 5
max_record_bytes = 16384
queue_capacity = 512
queue_max_bytes = 8388608
queue_full_policy = "drop_and_count"
"#;

    #[test]
    fn default_config_matches_the_compiled_defaults() {
        let config = parse(DEFAULT_CONFIG.as_bytes()).expect("default config parses");
        assert_eq!(config.theme, Theme::Terminal);
        assert_eq!(config.default_view, DefaultView::Library);
        assert!(!config.scan.cross_mounts);
        assert_eq!(config.scan.symlink_policy, SymlinkPolicy::FollowFiles);
        assert!(config.scan.ignore_hidden_audio);
        let empty = parse(b"").expect("an empty file uses the defaults");
        assert_eq!(empty.theme, Theme::Terminal);
        assert_eq!(empty.scan.symlink_policy, SymlinkPolicy::FollowFiles);
    }

    #[test]
    fn unknown_keys_and_values_are_refused() {
        for text in [
            format!("{DEFAULT_CONFIG}\nunknown_limit = 1\n"),
            DEFAULT_CONFIG.replace("[scan]\n", "[scan]\nchord = true\n"),
            DEFAULT_CONFIG.replace("theme = \"terminal\"", "theme = \"neon\""),
            DEFAULT_CONFIG.replace("default_view = \"library\"", "default_view = \"player\""),
        ] {
            assert!(parse(text.as_bytes()).is_err(), "accepted {text}");
        }
        assert!(parse(&vec![b'#'; MAX_CONFIG_BYTES + 1]).is_err());
    }

    #[test]
    fn version_one_files_keep_their_choices_and_drop_retired_limits() {
        let config = parse(VERSION_ONE_CONFIG.as_bytes()).expect("released config loads");
        assert_eq!(config.theme, Theme::Mono);
        assert_eq!(config.default_view, DefaultView::Queue);
        assert!(config.scan.cross_mounts);
        assert_eq!(config.scan.symlink_policy, SymlinkPolicy::Ignore);
        assert!(!config.scan.ignore_hidden_audio);
        assert_eq!(config.scan.max_files, 50_000);
    }

    #[test]
    fn prototype_files_still_load() {
        let prototype = "theme = \"terminal\"\ndefault_view = \"playlists\"\ndefault_volume = 0.70\n\n[artwork]\nenabled = true\n\n[input]\nleader_timeout_ms = 250\n";
        let config = parse(prototype.as_bytes()).expect("prototype config loads");
        assert_eq!(config.default_view, DefaultView::Library);
    }
}
