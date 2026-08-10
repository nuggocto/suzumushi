// SPDX-License-Identifier: Apache-2.0

//! Command-line surface for root initialization and diagnostics.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use clap::{Arg, Command};

use crate::app;
use crate::audio;
use crate::config;
use crate::display::terminal_safe;
use crate::errors::{AppError, AppResult};
use crate::init;
use crate::metadata;
use crate::paths;
use crate::scan;

/// Builds the current, intentionally small command-line interface.
#[must_use]
pub fn command() -> Command {
    Command::new("suzumushi")
        .about("A calm, fully local terminal audio player for Linux")
        .arg(
            Arg::new("root")
                .long("root")
                .value_name("PATH")
                .value_parser(clap::value_parser!(PathBuf))
                .global(true)
                .help("Use this Suzumushi root"),
        )
        .subcommand(
            Command::new("init")
                .about("Create a safe Suzumushi root")
                .arg(
                    Arg::new("path")
                        .value_name("PATH")
                        .value_parser(clap::value_parser!(PathBuf))
                        .required(true),
                ),
        )
        .subcommand(Command::new("diagnose").about("Scan a root and report its local media"))
        .subcommand(Command::new("__metadata-helper").hide(true))
        .subcommand(Command::new("__audio-decode-helper").hide(true))
}

/// Runs the process arguments and returns an application result.
///
/// # Errors
///
/// Returns a typed error when arguments, root selection, initialization,
/// configuration, scanning, or helper execution fails.
pub fn run_from<I, T>(arguments: I, current_dir: &Path) -> AppResult<()>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let matches = match command().try_get_matches_from(arguments) {
        Ok(matches) => matches,
        Err(error)
            if matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) =>
        {
            print!("{error}");
            return Ok(());
        }
        Err(error) => {
            let rendered = error.to_string();
            let message = rendered
                .strip_prefix("error: ")
                .unwrap_or(&rendered)
                .trim_end()
                .to_owned();
            return Err(AppError::InvalidArguments(message));
        }
    };
    if matches.subcommand_matches("__metadata-helper").is_some() {
        return if metadata::helper_main() == 0 {
            Ok(())
        } else {
            Err(AppError::MetadataHelper("helper reported an error".into()))
        };
    }
    if matches
        .subcommand_matches("__audio-decode-helper")
        .is_some()
    {
        return if audio::decoder_helper_main() == 0 {
            Ok(())
        } else {
            Err(AppError::Audio("decoder helper reported an error".into()))
        };
    }
    let explicit = matches.get_one::<PathBuf>("root").cloned();
    match matches.subcommand() {
        Some(("init", init_matches)) => {
            if explicit.is_some() {
                return Err(AppError::InvalidConfig("`init` takes its destination as a positional path; do not combine it with --root".into()));
            }
            let path = init_matches
                .get_one::<PathBuf>("path")
                .ok_or_else(|| AppError::InvalidConfig("init destination is required".into()))?;
            let initialized = init::initialize(path)?;
            println!("initialized {}", safe_path(&initialized.root, 4_096));
            Ok(())
        }
        Some(("diagnose", _)) => diagnose(explicit.as_deref(), current_dir),
        None => terminal_session(explicit.as_deref(), current_dir),
        Some((name, _)) => Err(AppError::InvalidConfig(format!(
            "unsupported command {name}"
        ))),
    }
}

fn terminal_session(explicit: Option<&Path>, current_dir: &Path) -> AppResult<()> {
    let selected = paths::discover_root(explicit, current_dir)?;
    let config = config::load_from(selected.descriptor(), &selected.path)?;
    app::run(&selected, &config)
}

fn diagnose(explicit: Option<&Path>, current_dir: &Path) -> AppResult<()> {
    let selected = paths::discover_root(explicit, current_dir)?;
    let config = config::load_from(selected.descriptor(), &selected.path)?;
    let index = scan::scan_from(selected.descriptor(), &selected.path, &config, 1)?;
    println!(
        "root: {}",
        safe_path(&selected.path, config.runtime.status_text_max_bytes)
    );
    println!("complete: {}", index.complete);
    println!("assets: {}", index.assets.len());
    println!("tracks: {}", index.entries.len());
    println!("playlists: {}", index.playlists.len());
    println!("warnings: {}", index.warnings.len());
    for entry in &index.entries {
        let asset = index
            .assets
            .binary_search_by_key(&entry.asset_id, |asset| asset.id)
            .ok()
            .map(|asset_index| &index.assets[asset_index]);
        let title = asset
            .and_then(|asset| asset.tags.title.as_deref())
            .map_or_else(
                || entry.search.filename.clone(),
                |title| terminal_safe(title.as_bytes(), config.scan.max_metadata_field_bytes),
            );
        let artist = asset
            .and_then(|asset| asset.tags.artist.as_deref())
            .map_or_else(
                || "unknown artist".to_owned(),
                |artist| terminal_safe(artist.as_bytes(), config.scan.max_metadata_field_bytes),
            );
        println!(
            "track: {} - {} [{}]",
            artist, title, entry.search.relative_path
        );
    }
    for warning in &index.warnings {
        println!(
            "warning: {:?}: {}: {}",
            warning.code,
            safe_path(&warning.path, config.scan.max_path_bytes),
            warning.message
        );
    }
    Ok(())
}

fn safe_path(path: &Path, max_bytes: usize) -> String {
    use std::os::unix::ffi::OsStrExt;

    terminal_safe(path.as_os_str().as_bytes(), max_bytes)
}
