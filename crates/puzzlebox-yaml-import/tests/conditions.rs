use std::collections::BTreeMap;

use puzzlebox_ir::{BinOp, Expr, ExprKind, NoExternals, StmtKind, compile, substitute_args};
use puzzlebox_yaml_import::{Imported, import_yaml};
use serde_json::{Value, json};

/// Imports an automation with these conditions (YAML list items).
fn import(conditions: &str) -> Imported {
    let indented: String = conditions.lines().map(|l| format!("  {l}\n")).collect();
    let text = format!(
        "triggers:\n  - trigger: state\n    entity_id: binary_sensor.door\nconditions:\n{indented}actions:\n  - action: light.turn_on\n    target: {{entity_id: light.a}}\n"
    );
    let mut out = import_yaml(&text).unwrap();
    out.remove(0)
}

/// The block in the "if" that wraps the actions.
fn cond_block(imp: &Imported) -> &Value {
    &imp.workspace["blocks"]["blocks"][0]["inputs"]["DO"]["block"]["inputs"]["IF0"]["block"]
}

fn ty(b: &Value) -> &str {
    b["type"].as_str().unwrap()
}

fn input<'a>(b: &'a Value, name: &str) -> &'a Value {
    let i = &b["inputs"][name];
    if i["block"].is_null() { &i["shadow"] } else { &i["block"] }
}

/// Compiles the workspace and returns the condition expression.
fn cond_expr(imp: &Imported) -> Expr {
    let p = compile(&imp.workspace, &NoExternals).expect("imported workspace compiles");
    let StmtKind::If { branches, .. } = &p.handlers[0].body[0].kind else { panic!("expected an if") };
    branches[0].0.clone()
}

/// The HA condition config a condition expression sends, with literal arguments filled in.
fn ha_config(e: &Expr) -> Value {
    let ExprKind::Condition { config, args } = &e.kind else { panic!("not an HA condition: {e:?}") };
    let vals: BTreeMap<String, Value> = args
        .iter()
        .map(|(k, a)| match &a.kind {
            ExprKind::Lit { value } => (k.clone(), value.clone()),
            other => panic!("argument {k} is not a literal: {other:?}"),
        })
        .collect();
    substitute_args(config, &vals)
}

fn binary(e: &Expr) -> (BinOp, &Expr, &Expr) {
    let ExprKind::Binary { bin, left, right } = &e.kind else { panic!("not binary: {e:?}") };
    (*bin, left, right)
}

fn raw_yaml(b: &Value) -> Value {
    assert_eq!(ty(b), "ha_condition_raw");
    serde_yaml::from_str(b["fields"]["CONFIG"].as_str().unwrap()).unwrap()
}

#[test]
fn state_condition() {
    let imp = import("- condition: state\n  entity_id: light.kitchen\n  state: \"on\"");
    let b = cond_block(&imp);
    assert_eq!(ty(b), "ha_is_state");
    assert_eq!(b["fields"]["STATE"], "on");
    assert_eq!(input(b, "ENTITY")["type"], "ha_entity");
    assert_eq!(input(b, "ENTITY")["fields"]["ENTITY"], "light.kitchen");
    assert_eq!(imp.raw_blocks, 0);
    assert_eq!(
        ha_config(&cond_expr(&imp)),
        json!({"condition": "state", "entity_id": "light.kitchen", "state": "on"})
    );
}

#[test]
fn state_entity_lists_and_state_lists() {
    for entities in ["[light.a, light.b]", "\"light.a, light.b\""] {
        let imp =
            import(&format!("- condition: state\n  entity_id: {entities}\n  state: \"off\"\n  match: all"));
        assert_eq!(ty(cond_block(&imp)), "ha_is_state");
        assert_eq!(ha_config(&cond_expr(&imp))["entity_id"], "light.a,light.b");
    }

    let imp = import(
        "- condition: state\n  entity_id: alarm_control_panel.home\n  state: [armed_away, armed_home]",
    );
    let b = cond_block(&imp);
    assert_eq!(ty(b), "logic_operation");
    assert_eq!(b["fields"]["OP"], "OR");
    let e = cond_expr(&imp);
    let (op, l, r) = binary(&e);
    assert_eq!(op, BinOp::Or);
    assert_eq!(ha_config(l)["state"], "armed_away");
    assert_eq!(ha_config(r)["state"], "armed_home");

    // A state naming an input_* entity is compared by HA, so it passes through.
    let imp = import("- condition: state\n  entity_id: input_select.mode\n  state: input_select.wanted");
    assert_eq!(ha_config(&cond_expr(&imp))["state"], "input_select.wanted");
}

