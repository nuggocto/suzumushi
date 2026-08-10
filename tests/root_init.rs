// SPDX-License-Identifier: Apache-2.0

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::Path;

use suzumushi::errors::AppError;
use suzumushi::init::initialize;
use suzumushi::locks::RootWriterLease;
use suzumushi::paths::{RootSource, discover_root_from};
use suzumushi::{config, scan};
use tempfile::TempDir;

fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path)
        .expect("fixture path exists")
        .permissions()
        .mode()
        & 0o777
}

#[test]
fn init_creates_the_documented_tree_with_private_storage() {
    let temp = TempDir::new().expect("temporary directory");
    let root = temp.path().join("music root");
    let paths = initialize(&root).expect("root initialization succeeds");

    for directory in [
        &paths.library,
        &paths.playlists.join("demo"),
        &paths.state,
        &paths.logs,
    ] {
        assert!(directory.is_dir(), "missing {}", directory.display());
    }
    assert!(paths.playlists.join("demo/README.txt").is_file());
    assert!(
        fs::read_to_string(&paths.config)
            .expect("config readable")
            .contains("max_index_bytes = 117440512")
    );
    for directory in [&paths.state, &paths.logs] {
        assert_eq!(
            mode(directory),
            0o700,
            "wrong mode for {}",
            directory.display()
        );
    }
    assert_eq!(mode(&paths.config), 0o600);

    initialize(&root).expect("initialization is idempotent for an owned root");
}

#[test]
fn init_refuses_symlinks_unrelated_contents_and_exposed_private_storage() {
    let temp = TempDir::new().expect("temporary directory");
    let real = temp.path().join("real");
    fs::create_dir(&real).expect("create real directory");
    let linked = temp.path().join("linked");
    symlink(&real, &linked).expect("create symlink");
    assert!(matches!(
        initialize(&linked),
        Err(AppError::InitRefused { .. })
    ));

    let unrelated = temp.path().join("unrelated");
    fs::create_dir(&unrelated).expect("create destination");
    fs::write(unrelated.join("notes.txt"), b"mine").expect("create unrelated file");
    assert!(matches!(
        initialize(&unrelated),
        Err(AppError::InitRefused { .. })
    ));

    let exposed = temp.path().join("exposed");
    let paths = initialize(&exposed).expect("initialize fixture");
    fs::set_permissions(&paths.state, fs::Permissions::from_mode(0o755))
        .expect("expose state fixture");
    let error = initialize(&exposed).expect_err("insecure private storage is refused");
    assert!(error.to_string().contains("mode 700"));

    let unsafe_lock = temp.path().join("unsafe-lock");
    initialize(&unsafe_lock).expect("initialize lock fixture");
    fs::set_permissions(
        unsafe_lock.join(".suzumushi-root.lock"),
        fs::Permissions::from_mode(0o644),
    )
    .expect("expose root lock fixture");
    let error = initialize(&unsafe_lock).expect_err("unsafe root lock is refused");
    assert!(error.to_string().contains("mode 0600"));
}

#[test]
fn nested_directory_failure_reports_its_exact_path() {
    let temp = TempDir::new().expect("temporary directory");
    let root = temp.path().join("root");
    let paths = initialize(&root).expect("initialize fixture");
    let demo = paths.playlists.join("demo");
    fs::remove_file(demo.join("README.txt")).expect("remove README fixture");
    fs::remove_dir(&demo).expect("remove demo directory");
    fs::write(&demo, b"not a directory").expect("replace demo with a file");

    let error = initialize(&root).expect_err("nested file blocks initialization");
    match error {
        AppError::Io { path, .. } => assert_eq!(path, demo),
        other => panic!("expected an I/O error for {demo:?}, got {other}"),
    }
}

