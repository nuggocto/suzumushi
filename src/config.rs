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
pub(crate) const TERMINAL_BUFFER_BYTES: usize = 8 * 1_048_576;
pub(crate) const AUDIO_WORKER_BYTES: usize = 2 * 1_048_576;
pub(crate) const MAX_LOG_FILES: usize = 5;
const QUEUE_ITEM_ACCOUNTING_BYTES: usize = 32;

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
pub const DEFAULT_CONFIG: &str = r#"config_version = 1
theme = "terminal"
default_view = "library"

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

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub config_version: u32,
    pub theme: String,
    pub default_view: String,
    pub input: InputConfig,
    pub scan: ScanConfig,
    #[serde(default)]
    pub search: SearchConfig,
    #[serde(default)]
    pub queue: QueueConfig,
    pub runtime: RuntimeConfig,
    pub logging: LoggingConfig,
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
section!(SearchConfig {
    max_results: usize,
    max_query_bytes: usize
});
section!(QueueConfig {
    max_items: usize,
    max_bytes: usize
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
section!(RuntimeConfig {
    process_memory_budget_bytes: usize,
    max_open_files: usize,
    status_text_max_bytes: usize
});
section!(LoggingConfig {
    max_file_bytes: usize,
    max_files: usize,
    max_record_bytes: usize,
    queue_capacity: usize,
    queue_max_bytes: usize,
    queue_full_policy: String
});

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            max_results: 200,
            max_query_bytes: 4_096,
        }
    }
}

impl Default for QueueConfig {
    fn default() -> Self {
        Self {
            max_items: 10_000,
            max_bytes: 1_048_576,
        }
    }
}

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
    let mut document: toml::Table =
        toml::from_str(text).map_err(|error| AppError::InvalidConfig(error.to_string()))?;
    if !document.contains_key("config_version") {
        remove_released_prototype_fields(&mut document);
        document.insert("config_version".into(), toml::Value::Integer(1));
    }
    let config: Config = toml::Value::Table(document)
        .try_into()
        .map_err(|error| AppError::InvalidConfig(error.to_string()))?;
    config.validate()?;
    Ok(config)
}