#[test]
fn state_for() {
    let imp =
        import("- condition: state\n  entity_id: binary_sensor.window\n  state: \"off\"\n  for: \"00:10\"");
    let b = cond_block(&imp);
    assert_eq!(ty(b), "ha_state_for");
    let d = input(b, "SECONDS");
    assert_eq!(d["type"], "duration");
    assert_eq!(d["fields"]["UNIT"], "minutes");
    assert_eq!(input(d, "N")["fields"]["NUM"], 10);
    assert_eq!(
        ha_config(&cond_expr(&imp)),
        json!({"condition": "state", "entity_id": "binary_sensor.window", "state": "off", "for": {"seconds": 600}})
    );

    for (period, secs) in [("{hours: 1, minutes: 30}", 5400), ("45", 45), ("\"01:00:05\"", 3605)] {
        let imp = import(&format!(
            "- condition: state\n  entity_id: binary_sensor.window\n  state: \"off\"\n  for: {period}"
        ));
        assert_eq!(ha_config(&cond_expr(&imp))["for"]["seconds"], secs, "{period}");
    }

    let imp = import(
        "- condition: state\n  entity_id: binary_sensor.w\n  state: [\"off\", unavailable]\n  for: 45",
    );
    let e = cond_expr(&imp);
    let (op, l, r) = binary(&e);
    assert_eq!(op, BinOp::Or);
    assert_eq!(ha_config(l)["for"]["seconds"], 45);
    assert_eq!(ha_config(r)["state"], "unavailable");
}

#[test]
fn state_forms_kept_raw() {
    for c in [
        // HA reports an unknown entity even when another one matches.
        "- condition: state\n  entity_id: [light.a, light.b]\n  state: \"on\"\n  match: any",
        "- condition: state\n  entity_id: light.a\n  attribute: brightness\n  state: 255",
        "- condition: state\n  entity_id: [light.a, light.b]\n  state: [\"on\", \"off\"]",
        "- condition: state\n  entity_id: light.a\n  state: \"on\"\n  for: \"{{ minutes }}\"",
        "- condition: state\n  entity_id: light.a\n  state: \"on\"\n  for: \"-00:05\"",
        "- condition: state\n  entity_id: light.a\n  state: 5",
    ] {
        let imp = import(c);
        assert_eq!(ty(cond_block(&imp)), "ha_condition_raw", "{c}");
        assert_eq!(imp.raw_blocks, 1);
        cond_expr(&imp);
    }
    // One entity: `match: any` is the same as `all`.
    let imp = import("- condition: state\n  entity_id: light.a\n  state: \"on\"\n  match: any");
    assert_eq!(ty(cond_block(&imp)), "ha_is_state");
}

#[test]
fn state_as_friendlier_blocks() {
    let imp = import("- condition: state\n  entity_id: sun.sun\n  state: below_horizon");
    let b = cond_block(&imp);
    assert_eq!((ty(b), &b["fields"]["WHERE"]), ("sun_is", &json!("down")));
    assert_eq!(
        ha_config(&cond_expr(&imp)),
        json!({"condition": "state", "entity_id": "sun.sun", "state": "below_horizon"})
    );

    let imp = import("- condition: state\n  entity_id: person.anna\n  state: not_home");
    let b = cond_block(&imp);
    assert_eq!((ty(b), &b["fields"]["WHERE"]), ("person_is_home", &json!("away")));
    assert_eq!(
        ha_config(&cond_expr(&imp)),
        json!({"condition": "state", "entity_id": "person.anna", "state": "not_home"})
    );
}

#[test]
fn numeric_state_template_sun_zone_and_integrations_stay_raw() {
    for c in [
        "- condition: numeric_state\n  entity_id: sensor.t\n  above: 17\n  below: 25",
        "- condition: numeric_state\n  entity_id: sensor.t\n  attribute: x\n  below: input_number.max",
        "- condition: sun\n  after: sunset\n  after_offset: \"-01:00:00\"",
        "- condition: sun.is_up",
        "- condition: zone\n  entity_id: person.anna\n  zone: zone.home",
        "- condition: light.is_on\n  target: {area_id: kitchen}\n  options: {behavior: any}",
    ] {
        let imp = import(c);
        let b = cond_block(&imp);
        let want: Value = serde_yaml::from_str::<Vec<Value>>(c).unwrap().remove(0);
        assert_eq!(raw_yaml(b), want);
        let e = cond_expr(&imp);
        assert_eq!(ha_config(&e), want);
    }
}

