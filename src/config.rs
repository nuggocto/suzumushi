// SPDX-License-Identifier: Apache-2.0

//! Strict, bounded configuration parsing.

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

pub(crate) const SCAN_PARSER_SCRATCH_BYTES: usize = 32 * 1_048_576;
pub(crate) const UI_STATE_SCRATCH_BYTES: usize = 8 * 1_048_576;
pub(crate) const MAX_LOG_FILES: usize = 5;

pub(crate) fn scan_reservation_bytes(
    active_index_bytes: usize,
    replacement_index_bytes: usize,
) -> AppResult<usize> {
    active_index_bytes
        .checked_add(replacement_index_bytes)
        .and_then(|value| value.checked_add(SCAN_PARSER_SCRATCH_BYTES))
        .ok_or_else(|| AppError::InvalidConfig("scan reservation overflow".into()))
}

/// The default configuration written by `init`.
pub const DEFAULT_CONFIG: &str = r#"theme = "terminal"
default_view = "playlists"
default_volume = 0.70
volume_step = 0.05
speed_min = 0.50
speed_max = 2.00
speed_step = 0.25
seek_short_seconds = 5
seek_long_seconds = 30
shuffle = false
repeat = "off"
queue_max_items = 10000
queue_max_bytes = 50331648
queue_max_history_items = 1000

[input]
leader_timeout_ms = 250

[scan]
max_files = 50000
max_entries = 100000
max_depth = 16
max_playlists = 2000
max_entries_per_playlist = 10000
max_symlink_resolutions = 50000
max_parser_attempts = 50000
cross_mounts = false
max_path_bytes = 4096
max_total_path_bytes = 25165824
max_metadata_field_bytes = 16384
max_total_metadata_bytes = 50331648
max_warning_bytes = 8388608
max_index_bytes = 117440512
follow_file_symlinks = true
follow_directory_symlinks = false
ignore_hidden_audio = true

[search]
enabled = true
max_results = 200
max_query_bytes = 4096
music_search_key = "/"
palette_key = "space slash"
metadata_weight = 3
filename_weight = 1

[artwork]
enabled = true
terminal_images = true
max_decode_bytes = 33554432
max_cache_bytes = 33554432
max_source_bytes = 33554432
max_width_px = 4096
max_height_px = 4096
preferred_size_px = 512

[visualizer]
enabled = true
style = "suzu"
bars = 16
height = 1
update_hz = 10
falloff = "soft"

[desktop]
mpris = true
notifications = true
notification_timeout_ms = 5000
status_write_coalesce_ms = 250

[runtime]
process_memory_budget_bytes = 402653184
command_channel_capacity = 32
event_channel_capacity = 64
position_channel_capacity = 1
visualizer_channel_capacity = 2
max_open_files = 64
max_blocking_jobs = 4
max_parser_helpers = 2
tokio_worker_threads = 2
audio_prefetch_frames = 16384
state_file_max_bytes = 1048576
status_text_max_bytes = 4096
shutdown_timeout_ms = 5000

[logging]
max_file_bytes = 10485760
max_files = 5
max_record_bytes = 16384
queue_capacity = 512
queue_max_bytes = 8388608
queue_full_policy = "drop_and_count"

[tags]
editing = true
batch_editing = true
backup_budget_bytes = 2147483648
backup_max_entries = 10000
max_batch_files = 200
max_field_bytes = 16384
refuse_loaded_audio_file = true

[mutation]
journal_record_max_bytes = 1048576
journal_max_entries = 10000
journal_max_total_bytes = 67108864
startup_recovery_max_records = 10000
"#;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub theme: String,
    pub default_view: String,
    pub default_volume: f64,
    pub volume_step: f64,
    pub speed_min: f64,
    pub speed_max: f64,
    pub speed_step: f64,
    pub seek_short_seconds: u64,
    pub seek_long_seconds: u64,
    pub shuffle: bool,
    pub repeat: String,
    pub queue_max_items: usize,
    pub queue_max_bytes: usize,
    pub queue_max_history_items: usize,
    pub input: InputConfig,
    pub scan: ScanConfig,
    pub search: SearchConfig,
    pub artwork: ArtworkConfig,
    pub visualizer: VisualizerConfig,
    pub desktop: DesktopConfig,
    pub runtime: RuntimeConfig,
    pub logging: LoggingConfig,
    pub tags: TagsConfig,
    pub mutation: MutationConfig,
}

