//! Every document form, imported end to end: each workspace must compile.

use puzzlebox_ir::{NoExternals, compile};
use puzzlebox_yaml_import::import_yaml;

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!("{}/tests/fixtures/documents/{name}", env!("CARGO_MANIFEST_DIR")))
        .unwrap()
}

/// Imports a fixture, checks every workspace compiles, and returns the imported names.
fn import(name: &str) -> Vec<String> {
    let items = import_yaml(&fixture(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
    for item in &items {
        if let Err(errors) = compile(&item.workspace, &NoExternals) {
            panic!("{name} / {}: {errors:?}\n{:#}", item.name, item.workspace);
        }
    }
    items.into_iter().map(|i| i.name).collect()
}

#[test]
fn configuration_yaml_with_packages() {
    assert_eq!(
        import("configuration.yaml"),
        [
            "Night mode on at 23:00",
            "All lights off",
            "Porch light at sunset",
            "Single legacy automation",
            "Goodnight"
        ]
    );
}

#[test]
fn automations_yaml() {
    // The `use_blueprint` automation is skipped: its blueprint isn't in the paste.
    assert_eq!(
        import("automations.yaml"),
        ["Hallway motion light", "Legacy notify on low battery", "With anchors"]
    );
}

#[test]
fn scripts_yaml() {
    assert_eq!(import("scripts.yaml"), ["Morning routine", "notify_everyone"]);
}

#[test]
fn package_file() {
    assert_eq!(
        import("package_garage.yaml"),
        ["Close garage at night", "Garage light with door", "open_garage"]
    );
}

#[test]
fn blueprint_file() {
    assert_eq!(import("blueprint_motion_light.yaml"), ["Motion-activated Light"]);
}

#[test]
fn blueprint_with_its_uses() {
    assert_eq!(import("blueprint_script_with_uses.yaml"), ["Confirm door", "confirm_oven"]);
}

#[test]
fn every_tag() {
    assert_eq!(import("tags.yaml"), ["Every tag"]);
}

#[test]
fn multiple_documents() {
    assert_eq!(import("multiple_documents.yaml"), ["First", "Second", "scriptone"]);
}

#[test]
fn bare_actions() {
    assert_eq!(import("bare_actions.yaml"), ["Imported"]);
}

#[test]
fn use_blueprint_alone_explains_what_is_missing() {
    let yaml = "alias: Uses a blueprint\nuse_blueprint:\n  path: someone/thing.yaml\n  input: {}\n";
    let err = import_yaml(yaml).unwrap_err();
    assert!(err.contains("someone/thing.yaml") && err.contains("config/blueprints/"), "{err}");
}

#[test]
fn include_only_explains_what_to_paste() {
    let err =
        import_yaml("automation: !include automations.yaml\nscript: !include scripts.yaml\n").unwrap_err();
    assert!(err.contains("`automation:` is `!include automations.yaml`"), "{err}");
}

#[test]
fn invalid_yaml_reports_the_line() {
    let err = import_yaml("alias: x\ntriggers:\n  - trigger: state\n   entity_id: a\n").unwrap_err();
    assert!(err.starts_with("invalid YAML") && err.contains("line 4"), "{err}");
    let err = import_yaml("alias: ok\nactions: []\n---\nalias: [\n").unwrap_err();
    assert!(err.contains("document 2") && err.contains("line"), "{err}");
}