#[test]
fn reinitialization_respects_the_root_writer_lease() {
    let temp = TempDir::new().expect("temporary directory");
    let root = temp.path().join("root");
    let paths = initialize(&root).expect("initialize fixture");
    let readme = paths.playlists.join("demo/README.txt");
    fs::remove_file(&readme).expect("remove owned file");
    let lease = RootWriterLease::acquire(&root).expect("hold root writer lease");

    let error = initialize(&root).expect_err("reinitialization must contend with the writer");
    assert!(
        error
            .to_string()
            .contains("root writer lease is already held")
    );
    assert!(
        !readme.exists(),
        "a failed reinitialization must not mutate the root"
    );

    drop(lease);
    initialize(&root).expect("reinitialization succeeds after the lease is released");
    assert!(readme.is_file());
}

#[test]
fn root_precedence_never_falls_through_an_invalid_higher_source() {
    let temp = TempDir::new().expect("temporary directory");
    let cli = initialize(&temp.path().join("cli")).expect("cli root");
    let environment = initialize(&temp.path().join("environment")).expect("environment root");
    let working = initialize(&temp.path().join("suzumushi")).expect("working root");

    let selected = discover_root_from(Some(&cli.root), Some(&environment.root), temp.path())
        .expect("cli wins");
    assert_eq!(selected.source, RootSource::CommandLine);
    assert_eq!(selected.path, cli.root);
    let selected =
        discover_root_from(None, Some(&environment.root), temp.path()).expect("environment wins");
    assert_eq!(selected.source, RootSource::Environment);
    let selected = discover_root_from(None, None, temp.path()).expect("working convention wins");
    assert_eq!(selected.source, RootSource::WorkingDirectory);
    assert_eq!(selected.path, working.root);

    let empty = TempDir::new().expect("empty working directory");
    let selected = discover_root_from(Some(&cli.root), None, empty.path()).expect("cli alone");
    assert_eq!(selected.source, RootSource::CommandLine);
    let selected =
        discover_root_from(None, Some(&environment.root), empty.path()).expect("environment alone");
    assert_eq!(selected.source, RootSource::Environment);
    assert!(matches!(
        discover_root_from(None, None, empty.path()),
        Err(AppError::RootMissing)
    ));

    let missing = temp.path().join("missing");
    assert!(matches!(
        discover_root_from(Some(&missing), Some(&environment.root), temp.path()),
        Err(AppError::InvalidRoot { .. })
    ));
    assert!(matches!(
        discover_root_from(None, Some(&missing), temp.path()),
        Err(AppError::InvalidRoot { .. })
    ));
    let linked = temp.path().join("linked-root");
    symlink(&working.root, &linked).expect("symlinked selected root");
    assert!(matches!(
        discover_root_from(Some(&linked), None, empty.path()),
        Err(AppError::InvalidRoot { .. })
    ));
}

#[test]
fn selected_root_stays_pinned_when_its_path_is_replaced() {
    let temp = TempDir::new().expect("temporary directory");
    let root = temp.path().join("root");
    let original = initialize(&root).expect("initialize original root");
    fs::write(original.library.join("original.mp3"), b"").expect("write original media fixture");
    let original_config = config::DEFAULT_CONFIG.replacen("max_files = 50000", "max_files = 1", 1);
    fs::write(&original.config, original_config).expect("write distinctive original config");
    let selected =
        discover_root_from(Some(&root), None, temp.path()).expect("select original root");

    let held = temp.path().join("held-root");
    fs::rename(&root, &held).expect("move selected root aside");
    let replacement = initialize(&root).expect("initialize replacement root");
    fs::write(replacement.library.join("replacement.mp3"), b"")
        .expect("write replacement media fixture");

    let held_metadata = fs::metadata(&held).expect("inspect held root");
    assert_eq!(selected.identity.device, held_metadata.dev());
    assert_eq!(selected.identity.inode, held_metadata.ino());
    let loaded = config::load_from(selected.descriptor(), &selected.path)
        .expect("load config through selected descriptor");
    assert_eq!(loaded.scan.max_files, 1);
    let index = scan::scan_from(selected.descriptor(), &selected.path, &loaded, 1)
        .expect("scan through selected descriptor");
    assert_eq!(index.entries.len(), 1);
    assert_eq!(index.entries[0].search.filename, "original");
    assert_ne!(index.entries[0].search.filename, "replacement");
}
