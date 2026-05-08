//! Smoke tests for the `exposure` module (FE-12 ExposureFilter).
//!
//! TODO (T-001): expand with full include/exclude/glob coverage. Sister tests
//! exist as unit tests in src/exposure.rs; this file holds integration-level
//! smoke checks against the public crate-root API.

use std::sync::{Arc, Mutex};

use apcore_cli::ExposureFilter;
use tracing::Level;
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::Registry;

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

// ---------------------------------------------------------------------------
// Custom tracing layer to capture WARN-level messages so the "none"-is-silent
// regression test can assert no spurious warning fired.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct WarnCaptureLayer {
    events: Arc<Mutex<Vec<String>>>,
}

impl WarnCaptureLayer {
    fn handle(&self) -> Arc<Mutex<Vec<String>>> {
        Arc::clone(&self.events)
    }
}

struct StringVisitor<'a>(&'a mut String);

impl tracing::field::Visit for StringVisitor<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            use std::fmt::Write as _;
            let _ = write!(self.0, "{value:?}");
        }
    }
}

impl<S> Layer<S> for WarnCaptureLayer
where
    S: tracing::Subscriber,
{
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        if *event.metadata().level() != Level::WARN {
            return;
        }
        let mut buf = String::new();
        event.record(&mut StringVisitor(&mut buf));
        if let Ok(mut guard) = self.events.lock() {
            guard.push(buf);
        }
    }
}

/// D11-011: `mode = "none"` is a legitimate user-supplied value (it hides
/// every module). Cross-SDK parity requires Rust to accept it silently —
/// matching Python and TypeScript — instead of warning "Unknown
/// ExposureFilter mode 'none'" and clamping back to "none".
#[test]
fn exposure_filter_new_none_mode_does_not_warn() {
    let layer = WarnCaptureLayer::default();
    let captured = layer.handle();
    let subscriber = Registry::default().with(layer);
    let _guard = tracing::subscriber::set_default(subscriber);

    let filter = ExposureFilter::new("none", &[], &[]);
    assert_eq!(filter.mode, "none");
    assert!(!filter.is_exposed("anything.goes"));

    let warns = captured.lock().unwrap().clone();
    let unknown_warns: Vec<&String> = warns
        .iter()
        .filter(|m| m.contains("Unknown ExposureFilter mode"))
        .collect();
    assert!(
        unknown_warns.is_empty(),
        "mode='none' must not emit an 'Unknown ExposureFilter mode' warning; got {warns:?}"
    );
}

/// D11-011 (control): a genuinely unknown mode must still warn.
#[test]
fn exposure_filter_new_truly_unknown_mode_still_warns() {
    let layer = WarnCaptureLayer::default();
    let captured = layer.handle();
    let subscriber = Registry::default().with(layer);
    let _guard = tracing::subscriber::set_default(subscriber);

    let filter = ExposureFilter::new("bogus", &[], &[]);
    // Unknown modes still clamp to "none" (fail-closed).
    assert_eq!(filter.mode, "none");

    let warns = captured.lock().unwrap().clone();
    let unknown_warns: Vec<&String> = warns
        .iter()
        .filter(|m| m.contains("Unknown ExposureFilter mode"))
        .collect();
    assert_eq!(
        unknown_warns.len(),
        1,
        "unknown mode 'bogus' must emit exactly one warning; got {warns:?}"
    );
}
