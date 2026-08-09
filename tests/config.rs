// SPDX-License-Identifier: Apache-2.0

use std::fs;

use suzumushi::config;
use suzumushi::init::initialize;
use tempfile::TempDir;

#[test]
fn an_absent_config_uses_the_compiled_safe_defaults() {
    let temp = TempDir::new().expect("temporary directory");
    let paths = initialize(&temp.path().join("root")).expect("initialize root");
    fs::remove_file(&paths.config).expect("remove config fixture");

    let loaded = config::load(&paths.root).expect("missing config uses defaults");
    assert_eq!(loaded.scan.max_files, 50_000);
    assert_eq!(loaded.runtime.max_open_files, 64);
}

#[test]
fn config_reserves_scanner_scratch_with_both_indexes() {
    let text = config::DEFAULT_CONFIG.replacen(
        "process_memory_budget_bytes = 402653184",
        "process_memory_budget_bytes = 268435455",
        1,
    );

    let error = config::parse(text.as_bytes()).expect_err("scratch must fit before scanning");
    assert!(error.to_string().contains("scan/parser scratch"), "{error}");
}
