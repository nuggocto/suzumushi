// SPDX-License-Identifier: Apache-2.0

//! Command-line surface for root initialization and diagnostics.

use std::ffi::OsString;
use std::io::{self, Write};
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
        .version(env!("CARGO_PKG_VERSION"))
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
        .subcommand(
            Command::new("__audio-decode-helper").hide(true).arg(
                Arg::new("position-micros")
                    .hide(true)
                    .value_parser(clap::value_parser!(u64))
                    .default_value("0"),
            ),
        )
}

/// Runs the process arguments and returns an application result.
///
/// `current_dir` is resolved only by commands that discover a root from it, so
/// helpers keep working after a session's working directory is deleted.
///
/// # Errors
///
/// Returns a typed error when arguments, the working directory, root
/// selection, initialization, configuration, scanning, or helper execution
/// fails.
pub fn run_from<I, T>(
    arguments: I,
    current_dir: impl FnOnce() -> std::io::Result<PathBuf>,
) -> AppResult<()>
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
            return print_report(|out| write!(out, "{error}"));
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
    if let Some(helper) = matches.subcommand_matches("__audio-decode-helper") {
        let position_micros = helper
            .get_one::<u64>("position-micros")
            .copied()
            .unwrap_or(0);
        return if audio::decoder_helper_main(position_micros) == 0 {
            Ok(())
        } else {
            Err(AppError::Audio("decoder helper reported an error".into()))
        };
    }
    let explicit = matches.get_one::<PathBuf>("root").cloned();
    match matches.subcommand() {
        Some(("init", init_matches)) => {
            if explicit.is_some() {
                return Err(AppError::InvalidArguments(
                    "`init` takes its destination as a positional path; do not combine it with --root"
                        .into(),
                ));
            }
            let path = init_matches
                .get_one::<PathBuf>("path")
                .ok_or_else(|| AppError::InvalidArguments("init destination is required".into()))?;
            let initialized = init::initialize(path)?;
            print_report(|out| {
                writeln!(out, "initialized {}", safe_path(&initialized.root, 4_096))?;
                writeln!(
                    out,
                    "add audio below {}",
                    safe_path(&initialized.library, 4_096)
                )?;
                writeln!(
                    out,
                    "then run `suzumushi --root <ROOT>` using the initialized path"
                )
            })
        }
        Some(("diagnose", _)) => diagnose(
            explicit.as_deref(),
            &current_dir().map_err(AppError::CurrentDirectory)?,
        ),
        None => terminal_session(
            explicit.as_deref(),
            &current_dir().map_err(AppError::CurrentDirectory)?,
        ),
        Some((name, _)) => Err(AppError::InvalidArguments(format!(
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
    print_report(|out| write_diagnosis(out, &selected.path, &config, &index))
}

/// Writes a command's report to stdout. A reader that closes the pipe early,
/// as `suzumushi diagnose | head` does, has taken all it wanted.
fn print_report(write: impl FnOnce(&mut io::StdoutLock<'_>) -> io::Result<()>) -> AppResult<()> {
    let mut out = io::stdout().lock();
    match write(&mut out).and_then(|()| out.flush()) {
        Err(error) if error.kind() != io::ErrorKind::BrokenPipe => {
            Err(AppError::io("write report", "stdout", error))
        }
        _ => Ok(()),
    }
}

fn write_diagnosis(
    out: &mut impl Write,
    root: &Path,
    config: &config::Config,
    index: &crate::model::ScanIndex,
) -> io::Result<()> {
    writeln!(
        out,
        "root: {}",
        safe_path(root, config.runtime.status_text_max_bytes)
    )?;
    writeln!(out, "complete: {}", index.complete)?;
    writeln!(out, "assets: {}", index.assets.len())?;
    writeln!(out, "tracks: {}", index.entries.len())?;
    writeln!(out, "playlists: {}", index.playlists.len())?;
    writeln!(out, "warnings: {}", index.warnings.len())?;
    if index.entries.is_empty() {
        writeln!(out, "hint: add audio below audio/library/")?;
    }
    for entry in &index.entries {
        let asset = index.asset_for_entry(entry);
        let title = asset
            .and_then(|asset| asset.tags.title.as_deref())
            .map_or_else(
                || entry.search.filename.clone(),
                |title| terminal_safe(title.as_bytes(), config.scan.max_metadata_field_bytes),
            );
        if let Some(artist) = asset.and_then(|asset| asset.tags.artist.as_deref()) {
            let artist = terminal_safe(artist.as_bytes(), config.scan.max_metadata_field_bytes);
            writeln!(
                out,
                "track: {artist} - {title} [{}]",
                entry.search.relative_path
            )?;
        } else {
            writeln!(out, "track: {title} [{}]", entry.search.relative_path)?;
        }
    }
    for warning in &index.warnings {
        writeln!(
            out,
            "warning: {:?}: {}: {}",
            warning.code,
            safe_path(&warning.path, config.scan.max_path_bytes),
            warning.message
        )?;
    }
    Ok(())
}

fn safe_path(path: &Path, max_bytes: usize) -> String {
    use std::os::unix::ffi::OsStrExt;

    terminal_safe(path.as_os_str().as_bytes(), max_bytes)
}
