//! Integration tests for `display_helpers` — display-overlay resolution
//! consumed by the grouped command surface (FE-09). Audit D5-002 — adds a
//! dedicated `tests/` peer that exercises the public re-exports through the
//! library boundary, complementing the in-file unit tests.

use apcore_cli::{get_cli_display_fields, get_display};
use serde_json::json;

#[test]
fn get_display_returns_metadata_display_block() {
    let descriptor = json!({
        "metadata": {
            "display": {
                "alias": "greet",
                "tags": ["demo", "fixture"]
            }
        }
    });
    let display = get_display(&descriptor);
    assert_eq!(display["alias"], "greet");
    assert_eq!(display["tags"], json!(["demo", "fixture"]));
}

#[test]
fn get_display_returns_null_when_metadata_missing() {
    let descriptor = json!({"module_id": "math.add"});
    assert!(get_display(&descriptor).is_null());
}

#[test]
fn cli_alias_takes_precedence_over_top_level_alias() {
    // Spec FE-09 fallback chain: cli.alias > display.alias > id > module_id.
    let descriptor = json!({
        "module_id": "math.add",
        "metadata": {
            "display": {
                "alias": "top-alias",
                "cli": { "alias": "cli-alias", "description": "cli-desc" }
            }
        }
    });
    let (name, desc, _) = get_cli_display_fields(&descriptor);
    assert_eq!(name, "cli-alias");
    assert_eq!(desc, "cli-desc");
}

#[test]
fn module_id_used_when_no_alias_or_id_present() {
    let descriptor = json!({"module_id": "math.multiply"});
    let (name, desc, tags) = get_cli_display_fields(&descriptor);
    assert_eq!(name, "math.multiply");
    assert_eq!(desc, "");
    assert!(tags.is_empty());
}

#[test]
fn tags_resolve_from_display_overlay_then_descriptor_root() {
    // Display.tags wins over descriptor.tags.
    let descriptor = json!({
        "tags": ["root-tag"],
        "metadata": { "display": { "tags": ["overlay-tag"] } }
    });
    let (_, _, tags) = get_cli_display_fields(&descriptor);
    assert_eq!(tags, vec!["overlay-tag"]);
}

#[test]
fn tags_fall_back_to_descriptor_root_when_overlay_absent() {
    let descriptor = json!({"tags": ["x", "y"]});
    let (_, _, tags) = get_cli_display_fields(&descriptor);
    assert_eq!(tags, vec!["x", "y"]);
}