#[test]
fn template_conditions() {
    let boolean = ["logic_boolean", "logic_compare", "logic_operation", "logic_negate", "ha_is_state"];
    for c in [
        "- condition: template\n  value_template: \"{{ is_state('light.a', 'on') }}\"",
        "- \"{{ now().hour >= 18 }}\"",
        // The condition action's shorthand.
        "- condition: \"{{ states('sensor.x') }}\"",
    ] {
        let imp = import(c);
        let b = cond_block(&imp);
        if ty(b) == "ha_condition_raw" {
            let cfg = raw_yaml(b);
            assert_eq!(cfg["condition"], "template", "{c}");
            assert!(cfg["value_template"].as_str().unwrap().contains("{{"));
        } else {
            assert!(boolean.contains(&ty(b)), "{c}: {}", ty(b));
        }
        cond_expr(&imp);
    }
}

#[test]
fn time_conditions() {
    let imp = import("- condition: time\n  after: \"15:00:00\"\n  before: \"2:00\"");
    let b = cond_block(&imp);
    assert_eq!(ty(b), "time_is_between");
    assert_eq!(b["fields"]["AFTER"], "15:00:00");
    assert_eq!(b["fields"]["BEFORE"], "02:00");
    assert_eq!(
        ha_config(&cond_expr(&imp)),
        json!({"condition": "time", "after": "15:00:00", "before": "02:00"})
    );

    let imp = import(
        "- condition: time\n  after: input_datetime.wake\n  before: sensor.alarm\n  weekday: [mon, tue, wed, thu, fri, sat]",
    );
    assert_eq!(
        ha_config(&cond_expr(&imp)),
        json!({"condition": "time", "after": "input_datetime.wake", "before": "sensor.alarm",
            "weekday": ["mon", "tue", "wed", "thu", "fri", "sat"]})
    );

    for (days, when) in [("sat", "sat"), ("[sun, sat]", "weekend"), ("[mon, tue, wed, thu, fri]", "weekday")]
    {
        let imp = import(&format!("- condition: time\n  weekday: {days}"));
        let b = cond_block(&imp);
        assert_eq!((ty(b), &b["fields"]["WHEN"]), ("weekday_is", &json!(when)));
        cond_expr(&imp);
    }

    let imp = import("- condition: time\n  weekday: [mon, wed]");
    let b = cond_block(&imp);
    assert_eq!(ty(b), "time_is_between");
    assert_eq!(ha_config(&cond_expr(&imp)), json!({"condition": "time", "weekday": ["mon", "wed"]}));

    // Every day: always true.
    let imp = import("- condition: time\n  weekday: [mon, tue, wed, thu, fri, sat, sun]");
    assert_eq!(ty(cond_block(&imp)), "logic_boolean");

    for c in [
        "- condition: time\n  before: \"07:30:00.5\"",
        "- condition: time\n  after: \"7\"",
        "- condition: time\n  after: light.kitchen",
        "- condition: time\n  weekday: []",
        "- condition: time\n  weekday: Monday",
    ] {
        let imp = import(c);
        assert_eq!(ty(cond_block(&imp)), "ha_condition_raw", "{c}");
        cond_expr(&imp);
    }
}

#[test]
fn trigger_conditions() {
    let imp = import("- condition: trigger\n  id: door");
    let b = cond_block(&imp);
    assert_eq!((ty(b), &b["fields"]["TRIGGER"]), ("triggered_by", &json!("door")));
    let e = cond_expr(&imp);
    let (op, l, r) = binary(&e);
    assert_eq!(op, BinOp::Eq);
    assert_eq!(l.kind, ExprKind::TriggerData { path: "id".into() });
    assert_eq!(r.kind, ExprKind::Lit { value: json!("door") });

    let imp = import("- condition: trigger\n  id: [door, morning]");
    let b = cond_block(&imp);
    assert_eq!(ty(b), "logic_operation");
    assert_eq!(input(b, "B")["fields"]["TRIGGER"], "morning");
    cond_expr(&imp);

    // The engine compares numbers numerically ("1" == "01"), HA compares text.
    for c in ["- condition: trigger\n  id: 0", "- condition: trigger\n  id: [door, \"2\"]"] {
        let imp = import(c);
        assert_eq!(ty(cond_block(&imp)), "ha_condition_raw", "{c}");
        cond_expr(&imp);
    }
}

