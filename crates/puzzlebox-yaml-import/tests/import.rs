use puzzlebox_ir::{ExprKind, HandlerKind, NoExternals, StmtKind, Trigger, compile};
use puzzlebox_yaml_import::import_yaml;
use serde_json::json;

const AUTOMATION: &str = r#"
alias: Hallway motion light
description: Turns the hallway light on when motion is detected
mode: restart
triggers:
  - trigger: state
    entity_id: binary_sensor.hallway_motion
    to: "on"
  - trigger: sun
    event: sunset
    offset: "-00:30:00"
  - trigger: zone
    entity_id: person.me
    zone: zone.home
    event: enter
conditions:
  - condition: state
    entity_id: input_boolean.guest_mode
    state: "off"
  - "{{ now().hour >= 18 }}"
actions:
  - variables:
      brightness: 180
  - action: light.turn_on
    target:
      entity_id: [light.hallway, light.stairs]
    data:
      brightness: "{{ brightness }}"
      transition: 2
  - delay: "00:05:00"
  - choose:
      - conditions:
          - condition: state
            entity_id: binary_sensor.hallway_motion
            state: "on"
        sequence:
          - action: notify.notify
            data: {message: still moving}
    default:
      - action: light.turn_off
        target: {entity_id: light.hallway}
  - repeat:
      count: 3
      sequence:
        - event: blink
          event_data: {n: 1}
  - repeat:
      until: "{{ false }}"
      sequence: []
"#;

#[test]
fn imports_and_compiles_automation() {
    let out = import_yaml(AUTOMATION).unwrap();
    assert_eq!(out.len(), 1);
    let imp = &out[0];
    assert_eq!(imp.name, "Hallway motion light");
    assert_eq!(imp.mode, "restart");
    // zone trigger, template condition and repeat-until stay raw.
    assert_eq!(imp.raw_blocks, 3);

    let program = compile(&imp.workspace, &NoExternals).expect("imported workspace compiles");
    let h = &program.handlers[0];
    let HandlerKind::Triggers { triggers } = &h.kind else { panic!() };
    assert_eq!(triggers.len(), 3);
    let Trigger::Ha { config, .. } = &triggers[1] else { panic!() };
    assert_eq!(config["offset"], "-00:30:00");
    assert_eq!(config["event"], "sunset");
    let Trigger::Ha { config, .. } = &triggers[2] else { panic!() };
    assert_eq!(config["trigger"], "zone");

    // Conditions wrap the actions.
    assert_eq!(h.body.len(), 1);
    let StmtKind::If { branches, .. } = &h.body[0].kind else { panic!() };
    let ExprKind::Binary { .. } = &branches[0].0.kind else { panic!("conditions are AND-ed") };
    let actions = &branches[0].1;
    assert!(matches!(&actions[0].kind, StmtKind::Set { name, .. } if name == "brightness"));
    let StmtKind::CallService { service, target: Some(t), data: Some(d), .. } = &actions[1].kind else {
        panic!()
    };
    assert_eq!(service, "light.turn_on");
    assert_eq!(t.kind, ExprKind::Lit { value: json!("light.hallway,light.stairs") });
    let ExprKind::Dict { entries } = &d.kind else { panic!() };
    assert!(matches!(&entries[0].1.kind, ExprKind::Template { template } if template == "{{ brightness }}"));
    assert!(
        matches!(&actions[2].kind, StmtKind::Wait { seconds } if seconds.kind == ExprKind::Lit { value: json!(300) })
    );
    let StmtKind::If { branches, otherwise } = &actions[3].kind else { panic!() };
    assert_eq!(branches.len(), 1);
    assert!(otherwise.is_some());
    assert!(matches!(&actions[4].kind, StmtKind::Repeat { .. }));
    assert!(matches!(&actions[5].kind, StmtKind::HaScript { .. }));
}

#[test]
fn imports_script_maps_and_lists() {
    let scripts = r#"
morning:
  alias: Morning routine
  sequence:
    - action: cover.open_cover
      entity_id: cover.bedroom
    - parallel:
        - action: light.turn_on
          target: {area_id: kitchen}
        - sequence:
            - delay: {seconds: 2}
            - stop: done
evening:
  sequence:
    - scene: scene.evening
"#;
    let out = import_yaml(scripts).unwrap();
    assert_eq!(out.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), ["Morning routine", "evening"]);
    for imp in &out {
        assert_eq!(imp.raw_blocks, 0, "{}", imp.name);
        compile(&imp.workspace, &NoExternals).unwrap();
    }
    let p = compile(&out[0].workspace, &NoExternals).unwrap();
    let StmtKind::Parallel { branches } = &p.handlers[0].body[1].kind else { panic!() };
    assert_eq!(branches.len(), 2);
    assert!(matches!(branches[1][1].kind, StmtKind::Stop));
}

#[test]
fn rejects_non_automations() {
    assert!(import_yaml("foo: bar").is_err());
    assert!(import_yaml("- 1\n- 2").is_err());
}
