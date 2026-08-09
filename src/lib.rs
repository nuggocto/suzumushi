// SPDX-License-Identifier: Apache-2.0

//! Suzumushi root, configuration, locking, and scanner library.

pub mod cli;
pub mod config;
pub mod display;
pub mod errors;
pub mod init;
pub mod locks;
pub mod metadata;
pub mod model;
pub mod paths;
pub mod scan;

/// Parses the process command line and returns a conventional exit status.
#[must_use]
pub fn run() -> std::process::ExitCode {
    let current_dir = match std::env::current_dir() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("error: cannot determine current directory: {error}");
            return std::process::ExitCode::from(2);
        }
    };
    match cli::run_from(std::env::args_os(), &current_dir) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            let message = display::terminal_safe(error.to_string().as_bytes(), 16_384);
            eprintln!("error: {message}");
            std::process::ExitCode::from(2)
        }
    }
}
