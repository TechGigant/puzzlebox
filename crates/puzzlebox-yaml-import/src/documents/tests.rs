use super::*;

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!("{}/tests/fixtures/documents/{name}", env!("CARGO_MANIFEST_DIR")))
        .unwrap()
}

fn keys(docs: &Documents) -> Vec<(Option<&str>, &str)> {
    docs.items.iter().map(|d| (d.key.as_deref(), d.config["alias"].as_str().unwrap_or(""))).collect()
}

#[test]
fn configuration_yaml_finds_labelled_keys_and_inline_packages() {
    let docs = documents(&fixture("configuration.yaml")).unwrap();
    assert_eq!(
        keys(&docs),
        [
            (None, "Night mode on at 23:00"),
            (Some("all_off"), "All lights off"),
            (None, "Porch light at sunset"),
            (None, "Single legacy automation"),
            (Some("goodnight"), "Goodnight"),
        ]
    );
    let legacy = &docs.items[3].config;
    assert_eq!(legacy["triggers"]["platform"], "state");
    assert_eq!(legacy["conditions"]["condition"], "state");
    assert_eq!(legacy["actions"]["service"], "notify.mobile_app_phone");
    assert!(legacy.get("trigger").is_none() && legacy.get("action").is_none());
    let goodnight = &docs.items[4].config;
    assert_eq!((goodnight["mode"].as_str(), goodnight["max"].as_u64()), (Some("queued"), Some(5)));
    assert!(docs.items.iter().all(|d| d.warnings.is_empty() && d.error.is_none()));
    let w = docs.warnings.join("\n");
    for expected in [
        "Package `from_file` is `!include packages/garage.yaml`",
        "Package `everything` is `!include_dir_named packages/more`",
        "`automation:` is `!include automations.yaml`",
        "`script:` is `!include scripts.yaml`",
    ] {
        assert!(w.contains(expected), "missing {expected:?} in {w}");
    }
    // Other integrations (`scene: !include`, `http:` secrets) aren't ours to report.
    assert!(!w.contains("scenes.yaml") && !w.contains("http_password"), "{w}");
}

#[test]
fn automations_yaml_keeps_custom_integration_actions() {
    let docs = documents(&fixture("automations.yaml")).unwrap();
    assert_eq!(docs.items.len(), 4);
    let legacy = &docs.items[1].config;
    assert_eq!(legacy["initial_state"], false);
    assert_eq!(legacy["trace"]["stored_traces"], 20);
    assert_eq!(legacy["max_exceeded"], "silent");
    let services: Vec<&str> =
        legacy["actions"].as_array().unwrap().iter().filter_map(|a| a["service"].as_str()).collect();
    assert_eq!(
        services,
        [
            "notify.mobile_app_phone",
            "notify.alexa_media",
            "browser_mod.popup",
            "homeassistant.delete_all_orphaned_entities",
            "pyscript.notify_all"
        ]
    );
    assert_eq!(legacy["actions"][5]["data"]["token"], "!secret webhook_token");
    assert!(docs.items[1].warnings[0].contains("`!secret webhook_token`"));

    let blueprint = &docs.items[2];
    assert!(blueprint.error.as_deref().unwrap().contains("homeassistant/motion_light.yaml"));

    let anchors = &docs.items[3].config;
    assert_eq!(anchors["triggers"][1]["entity_id"], "binary_sensor.office_motion");
    assert_eq!(anchors["triggers"][1]["to"], "off");
    assert_eq!(anchors["actions"][0]["data"]["brightness_pct"], 40);
    assert_eq!(anchors["trigger_variables"]["room"], "office");

    let ok = configs(&fixture("automations.yaml")).unwrap();
    assert_eq!(ok.len(), 3, "the item with an error isn't handed to the importer");
}

#[test]
fn scripts_yaml_keeps_ids_and_fields() {
    let docs = documents(&fixture("scripts.yaml")).unwrap();
    assert_eq!(keys(&docs), [(Some("morning_routine"), "Morning routine"), (Some("notify_everyone"), "")]);
    let morning = &docs.items[0].config;
    assert_eq!(morning["fields"]["brightness"]["default"], 60);
    assert_eq!(morning["icon"], "mdi:coffee");
    assert_eq!(docs.items[1].config["sequence"][0]["data"]["message"], "!include messages/hello.txt");
    assert!(docs.items[1].warnings[0].contains("points to another file"));
    assert!(
        docs.warnings[0].starts_with("`extra_scripts` is `!include more_scripts.yaml`"),
        "{:?}",
        docs.warnings
    );
}

