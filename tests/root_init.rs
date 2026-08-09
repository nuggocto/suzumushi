// SPDX-License-Identifier: Apache-2.0

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;

use suzumushi::errors::AppError;
use suzumushi::init::initialize;
use suzumushi::locks::RootMutationLease;
use suzumushi::paths::{RootSource, discover_root_from};
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
        &paths.artwork_cache,
        &paths.logs,
        &paths.backups,
        &paths.tag_backups,
    ] {
        assert!(directory.is_dir(), "missing {}", directory.display());
    }
    assert!(paths.playlists.join("demo/README.txt").is_file());
    assert!(
        fs::read_to_string(&paths.config)
            .expect("config readable")
            .contains("max_index_bytes = 117440512")
    );
    for directory in [
        &paths.state,
        &paths.artwork_cache,
        &paths.logs,
        &paths.backups,
        &paths.tag_backups,
    ] {
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
    let lease = RootMutationLease::acquire(&root).expect("hold root writer lease");

    let error = initialize(&root).expect_err("reinitialization must contend with the writer");
    assert!(
        error
            .to_string()
            .contains("root mutation lease is already held")
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