fn remove_released_prototype_fields(document: &mut toml::Table) {
    for key in [
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
        "search",
        "artwork",
        "visualizer",
        "desktop",
        "tags",
        "mutation",
    ] {
        document.remove(key);
    }
    if document.get("default_view").and_then(toml::Value::as_str) == Some("playlists") {
        document.insert("default_view".into(), toml::Value::String("library".into()));
    }
    if let Some(runtime) = document
        .get_mut("runtime")
        .and_then(toml::Value::as_table_mut)
    {
        for key in [
            "command_channel_capacity",
            "event_channel_capacity",
            "position_channel_capacity",
            "visualizer_channel_capacity",
            "max_blocking_jobs",
            "max_parser_helpers",
            "tokio_worker_threads",
            "audio_prefetch_frames",
            "state_file_max_bytes",
            "shutdown_timeout_ms",
        ] {
            runtime.remove(key);
        }
    }
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

fn validate_scan(scan: &ScanConfig) -> AppResult<()> {
    inclusive("scan.max_files", scan.max_files, 1, 50_000)?;
    inclusive("scan.max_entries", scan.max_entries, 1, 100_000)?;
    if scan.max_entries < scan.max_files {
        return Err(AppError::InvalidConfig(
            "scan.max_entries must be at least scan.max_files".into(),
        ));
    }
    inclusive("scan.max_depth", scan.max_depth, 1, 16)?;
    inclusive("scan.max_playlists", scan.max_playlists, 1, 2_000)?;
    inclusive(
        "scan.max_entries_per_playlist",
        scan.max_entries_per_playlist,
        1,
        10_000,
    )?;
    inclusive(
        "scan.max_symlink_resolutions",
        scan.max_symlink_resolutions,
        1,
        50_000,
    )?;
    inclusive(
        "scan.max_parser_attempts",
        scan.max_parser_attempts,
        1,
        50_000,
    )?;
    inclusive("scan.max_path_bytes", scan.max_path_bytes, 1, 4_096)?;
    inclusive(
        "scan.max_total_path_bytes",
        scan.max_total_path_bytes,
        1,
        25_165_824,
    )?;
    inclusive(
        "scan.max_metadata_field_bytes",
        scan.max_metadata_field_bytes,
        1,
        16_384,
    )?;
    inclusive(
        "scan.max_total_metadata_bytes",
        scan.max_total_metadata_bytes,
        1,
        50_331_648,
    )?;
    inclusive(
        "scan.max_warning_bytes",
        scan.max_warning_bytes,
        1,
        8_388_608,
    )?;
    inclusive("scan.max_index_bytes", scan.max_index_bytes, 1, 117_440_512)?;
    if scan.follow_directory_symlinks {
        return Err(AppError::InvalidConfig(
            "scan.follow_directory_symlinks must be false in v1".into(),
        ));
    }
    Ok(())
}

fn validate_logging(logging: &LoggingConfig) -> AppResult<()> {
    inclusive(
        "logging.max_file_bytes",
        logging.max_file_bytes,
        1,
        10_485_760,
    )?;
    inclusive("logging.max_files", logging.max_files, 1, MAX_LOG_FILES)?;
    inclusive(
        "logging.max_record_bytes",
        logging.max_record_bytes,
        1,
        16_384,
    )?;
    inclusive("logging.queue_capacity", logging.queue_capacity, 1, 512)?;
    inclusive(
        "logging.queue_max_bytes",
        logging.queue_max_bytes,
        1,
        8_388_608,
    )?;
    one_of(
        "logging.queue_full_policy",
        &logging.queue_full_policy,
        &["drop_and_count"],
    )?;
    let queued_bytes = logging
        .queue_capacity
        .checked_mul(logging.max_record_bytes)
        .ok_or_else(|| AppError::InvalidConfig("logging queue reservation overflow".into()))?;
    if queued_bytes > logging.queue_max_bytes {
        return Err(AppError::InvalidConfig(
            "logging.queue_capacity times logging.max_record_bytes must fit logging.queue_max_bytes"
                .into(),
        ));
    }
    if logging.max_record_bytes > logging.max_file_bytes {
        return Err(AppError::InvalidConfig(
            "logging.max_record_bytes must not exceed logging.max_file_bytes".into(),
        ));
    }
    Ok(())
}

fn validate_search(search: &SearchConfig) -> AppResult<()> {
    inclusive("search.max_results", search.max_results, 1, 200)?;
    inclusive("search.max_query_bytes", search.max_query_bytes, 1, 4_096)
}

fn validate_queue(queue: &QueueConfig) -> AppResult<()> {
    inclusive("queue.max_items", queue.max_items, 1, 10_000)?;
    inclusive("queue.max_bytes", queue.max_bytes, 1, 8_388_608)?;
    let retained = queue
        .max_items
        .checked_mul(QUEUE_ITEM_ACCOUNTING_BYTES)
        .ok_or_else(|| AppError::InvalidConfig("queue reservation overflow".into()))?;
    if retained > queue.max_bytes {
        return Err(AppError::InvalidConfig(
            "queue.max_items must fit queue.max_bytes".into(),
        ));
    }
    Ok(())
}

impl Config {
    /// Enforces the exact compiled safety contract.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::InvalidConfig`] when any compiled invariant fails.
    pub fn validate(&self) -> AppResult<()> {
        if self.config_version != 1 {
            return Err(AppError::InvalidConfig(format!(
                "config_version must equal 1, got {}",
                self.config_version
            )));
        }
        one_of("theme", &self.theme, &["terminal", "mono"])?;
        one_of("default_view", &self.default_view, &["library", "queue"])?;
        inclusive(
            "input.leader_timeout_ms",
            self.input.leader_timeout_ms,
            50,
            2_000,
        )?;
        validate_scan(&self.scan)?;
        validate_search(&self.search)?;
        validate_queue(&self.queue)?;
        let r = &self.runtime;
        inclusive(
            "runtime.process_memory_budget_bytes",
            r.process_memory_budget_bytes,
            67_108_864,
            402_653_184,
        )?;
        inclusive("runtime.max_open_files", r.max_open_files, 1, 64)?;
        inclusive(
            "runtime.status_text_max_bytes",
            r.status_text_max_bytes,
            1,
            4_096,
        )?;
        validate_logging(&self.logging)?;
        let reserved_app =
            scan_reservation_bytes(self.scan.max_index_bytes, self.scan.max_index_bytes)?
                .checked_add(self.logging.queue_max_bytes)
                .and_then(|value| value.checked_add(UI_STATE_SCRATCH_BYTES))
                .and_then(|value| value.checked_add(TERMINAL_BUFFER_BYTES))
                .and_then(|value| value.checked_add(AUDIO_WORKER_BYTES))
                .and_then(|value| value.checked_add(self.queue.max_bytes))
                .ok_or_else(|| {
                    AppError::InvalidConfig("application reservation overflow".into())
                })?;
        if reserved_app > r.process_memory_budget_bytes {
            return Err(AppError::InvalidConfig(
                "indexes, scan/parser scratch, queues, terminal buffers, audio buffers, and UI/state scratch exceed process_memory_budget_bytes".into(),
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
    fn released_prototype_config_migrates_to_the_smaller_contract() {
        let legacy = replace_once(DEFAULT_CONFIG, "config_version = 1\n", "");
        let mut legacy = replace_once(
            &legacy,
            "default_view = \"library\"",
            "default_view = \"playlists\"\ndefault_volume = 0.70",
        );
        legacy = replace_once(
            &legacy,
            "max_open_files = 64",
            "command_channel_capacity = 32\nvisualizer_channel_capacity = 2\nmax_open_files = 64",
        );
        legacy.push_str("\n[artwork]\nenabled = true\n");
        let config = parse(legacy.as_bytes()).expect("released prototype config remains readable");
        assert_eq!(config.default_view, "library");

        let current = format!("{DEFAULT_CONFIG}\n[artwork]\nenabled = true\n");
        assert!(
            parse(current.as_bytes()).is_err(),
            "retired fields are accepted only by the unversioned migration"
        );
    }

    #[test]
    fn existing_version_one_configs_receive_current_search_and_queue_defaults() {
        let without_search = DEFAULT_CONFIG.replace(
            "[search]\nmax_results = 200\nmax_query_bytes = 4096\n\n",
            "",
        );
        let without_current_sections =
            without_search.replace("[queue]\nmax_items = 10000\nmax_bytes = 1048576\n\n", "");

        let config = parse(without_current_sections.as_bytes())
            .expect("known version one configs remain readable");
        assert_eq!(config.search.max_results, 200);
        assert_eq!(config.search.max_query_bytes, 4_096);
        assert_eq!(config.queue.max_items, 10_000);
        assert_eq!(config.queue.max_bytes, 1_048_576);
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
            ("max_results = 200", "max_results = 201"),
            ("max_query_bytes = 4096", "max_query_bytes = 4097"),
            ("max_items = 10000", "max_items = 10001"),
            ("max_bytes = 1048576", "max_bytes = 8388609"),
            ("max_open_files = 64", "max_open_files = 65"),
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
    fn cross_field_contradictions_fail_before_work_starts() {
        let text = replace_once(
            DEFAULT_CONFIG,
            "max_file_bytes = 10485760",
            "max_file_bytes = 16383",
        );
        assert!(parse(text.as_bytes()).is_err());

        let text = replace_once(
            DEFAULT_CONFIG,
            "max_entries = 100000",
            "max_entries = 49999",
        );
        assert!(parse(text.as_bytes()).is_err());

        let text = replace_once(
            DEFAULT_CONFIG,
            "follow_directory_symlinks = false",
            "follow_directory_symlinks = true",
        );
        assert!(parse(text.as_bytes()).is_err());

        let text = replace_once(DEFAULT_CONFIG, "max_bytes = 1048576", "max_bytes = 319999");
        assert!(parse(text.as_bytes()).is_err());
    }
}
