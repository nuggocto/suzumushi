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
fn one_index_and_metadata_cache_fit_a_192_mib_process_budget() {
    let mut config = config::Config::default();
    config.runtime.process_memory_budget_bytes = 192 * 1_048_576;
    config.validate().expect("one index and cache fit");
    config.runtime.process_memory_budget_bytes = 160 * 1_048_576;
    let error = config
        .validate()
        .expect_err("all application reservations must fit");
    assert!(error.to_string().contains("UI/state scratch"));
}
