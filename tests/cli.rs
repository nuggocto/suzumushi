// SPDX-License-Identifier: Apache-2.0

use std::process::Command;

#[test]
fn help_succeeds_and_names_the_canonical_command() {
    let output = Command::new(env!("CARGO_BIN_EXE_suzumushi"))
        .arg("--help")
        .output()
        .expect("the suzumushi binary should start");

    assert!(
        output.status.success(),
        "--help failed with status {}",
        output.status
    );

    let stdout = String::from_utf8(output.stdout).expect("help output should be valid UTF-8");

    assert!(
        stdout.contains("Usage: suzumushi"),
        "help did not name the canonical command:\n{stdout}"
    );
    assert!(
        stdout.contains("--help"),
        "help did not describe its help option:\n{stdout}"
    );
    assert!(
        output.stderr.is_empty(),
        "--help wrote to stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