#[test]
fn package_file_with_labelled_automation_key() {
    let docs = documents(&fixture("package_garage.yaml")).unwrap();
    assert_eq!(
        keys(&docs),
        [(None, "Close garage at night"), (None, "Garage light with door"), (Some("open_garage"), "")]
    );
}

#[test]
fn named_packages_map() {
    let yaml = "garage:\n  automation:\n    - alias: A\n      triggers: []\n      actions: []\n\
                lights:\n  script:\n    s1:\n      sequence: []\n";
    let docs = documents(yaml).unwrap();
    assert_eq!(keys(&docs), [(None, "A"), (Some("s1"), "")]);
}

#[test]
fn blueprint_file_uses_defaults_and_flattens_sections() {
    let docs = documents(&fixture("blueprint_motion_light.yaml")).unwrap();
    assert_eq!(docs.items.len(), 1);
    let d = &docs.items[0];
    assert_eq!(d.config["alias"], "Motion-activated Light");
    assert_eq!(d.config["description"], "Turn on a light when motion is detected.");
    assert!(d.config.get("blueprint").is_none());
    // `no_motion_wait` sits in a section and has a default.
    assert_eq!(d.config["actions"][2]["delay"], 120);
    assert_eq!(d.config["triggers"]["entity_id"], "!input motion_entity");
    assert_eq!(d.config["actions"][0]["target"], "!input light_target");
    let w = d.warnings.join("\n");
    assert!(w.contains("default inputs"), "{w}");
    assert!(w.contains("input motion_entity has no value") && w.contains("input light_target has no value"));
}

#[test]
fn use_blueprint_is_expanded_when_the_blueprint_is_pasted_too() {
    let docs = documents(&fixture("blueprint_script_with_uses.yaml")).unwrap();
    assert_eq!(keys(&docs), [(Some("confirm_door"), "Confirm door"), (Some("confirm_oven"), "")]);
    let door = &docs.items[0];
    assert_eq!(door.config["mode"], "restart");
    assert_eq!(door.config["sequence"][0]["device_id"], "0123456789abcdef");
    assert_eq!(door.config["sequence"][0]["title"], "", "falls back to the input's default");
    assert_eq!(door.config["sequence"][0]["message"], "Is the door locked?");
    assert!(door.config.get("use_blueprint").is_none() && door.config.get("blueprint").is_none());
    assert_eq!(door.warnings.len(), 1);
    let oven = &docs.items[1];
    assert_eq!(oven.config["sequence"][0]["message"], "!input message");
    assert!(oven.warnings.iter().any(|w| w.contains("input message has no value")));
}

