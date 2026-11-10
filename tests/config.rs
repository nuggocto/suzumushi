// SPDX-License-Identifier: Apache-2.0

use std::fs;

use suzumushi::config;
use suzumushi::init::initialize;
use tempfile::TempDir;

#[test]
fn an_absent_config_uses_the_compiled_defaults() {
    let temp = TempDir::new().expect("temporary directory");
    let paths = initialize(&temp.path().join("root")).expect("initialize root");
    fs::remove_file(&paths.config).expect("remove config fixture");

    let loaded = config::load(&paths.root).expect("missing config uses defaults");
    assert_eq!(loaded.theme, config::Theme::Terminal);
    assert_eq!(loaded.scan.max_files, 50_000);
}

#[test]
fn the_config_init_writes_loads_back_unchanged() {
    let temp = TempDir::new().expect("temporary directory");
    let paths = initialize(&temp.path().join("root")).expect("initialize root");
    assert_eq!(
        fs::read_to_string(&paths.config).expect("read config"),
        config::DEFAULT_CONFIG
    );
    let loaded = config::load(&paths.root).expect("load written config");
    assert_eq!(loaded.default_view, config::DefaultView::Library);
}