#[test]
fn device_condition() {
    let imp = import(
        "- condition: device\n  device_id: abc123\n  domain: light\n  entity_id: e456\n  type: is_on\n  for: {minutes: 2}\n  metadata: {secondary: false}",
    );
    let b = cond_block(&imp);
    assert_eq!(ty(b), "device_condition");
    assert_eq!(b["extraState"]["label"], "light is on");
    assert_eq!(imp.raw_blocks, 0);
    assert_eq!(
        ha_config(&cond_expr(&imp)),
        json!({"condition": "device", "device_id": "abc123", "domain": "light", "entity_id": "e456",
            "type": "is_on", "for": {"minutes": 2}})
    );
}

#[test]
fn and_or_not() {
    let a = "  - condition: state\n    entity_id: light.a\n    state: \"on\"\n";
    let b = "  - condition: state\n    entity_id: light.b\n    state: \"on\"\n";
    for (c, op) in [
        (format!("- condition: and\n  conditions:\n{a}{b}"), "AND"),
        (format!("- condition: or\n  conditions:\n{a}{b}"), "OR"),
        (format!("- and:\n{a}{b}"), "AND"),
        (format!("- or:\n{a}{b}"), "OR"),
        (format!("- condition:\n{a}{b}"), "AND"),
    ] {
        let imp = import(&c);
        let blk = cond_block(&imp);
        assert_eq!((ty(blk), &blk["fields"]["OP"]), ("logic_operation", &json!(op)), "{c}");
        assert_eq!(imp.raw_blocks, 0);
        cond_expr(&imp);
    }
    for c in [format!("- condition: not\n  conditions:\n{a}{b}"), format!("- not:\n{a}{b}")] {
        let imp = import(&c);
        let blk = cond_block(&imp);
        assert_eq!(ty(blk), "logic_negate");
        assert_eq!(input(blk, "BOOL")["fields"]["OP"], "OR");
        let ExprKind::Not { .. } = cond_expr(&imp).kind else { panic!() };
    }

    // Top-level conditions are AND-ed in order.
    let imp = import(&format!("{}{}", a.trim_start(), b.trim_start()).replace("\n  ", "\n"));
    assert_eq!(cond_block(&imp)["fields"]["OP"], "AND");

    // Empty: HA's AND is true, OR false, NOT true (not false).
    for (kind, want) in [("and", "TRUE"), ("or", "FALSE"), ("not", "FALSE")] {
        let imp = import(&format!("- condition: {kind}\n  conditions: []"));
        let blk = cond_block(&imp);
        let blk = if kind == "not" { input(blk, "BOOL") } else { blk };
        assert_eq!(blk["fields"]["BOOL"], want, "{kind}");
        cond_expr(&imp);
    }
}

#[test]
fn all_raw_parts_stay_one_raw_condition() {
    let c = "- or:\n  - condition: numeric_state\n    entity_id: sensor.a\n    above: 1\n  - condition: sun\n    after: sunset";
    let imp = import(c);
    assert_eq!(imp.raw_blocks, 1);
    let cfg = raw_yaml(cond_block(&imp));
    assert_eq!(cfg["or"].as_array().unwrap().len(), 2);
    cond_expr(&imp);

    // A whole list of raw conditions becomes one raw AND.
    let imp = import(
        "- condition: sun\n  after: sunset\n- condition: zone\n  entity_id: person.a\n  zone: zone.home",
    );
    assert_eq!(imp.raw_blocks, 1);
    let cfg = raw_yaml(cond_block(&imp));
    assert_eq!(cfg["condition"], "and");
    assert_eq!(ha_config(&cond_expr(&imp))["conditions"][1]["condition"], "zone");

    // Mixed: native parts stay native.
    let imp = import(
        "- or:\n  - condition: state\n    entity_id: light.a\n    state: \"on\"\n  - condition: sun\n    after: sunset",
    );
    assert_eq!(imp.raw_blocks, 1);
    let blk = cond_block(&imp);
    assert_eq!(ty(blk), "logic_operation");
    assert_eq!(ty(input(blk, "A")), "ha_is_state");
    assert_eq!(ty(input(blk, "B")), "ha_condition_raw");
}