#[test]
fn use_blueprint_picks_the_blueprint_by_source_url() {
    let yaml = "\
blueprint: {name: One, domain: automation, source_url: https://x/one.yaml, input: {a: {default: 1}}}
triggers: []
actions: [{delay: !input a}]
---
blueprint: {name: Two, domain: automation, source_url: https://x/two.yaml, input: {a: {default: 2}}}
triggers: []
actions: [{delay: !input a}]
---
- use_blueprint: {path: me/two.yaml}
- use_blueprint: {path: me/three.yaml}
";
    let docs = documents(yaml).unwrap();
    // `One` wasn't used, so it is imported on its own; `Two` became the first automation.
    assert_eq!(docs.items.len(), 3);
    assert_eq!(docs.items[0].config["alias"], "One");
    assert_eq!(docs.items[1].config["actions"][0]["delay"], 2);
    assert!(docs.items[2].error.as_deref().unwrap().contains("several blueprints"));
}

#[test]
fn every_tag_becomes_a_placeholder_with_a_warning() {
    let docs = documents(&fixture("tags.yaml")).unwrap();
    let d = &docs.items[0];
    assert_eq!(d.config["triggers"][0]["entity_id"], "binary_sensor.front_door", "!env_var default");
    let data = &d.config["actions"][0]["data"];
    for (k, v) in [
        ("message", "!secret telegram_message"),
        ("target", "!env_var TELEGRAM_CHAT"),
        ("config", "!include telegram.yaml"),
        ("many", "!include_dir_list snippets"),
        ("named", "!include_dir_named named"),
        ("merged", "!include_dir_merge_list merged"),
        ("merged_named", "!include_dir_merge_named merged_named"),
        ("device", "!input notify_device"),
        ("other", "kept"),
        ("typed", "123"),
    ] {
        assert_eq!(data[k], v, "{k}");
    }
    assert_eq!(d.warnings.len(), 10, "{:#?}", d.warnings);
    assert!(d.warnings.iter().any(|w| w.contains("Unknown YAML tag `!custom_tag`")));
}

#[test]
fn tags_on_maps_and_lists_keep_their_content() {
    let docs = documents("alias: t\nactions: !my_list\n  - delay: !secret wait\n").unwrap();
    assert_eq!(docs.items[0].config["actions"][0]["delay"], "!secret wait");
    assert_eq!(docs.items[0].warnings.len(), 2);
}

#[test]
fn multiple_documents_are_all_read() {
    let docs = documents(&fixture("multiple_documents.yaml")).unwrap();
    assert_eq!(keys(&docs), [(None, "First"), (None, "Second"), (Some("scriptone"), "")]);
}

#[test]
fn bare_actions_become_a_script() {
    let docs = documents(&fixture("bare_actions.yaml")).unwrap();
    assert_eq!(docs.items.len(), 1);
    assert_eq!(docs.items[0].config["sequence"].as_array().unwrap().len(), 3);
    let single = documents("action: light.turn_on\ntarget:\n  entity_id: light.x\n").unwrap();
    assert_eq!(single.items[0].config["sequence"][0]["action"], "light.turn_on");
}

#[test]
fn single_automation_from_the_ui_or_the_api() {
    let yaml = r#"{"id": "1", "alias": "JSON", "triggers": [{"trigger": "state", "entity_id": "a.b"}], "actions": []}"#;
    assert_eq!(keys(&documents(yaml).unwrap()), [(None, "JSON")]);
}

#[test]
fn legacy_and_modern_keys_together_keep_the_modern_one() {
    let docs =
        documents("trigger: [{platform: sun}]\ntriggers: [{trigger: time, at: '10:00'}]\nactions: []\n")
            .unwrap();
    assert_eq!(docs.items[0].config["triggers"][0]["trigger"], "time");
    assert!(docs.items[0].warnings[0].contains("Both `trigger` and `triggers`"));
    // Normalising keeps the key's position.
    let docs = documents("alias: a\ntrigger: []\naction: []\nmode: single\n").unwrap();
    let order: Vec<&str> = docs.items[0].config.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(order, ["alias", "triggers", "actions", "mode"]);
}

#[test]
fn numbers_and_booleans_in_text_fields() {
    let docs = documents("alias: 2024\nid: 17\ndescription: true\ntriggers: []\nactions: []\n").unwrap();
    let c = &docs.items[0].config;
    assert_eq!(
        (c["alias"].as_str(), c["id"].as_str(), c["description"].as_str()),
        (Some("2024"), Some("17"), Some("true"))
    );
}

#[test]
fn nothing_to_import() {
    assert!(documents("").unwrap().items.is_empty());
    assert_eq!(configs("sensor:\n  - platform: template\n").unwrap(), vec![]);
    let err = configs("automation: !include_dir_merge_list automations/\n").unwrap_err();
    assert!(err.contains("!include_dir_merge_list automations/"), "{err}");
    let docs = documents("just text").unwrap();
    assert!(docs.warnings[0].contains("single value"));
    let docs = documents("automation:\n  - alias: no body\n").unwrap();
    assert_eq!(docs.items.len(), 1, "under `automation:` an incomplete item is still an automation");
}

#[test]
fn non_automation_blueprints_are_skipped() {
    let yaml = "blueprint: {name: T, domain: template, input: {}}\nsensor: {state: '{{ 1 }}'}\n";
    let docs = documents(yaml).unwrap();
    assert!(docs.items.is_empty());
    assert!(docs.warnings[0].contains("for template"));
}

#[test]
fn duplicate_keys_warn_and_the_last_wins() {
    let docs = documents("alias: a\nalias: b\ntriggers: []\nactions: []\n").unwrap();
    assert_eq!(docs.items[0].config["alias"], "b");
    assert!(docs.warnings[0].contains("`alias` appears twice"), "{:?}", docs.warnings);
}

#[test]
fn errors_carry_line_numbers() {
    let err = documents("alias: a\nactions:\n  - delay: 1\n bad: x\n").unwrap_err();
    assert!(err.contains("line 4"), "{err}");
}
