//! Smoke tests for the `fs_discoverer` module.
//!
//! TODO (T-001): expand with multi-extension discovery, error handling, and
//! permission edge cases. Real verification benefits from a live filesystem
//! fixture matching the apcore extensions layout.

use apcore_cli::FsDiscoverer;
use tempfile::TempDir;

#[test]
fn fs_discoverer_construct_from_path() {
    let tmp = TempDir::new().unwrap();
    let _ = FsDiscoverer::new(tmp.path());
}

#[test]
fn fs_discoverer_executables_snapshot_empty_dir() {
    let tmp = TempDir::new().unwrap();
    let discoverer = FsDiscoverer::new(tmp.path());
    let snap = discoverer.executables_snapshot();
    assert!(snap.is_empty());
}