#[test]
fn disabled_conditions() {
    let on = |e: &str, extra: &str| format!("- condition: state\n  entity_id: {e}\n  state: \"on\"\n{extra}");
    // Skipped by HA: dropped from an AND list.
    let imp = import(&format!("{}{}", on("light.a", "  enabled: false\n"), on("light.b", "")));
    let b = cond_block(&imp);
    assert_eq!(ty(b), "ha_is_state");
    assert_eq!(input(b, "ENTITY")["fields"]["ENTITY"], "light.b");

    let imp = import(&on("light.a", "  enabled: \"off\"\n"));
    assert_eq!(cond_block(&imp)["fields"]["BOOL"], "TRUE");

    let imp =
        import("- or:\n  - condition: state\n    entity_id: light.a\n    state: \"on\"\n    enabled: false");
    assert_eq!(cond_block(&imp)["fields"]["BOOL"], "FALSE");

    // A template decides: true when disabled inside an AND, false inside an OR.
    let imp = import(&on("light.a", "  enabled: \"{{ use_a }}\"\n"));
    let b = cond_block(&imp);
    assert_eq!(b["fields"]["OP"], "OR");
    assert_eq!(ty(input(b, "A")), "logic_negate");
    assert_eq!(input(input(b, "A"), "BOOL")["fields"]["TEMPLATE"], "{{ use_a }}");
    assert_eq!(ty(input(b, "B")), "ha_is_state");
    cond_expr(&imp);

    let imp = import(
        "- or:\n  - condition: state\n    entity_id: light.a\n    state: \"on\"\n    enabled: \"{{ use_a }}\"\n  - condition: state\n    entity_id: light.b\n    state: \"on\"",
    );
    let b = input(cond_block(&imp), "A");
    assert_eq!(b["fields"]["OP"], "AND");
    assert_eq!(ty(input(b, "A")), "ha_template");
    cond_expr(&imp);
}

#[test]
fn alias_becomes_a_comment() {
    let imp = import(
        "- condition: state\n  alias: Kitchen light is on\n  entity_id: light.kitchen\n  state: \"on\"",
    );
    let b = cond_block(&imp);
    assert_eq!(ty(b), "ha_is_state");
    assert_eq!(b["icons"]["comment"]["text"], "Kitchen light is on");
    cond_expr(&imp);

    // Raw blocks keep the alias in their YAML.
    let imp = import("- condition: sun\n  alias: After dark\n  after: sunset");
    let b = cond_block(&imp);
    assert!(b.get("icons").is_none());
    assert_eq!(raw_yaml(b)["alias"], "After dark");
}

#[test]
fn imports_every_condition_form() {
    let text = include_str!("fixtures/conditions/all.yaml");
    let imp = import_yaml(text).unwrap().remove(0);
    let p = compile(&imp.workspace, &NoExternals).expect("every condition compiles");
    let StmtKind::If { .. } = &p.handlers[0].body[0].kind else { panic!() };

    let mut types = Vec::new();
    collect(cond_block(&imp), &mut types);
    for want in [
        "ha_is_state",
        "ha_state_for",
        "sun_is",
        "person_is_home",
        "time_is_between",
        "weekday_is",
        "triggered_by",
        "device_condition",
        "logic_negate",
        "logic_operation",
        "ha_condition_raw",
        "ha_template",
    ] {
        assert!(types.contains(&want.to_owned()), "{want} missing");
    }
    // Templates may convert to blocks; everything else here has no exact block.
    let mut raw = Vec::new();
    collect_raw(cond_block(&imp), &mut raw);
    let templates = raw.iter().filter(|k| *k == "template").count();
    assert!(templates <= 4, "{raw:?}");
    raw.retain(|k| k != "template");
    let want = [
        "state",
        "state",
        "state",
        "state",
        "numeric_state",
        "numeric_state",
        "numeric_state",
        "numeric_state",
        "time",
        "sun",
        "sun",
        "sun.is_up",
        "zone",
        "zone.in_zone",
        "trigger",
        "light.is_on",
        "light.brightness",
        "or",
    ];
    assert_eq!(raw, want);
    let json = serde_json::to_string(&imp.workspace).unwrap();
    assert!(!json.contains("light.disabled"), "disabled conditions are left out");
    assert!(!json.contains("ha_cnd__"));
}

/// The kind of each raw condition (its `condition`, or the shorthand's key).
fn collect_raw(b: &Value, out: &mut Vec<String>) {
    if b["type"] == "ha_condition_raw" {
        let cfg = raw_yaml(b);
        let kind = match cfg["condition"].as_str() {
            Some(k) => k.to_owned(),
            None => cfg.as_object().unwrap().keys().find(|k| *k != "alias").unwrap().clone(),
        };
        out.push(kind);
        return;
    }
    if let Some(ins) = b["inputs"].as_object() {
        for slot in ins.values() {
            collect_raw(&slot["block"], out);
        }
    }
}

fn collect(b: &Value, out: &mut Vec<String>) {
    if let Some(t) = b["type"].as_str() {
        out.push(t.to_owned());
    }
    if let Some(ins) = b["inputs"].as_object() {
        for slot in ins.values() {
            collect(&slot["block"], out);
            collect(&slot["shadow"], out);
        }
    }
}