macro_rules! section {
    ($name:ident { $($field:ident : $ty:ty),+ $(,)? }) => {
        #[derive(Clone, Debug, Deserialize)]
        #[serde(deny_unknown_fields)]
        pub struct $name { $(pub $field: $ty),+ }
    };
}

section!(InputConfig {
    leader_timeout_ms: u64
});
section!(ScanConfig {
    max_files: usize,
    max_entries: usize,
    max_depth: usize,
    max_playlists: usize,
    max_entries_per_playlist: usize,
    max_symlink_resolutions: usize,
    max_parser_attempts: usize,
    cross_mounts: bool,
    max_path_bytes: usize,
    max_total_path_bytes: usize,
    max_metadata_field_bytes: usize,
    max_total_metadata_bytes: usize,
    max_warning_bytes: usize,
    max_index_bytes: usize,
    follow_file_symlinks: bool,
    follow_directory_symlinks: bool,
    ignore_hidden_audio: bool
});
section!(SearchConfig {
    enabled: bool,
    max_results: usize,
    max_query_bytes: usize,
    music_search_key: String,
    palette_key: String,
    metadata_weight: u8,
    filename_weight: u8
});
section!(ArtworkConfig {
    enabled: bool,
    terminal_images: bool,
    max_decode_bytes: usize,
    max_cache_bytes: usize,
    max_source_bytes: usize,
    max_width_px: usize,
    max_height_px: usize,
    preferred_size_px: usize
});
section!(VisualizerConfig {
    enabled: bool,
    style: String,
    bars: usize,
    height: usize,
    update_hz: usize,
    falloff: String
});
section!(DesktopConfig {
    mpris: bool,
    notifications: bool,
    notification_timeout_ms: u64,
    status_write_coalesce_ms: u64
});
section!(RuntimeConfig {
    process_memory_budget_bytes: usize,
    command_channel_capacity: usize,
    event_channel_capacity: usize,
    position_channel_capacity: usize,
    visualizer_channel_capacity: usize,
    max_open_files: usize,
    max_blocking_jobs: usize,
    max_parser_helpers: usize,
    tokio_worker_threads: usize,
    audio_prefetch_frames: usize,
    state_file_max_bytes: usize,
    status_text_max_bytes: usize,
    shutdown_timeout_ms: u64
});
section!(LoggingConfig {
    max_file_bytes: usize,
    max_files: usize,
    max_record_bytes: usize,
    queue_capacity: usize,
    queue_max_bytes: usize,
    queue_full_policy: String
});
section!(TagsConfig {
    editing: bool,
    batch_editing: bool,
    backup_budget_bytes: u64,
    backup_max_entries: usize,
    max_batch_files: usize,
    max_field_bytes: usize,
    refuse_loaded_audio_file: bool
});
section!(MutationConfig {
    journal_record_max_bytes: usize,
    journal_max_entries: usize,
    journal_max_total_bytes: usize,
    startup_recovery_max_records: usize
});

/// Parses a bounded TOML document and validates every compiled range.
///
/// # Errors
///
/// Returns [`AppError::InvalidConfig`] for oversized, malformed, unknown,
/// contradictory, or out-of-range input.
pub fn parse(input: &[u8]) -> AppResult<Config> {
    if input.len() > MAX_CONFIG_BYTES {
        return Err(AppError::InvalidConfig(format!(
            "config exceeds {MAX_CONFIG_BYTES} bytes"
        )));
    }
    let text = std::str::from_utf8(input)
        .map_err(|error| AppError::InvalidConfig(format!("config is not UTF-8: {error}")))?;
    let config: Config =
        toml::from_str(text).map_err(|error| AppError::InvalidConfig(error.to_string()))?;
    config.validate()?;
    Ok(config)
}

