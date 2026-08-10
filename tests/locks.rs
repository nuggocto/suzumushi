// SPDX-License-Identifier: Apache-2.0

use std::fs;
use std::os::unix::fs::PermissionsExt;

use suzumushi::init::initialize;
use suzumushi::locks::{ActiveTuiLease, RootWriterLease};
use tempfile::TempDir;

#[test]
fn root_writer_lease_is_exclusive_and_stale_file_is_reused() {
    let temp = TempDir::new().expect("temporary directory");
    let root = initialize(&temp.path().join("root")).expect("initialize root");
    let first = RootWriterLease::acquire(&root.root).expect("first lease");
    assert!(RootWriterLease::acquire(&root.root).is_err());
    drop(first);
    RootWriterLease::acquire(&root.root).expect("stale lock file is safely reused");
}

#[test]
fn one_runtime_lease_contends_across_sessions_roots_and_readers() {
    let temp = TempDir::new().expect("temporary directory");
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))
        .expect("private runtime fixture");
    let root_a = initialize(&temp.path().join("root-a")).expect("root a");
    let root_b = initialize(&temp.path().join("root-b")).expect("root b");
    let global = ActiveTuiLease::acquire_in(Some(temp.path())).expect("first login session");
    assert!(
        ActiveTuiLease::acquire_in(Some(temp.path())).is_err(),
        "second session must contend"
    );

    RootWriterLease::acquire(&root_a.root).expect("different root lease remains independent");
    RootWriterLease::acquire(&root_b.root).expect("second root lease remains independent");
    suzumushi::config::load(&root_a.root).expect("read-only config access remains concurrent");
    drop(global);
    ActiveTuiLease::acquire_in(Some(temp.path())).expect("released global lease can be reacquired");
}

#[test]
fn runtime_lock_fails_closed_without_secure_storage() {
    assert!(ActiveTuiLease::acquire_in(None).is_err());
    let temp = TempDir::new().expect("temporary directory");
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o755))
        .expect("insecure runtime fixture");
    assert!(ActiveTuiLease::acquire_in(Some(temp.path())).is_err());
}

#[test]
fn mutable_session_leases_release_during_unwinding() {
    let temp = TempDir::new().expect("temporary directory");
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))
        .expect("private runtime fixture");
    let root = initialize(&temp.path().join("root")).expect("initialize root");

    let unwind = std::panic::catch_unwind(|| {
        let _active = ActiveTuiLease::acquire_in(Some(temp.path())).expect("active lease");
        let _root = RootWriterLease::acquire(&root.root).expect("root lease");
        panic!("fixture unwind");
    });
    assert!(unwind.is_err());
    ActiveTuiLease::acquire_in(Some(temp.path())).expect("active lease released while unwinding");
    RootWriterLease::acquire(&root.root).expect("root lease released while unwinding");
}
