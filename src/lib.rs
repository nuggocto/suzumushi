// SPDX-License-Identifier: Apache-2.0

//! Suzumushi application entry points.

use clap::Command;

fn command() -> Command {
    Command::new("suzumushi").about("A calm, fully local terminal audio player for Linux")
}

/// Parses the process command line and handles standard CLI output.
pub fn run() {
    let _matches = command().get_matches();
}