/// Loads `config.toml` from a pinned root without following the file target.
///
/// # Errors
///
/// Returns safe compiled defaults when the file is absent. Returns an error
/// when an existing file cannot be opened safely or its contents do not validate.
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
/// Returns an error for unsafe file identity, ownership, modes, size, syntax,
/// or values.
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

#[allow(clippy::needless_pass_by_value)]
fn inclusive<T: PartialOrd + std::fmt::Display>(
    name: &str,
    value: T,
    min: T,
    max: T,
) -> AppResult<()> {
    if value < min || value > max {
        return Err(AppError::InvalidConfig(format!(
            "{name} must be in {min}..={max}, got {value}"
        )));
    }
    Ok(())
}

fn one_of(name: &str, value: &str, accepted: &[&str]) -> AppResult<()> {
    if !accepted.contains(&value) {
        return Err(AppError::InvalidConfig(format!(
            "{name} must be one of {}, got {value:?}",
            accepted.join(", ")
        )));
    }
    Ok(())
}

fn key(name: &str, value: &str) -> AppResult<()> {
    if value.is_empty() || value.len() > 64 || value.chars().any(char::is_control) {
        return Err(AppError::InvalidConfig(format!(
            "{name} must be one non-control chord of 1..=64 bytes"
        )));
    }
    Ok(())
}

