// SPDX-License-Identifier: Apache-2.0

use suzumushi::init::initialize;
use suzumushi::locks::RootWriterLease;
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
fn different_roots_have_independent_leases() {
    let temp = TempDir::new().expect("temporary directory");
    let root_a = initialize(&temp.path().join("root-a")).expect("root a");
    let root_b = initialize(&temp.path().join("root-b")).expect("root b");
    let _a = RootWriterLease::acquire(&root_a.root).expect("first root lease");
    let _b = RootWriterLease::acquire(&root_b.root).expect("second root lease");
    suzumushi::config::load(&root_a.root).expect("read-only config access remains concurrent");
}

#[test]
fn root_lease_is_released_during_unwinding() {
    let temp = TempDir::new().expect("temporary directory");
    let root = initialize(&temp.path().join("root")).expect("initialize root");

    let unwind = std::panic::catch_unwind(|| {
        let _root = RootWriterLease::acquire(&root.root).expect("root lease");
        panic!("fixture unwind");
    });
    assert!(unwind.is_err());
    RootWriterLease::acquire(&root.root).expect("root lease released while unwinding");
}
