//! Smoke tests for the `exposure` module (FE-12 ExposureFilter).
//!
//! TODO (T-001): expand with full include/exclude/glob coverage. Sister tests
//! exist as unit tests in src/exposure.rs; this file holds integration-level
//! smoke checks against the public crate-root API.

use apcore_cli::ExposureFilter;

#[test]
fn exposure_filter_default_is_all_mode() {
    let filter = ExposureFilter::default();
    assert_eq!(filter.mode, "all");
    assert!(filter.is_exposed("anything.goes"));
}

#[test]
fn exposure_filter_new_include_mode() {
    let filter = ExposureFilter::new("include", &["admin.*".to_string()], &[]);
    assert_eq!(filter.mode, "include");
    assert!(filter.is_exposed("admin.users"));
    assert!(!filter.is_exposed("public.modules"));
}

#[test]
fn exposure_filter_new_exclude_mode() {
    let filter = ExposureFilter::new("exclude", &[], &["debug.*".to_string()]);
    assert_eq!(filter.mode, "exclude");
    assert!(!filter.is_exposed("debug.dump"));
    assert!(filter.is_exposed("math.add"));
}

#[test]
fn exposure_filter_filter_modules_partitions() {
    let filter = ExposureFilter::new("exclude", &[], &["test.*".to_string()]);
    let modules = vec![
        "math.add".to_string(),
        "test.fixture".to_string(),
        "text.upper".to_string(),
    ];
    let (exposed, hidden) = filter.filter_modules(&modules);
    assert_eq!(exposed.len(), 2);
    assert_eq!(hidden.len(), 1);
    assert!(hidden.contains(&"test.fixture".to_string()));
}