impl Config {
    /// Enforces the exact compiled safety contract.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::InvalidConfig`] when any compiled invariant fails.
    #[allow(clippy::too_many_lines)]
    pub fn validate(&self) -> AppResult<()> {
        one_of("theme", &self.theme, &["terminal", "mono"])?;
        one_of(
            "default_view",
            &self.default_view,
            &["library", "playlists", "queue"],
        )?;
        for (name, value, min, max) in [
            ("default_volume", self.default_volume, 0.0, 1.0),
            ("volume_step", self.volume_step, 0.01, 1.0),
            ("speed_min", self.speed_min, 0.5, 2.0),
            ("speed_max", self.speed_max, 0.5, 2.0),
            ("speed_step", self.speed_step, 0.01, 1.5),
        ] {
            if !value.is_finite() {
                return Err(AppError::InvalidConfig(format!("{name} must be finite")));
            }
            inclusive(name, value, min, max)?;
        }
        if self.speed_min > 1.0 || self.speed_max < 1.0 || self.speed_min > self.speed_max {
            return Err(AppError::InvalidConfig(
                "speed_min <= 1.0 <= speed_max is required".into(),
            ));
        }
        inclusive("seek_short_seconds", self.seek_short_seconds, 1, 3600)?;
        inclusive("seek_long_seconds", self.seek_long_seconds, 1, 3600)?;
        if self.seek_short_seconds > self.seek_long_seconds {
            return Err(AppError::InvalidConfig(
                "seek_short_seconds must not exceed seek_long_seconds".into(),
            ));
        }
        one_of("repeat", &self.repeat, &["off", "one", "queue"])?;
        inclusive("queue_max_items", self.queue_max_items, 1, 10_000)?;
        inclusive("queue_max_bytes", self.queue_max_bytes, 1, 50_331_648)?;
        inclusive(
            "queue_max_history_items",
            self.queue_max_history_items,
            1,
            1_000,
        )?;
        if self.queue_max_history_items > self.queue_max_items {
            return Err(AppError::InvalidConfig(
                "queue_max_history_items must not exceed queue_max_items".into(),
            ));
        }
        inclusive(
            "input.leader_timeout_ms",
            self.input.leader_timeout_ms,
            50,
            2_000,
        )?;
        let s = &self.scan;
        inclusive("scan.max_files", s.max_files, 1, 50_000)?;
        inclusive("scan.max_entries", s.max_entries, 1, 100_000)?;
        if s.max_entries < s.max_files {
            return Err(AppError::InvalidConfig(
                "scan.max_entries must be at least scan.max_files".into(),
            ));
        }
        inclusive("scan.max_depth", s.max_depth, 1, 16)?;
        inclusive("scan.max_playlists", s.max_playlists, 1, 2_000)?;
        inclusive(
            "scan.max_entries_per_playlist",
            s.max_entries_per_playlist,
            1,
            10_000,
        )?;
        inclusive(
            "scan.max_symlink_resolutions",
            s.max_symlink_resolutions,
            1,
            50_000,
        )?;
        inclusive("scan.max_parser_attempts", s.max_parser_attempts, 1, 50_000)?;
        inclusive("scan.max_path_bytes", s.max_path_bytes, 1, 4_096)?;
        inclusive(
            "scan.max_total_path_bytes",
            s.max_total_path_bytes,
            1,
            25_165_824,
        )?;
        inclusive(
            "scan.max_metadata_field_bytes",
            s.max_metadata_field_bytes,
            1,
            16_384,
        )?;
        inclusive(
            "scan.max_total_metadata_bytes",
            s.max_total_metadata_bytes,
            1,
            50_331_648,
        )?;
        inclusive("scan.max_warning_bytes", s.max_warning_bytes, 1, 8_388_608)?;
        inclusive("scan.max_index_bytes", s.max_index_bytes, 1, 117_440_512)?;
        if s.follow_directory_symlinks {
            return Err(AppError::InvalidConfig(
                "scan.follow_directory_symlinks must be false in v1".into(),
            ));
        }
        inclusive("search.max_results", self.search.max_results, 1, 200)?;
        inclusive(
            "search.max_query_bytes",
            self.search.max_query_bytes,
            1,
            4_096,
        )?;
        inclusive("search.metadata_weight", self.search.metadata_weight, 0, 16)?;
        inclusive("search.filename_weight", self.search.filename_weight, 0, 16)?;
        if self.search.metadata_weight == 0 && self.search.filename_weight == 0 {
            return Err(AppError::InvalidConfig(
                "one search weight must be non-zero".into(),
            ));
        }
        key("search.music_search_key", &self.search.music_search_key)?;
        key("search.palette_key", &self.search.palette_key)?;
        let a = &self.artwork;
        inclusive(
            "artwork.max_source_bytes",
            a.max_source_bytes,
            1,
            33_554_432,
        )?;
        inclusive(
            "artwork.max_decode_bytes",
            a.max_decode_bytes,
            1,
            33_554_432,
        )?;
        inclusive("artwork.max_cache_bytes", a.max_cache_bytes, 1, 33_554_432)?;
        inclusive("artwork.max_width_px", a.max_width_px, 1, 4_096)?;
        inclusive("artwork.max_height_px", a.max_height_px, 1, 4_096)?;
        inclusive(
            "artwork.preferred_size_px",
            a.preferred_size_px,
            1,
            a.max_width_px.min(a.max_height_px),
        )?;
        one_of(
            "visualizer.style",
            &self.visualizer.style,
            &["suzu", "bars"],
        )?;
        one_of(
            "visualizer.falloff",
            &self.visualizer.falloff,
            &["soft", "none"],
        )?;
        inclusive("visualizer.bars", self.visualizer.bars, 1, 32)?;
        inclusive("visualizer.height", self.visualizer.height, 1, 4)?;
        inclusive("visualizer.update_hz", self.visualizer.update_hz, 1, 12)?;
        inclusive(
            "desktop.notification_timeout_ms",
            self.desktop.notification_timeout_ms,
            100,
            60_000,
        )?;
        inclusive(
            "desktop.status_write_coalesce_ms",
            self.desktop.status_write_coalesce_ms,
            50,
            1_000,
        )?;
        let r = &self.runtime;
        inclusive(
            "runtime.process_memory_budget_bytes",
            r.process_memory_budget_bytes,
            67_108_864,
            402_653_184,
        )?;
        inclusive(
            "runtime.command_channel_capacity",
            r.command_channel_capacity,
            1,
            32,
        )?;
        inclusive(
            "runtime.event_channel_capacity",
            r.event_channel_capacity,
            1,
            64,
        )?;
        if r.position_channel_capacity != 1 {
            return Err(AppError::InvalidConfig(
                "runtime.position_channel_capacity must equal 1".into(),
            ));
        }
        inclusive(
            "runtime.visualizer_channel_capacity",
            r.visualizer_channel_capacity,
            1,
            2,
        )?;
        inclusive("runtime.max_open_files", r.max_open_files, 1, 64)?;
        inclusive("runtime.max_blocking_jobs", r.max_blocking_jobs, 1, 4)?;
        inclusive("runtime.max_parser_helpers", r.max_parser_helpers, 1, 2)?;
        inclusive("runtime.tokio_worker_threads", r.tokio_worker_threads, 1, 2)?;
        inclusive(
            "runtime.audio_prefetch_frames",
            r.audio_prefetch_frames,
            1_024,
            16_384,
        )?;
        inclusive(
            "runtime.state_file_max_bytes",
            r.state_file_max_bytes,
            1,
            1_048_576,
        )?;
        inclusive(
            "runtime.status_text_max_bytes",
            r.status_text_max_bytes,
            1,
            4_096,
        )?;
        inclusive(
            "runtime.shutdown_timeout_ms",
            r.shutdown_timeout_ms,
            100,
            5_000,
        )?;
        let l = &self.logging;
        inclusive("logging.max_file_bytes", l.max_file_bytes, 1, 10_485_760)?;
        inclusive("logging.max_files", l.max_files, 1, MAX_LOG_FILES)?;
        inclusive("logging.max_record_bytes", l.max_record_bytes, 1, 16_384)?;
        inclusive("logging.queue_capacity", l.queue_capacity, 1, 512)?;
        inclusive("logging.queue_max_bytes", l.queue_max_bytes, 1, 8_388_608)?;
        one_of(
            "logging.queue_full_policy",
            &l.queue_full_policy,
            &["drop_and_count"],
        )?;
        let queued_log_bytes = l
            .queue_capacity
            .checked_mul(l.max_record_bytes)
            .ok_or_else(|| AppError::InvalidConfig("logging queue reservation overflow".into()))?;
        if queued_log_bytes > l.queue_max_bytes {
            return Err(AppError::InvalidConfig(
                "logging.queue_capacity times logging.max_record_bytes must fit logging.queue_max_bytes"
                    .into(),
            ));
        }
        if l.max_record_bytes > l.max_file_bytes {
            return Err(AppError::InvalidConfig(
                "logging.max_record_bytes must not exceed logging.max_file_bytes".into(),
            ));
        }
        inclusive(
            "tags.backup_budget_bytes",
            self.tags.backup_budget_bytes,
            1,
            2_147_483_648,
        )?;
        inclusive(
            "tags.backup_max_entries",
            self.tags.backup_max_entries,
            1,
            10_000,
        )?;
        inclusive("tags.max_batch_files", self.tags.max_batch_files, 1, 200)?;
        inclusive("tags.max_field_bytes", self.tags.max_field_bytes, 1, 16_384)?;
        inclusive(
            "mutation.journal_record_max_bytes",
            self.mutation.journal_record_max_bytes,
            1,
            1_048_576,
        )?;
        inclusive(
            "mutation.journal_max_entries",
            self.mutation.journal_max_entries,
            1,
            10_000,
        )?;
        inclusive(
            "mutation.journal_max_total_bytes",
            self.mutation.journal_max_total_bytes,
            1,
            67_108_864,
        )?;
        inclusive(
            "mutation.startup_recovery_max_records",
            self.mutation.startup_recovery_max_records,
            1,
            10_000,
        )?;
        let reserved_app = scan_reservation_bytes(s.max_index_bytes, s.max_index_bytes)?
            .checked_add(l.queue_max_bytes)
            .and_then(|value| value.checked_add(UI_STATE_SCRATCH_BYTES))
            .ok_or_else(|| AppError::InvalidConfig("application reservation overflow".into()))?;
        if reserved_app > r.process_memory_budget_bytes {
            return Err(AppError::InvalidConfig(
                "indexes, scan/parser scratch, logging queue, and UI/state scratch exceed process_memory_budget_bytes".into(),
            ));
        }
        Ok(())
    }
}

impl Default for Config {
    fn default() -> Self {
        parse(DEFAULT_CONFIG.as_bytes()).expect("compiled default config must remain valid")
    }
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_CONFIG, MAX_CONFIG_BYTES, parse};

    fn replace_once(source: &str, old: &str, new: &str) -> String {
        assert_eq!(
            source.matches(old).count(),
            1,
            "test replacement must be unique: {old}"
        );
        source.replacen(old, new, 1)
    }

    #[test]
    fn default_config_is_strict_and_valid() {
        parse(DEFAULT_CONFIG.as_bytes()).expect("compiled default validates");
        let unknown = format!("{DEFAULT_CONFIG}\nunknown_limit = 1\n");
        assert!(parse(unknown.as_bytes()).is_err());
        assert!(parse(&vec![b'x'; MAX_CONFIG_BYTES + 1]).is_err());
    }

    #[test]
    fn current_scan_and_runtime_bounds_reject_limit_plus_one() {
        let invalid_replacements = [
            ("max_files = 50000", "max_files = 50001"),
            ("max_entries = 100000", "max_entries = 100001"),
            ("max_depth = 16", "max_depth = 17"),
            ("max_playlists = 2000", "max_playlists = 2001"),
            (
                "max_entries_per_playlist = 10000",
                "max_entries_per_playlist = 10001",
            ),
            (
                "max_symlink_resolutions = 50000",
                "max_symlink_resolutions = 50001",
            ),
            ("max_parser_attempts = 50000", "max_parser_attempts = 50001"),
            ("max_path_bytes = 4096", "max_path_bytes = 4097"),
            (
                "max_total_path_bytes = 25165824",
                "max_total_path_bytes = 25165825",
            ),
            (
                "max_metadata_field_bytes = 16384",
                "max_metadata_field_bytes = 16385",
            ),
            (
                "max_total_metadata_bytes = 50331648",
                "max_total_metadata_bytes = 50331649",
            ),
            ("max_warning_bytes = 8388608", "max_warning_bytes = 8388609"),
            ("max_index_bytes = 117440512", "max_index_bytes = 117440513"),
            ("max_open_files = 64", "max_open_files = 65"),
            ("max_parser_helpers = 2", "max_parser_helpers = 3"),
            (
                "process_memory_budget_bytes = 402653184",
                "process_memory_budget_bytes = 402653185",
            ),
        ];
        for (valid, invalid) in invalid_replacements {
            let config = replace_once(DEFAULT_CONFIG, valid, invalid);
            assert!(parse(config.as_bytes()).is_err(), "accepted {invalid}");
        }
    }

    #[test]
    fn contradictions_and_non_finite_values_fail_before_work_starts() {
        for (old, new) in [
            ("default_volume = 0.70", "default_volume = nan"),
            ("speed_min = 0.50", "speed_min = 1.25"),
            ("seek_short_seconds = 5", "seek_short_seconds = 31"),
            (
                "queue_max_history_items = 1000",
                "queue_max_history_items = 10001",
            ),
            ("metadata_weight = 3", "metadata_weight = 0"),
        ] {
            let mut text = replace_once(DEFAULT_CONFIG, old, new);
            if new == "metadata_weight = 0" {
                text = replace_once(&text, "filename_weight = 1", "filename_weight = 0");
            }
            assert!(
                parse(text.as_bytes()).is_err(),
                "accepted contradictory {new}"
            );
        }

        let text = replace_once(
            DEFAULT_CONFIG,
            "queue_max_bytes = 8388608",
            "queue_max_bytes = 8388607",
        );
        assert!(parse(text.as_bytes()).is_err());

        let text = replace_once(
            DEFAULT_CONFIG,
            "max_file_bytes = 10485760",
            "max_file_bytes = 16383",
        );
        assert!(parse(text.as_bytes()).is_err());
    }
}
