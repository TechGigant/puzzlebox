use puzzlebox_ir::{ExprKind, NoExternals, Program, Stmt, StmtKind, compile};
use puzzlebox_yaml_import::{Imported, import_yaml};
use serde_json::{Value, json};

/// Imports a script whose `sequence` is `actions` (YAML list, indented by two).
fn script(actions: &str) -> (Imported, Program) {
    let yaml = format!("alias: test\nsequence:\n{actions}");
    let mut out = import_yaml(&yaml).expect("imports");
    let imp = out.remove(0);
    let program = compile(&imp.workspace, &NoExternals).unwrap_or_else(|e| panic!("compiles: {e:?}\n{yaml}"));
    (imp, program)
}

fn body(p: &Program) -> &[Stmt] {
    &p.handlers[0].body
}

/// Types of the statement chain starting at `first`.
fn chain_types(first: Option<&Value>) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = first;
    while let Some(b) = cur {
        out.push(b["type"].as_str().unwrap_or("").to_owned());
        cur = b.pointer("/next/block");
    }
    out
}

/// Types of the top-level statements of an imported script.
fn top(imp: &Imported) -> Vec<String> {
    chain_types(imp.workspace.pointer("/blocks/blocks/0/inputs/DO/block"))
}

fn first_block(imp: &Imported) -> &Value {
    imp.workspace.pointer("/blocks/blocks/0/inputs/DO/block").expect("a statement")
}

fn raw_config(imp: &Imported, index: usize) -> Value {
    let mut cur = first_block(imp);
    for _ in 0..index {
        cur = cur.pointer("/next/block").unwrap();
    }
    assert_eq!(cur["type"], "ha_action_raw");
    serde_yaml::from_str(cur["fields"]["CONFIG"].as_str().unwrap()).unwrap()
}

fn lit(e: &puzzlebox_ir::Expr) -> &Value {
    match &e.kind {
        ExprKind::Lit { value } => value,
        other => panic!("not a literal: {other:?}"),
    }
}

fn dict(e: &puzzlebox_ir::Expr) -> Vec<(String, Value)> {
    let ExprKind::Dict { entries } = &e.kind else { panic!("not a dict: {:?}", e.kind) };
    entries.iter().map(|(k, v)| (k.clone(), plain(v))).collect()
}

/// Literals (and lists of them) as JSON, templates as `{"template": …}`.
fn plain(e: &puzzlebox_ir::Expr) -> Value {
    match &e.kind {
        ExprKind::Lit { value } => value.clone(),
        ExprKind::Template { template } => json!({"template": template}),
        ExprKind::List { items } => Value::Array(items.iter().map(plain).collect()),
        other => json!(format!("{other:?}")),
    }
}

#[test]
fn service_calls() {
    let (imp, p) = script(
        r#"
  - action: Light.Turn_On
    target:
      entity_id: [light.a, light.b]
      area_id: kitchen
      label_id: [evening, night]
    data: {brightness: 120, transition: 2.0, flash: short}
    data_template: {color_name: "{{ 'red' }}", flash: long}
  - service: switch.turn_on
    entity_id: switch.fan, switch.heater
  - action: light.turn_on
    target: {entity_id: light.x}
    entity_id: light.y
  - action: light.turn_on
    target: {entity_id: "{{ trigger.entity_id }}", area_id: hall}
"#,
    );
    assert_eq!(imp.raw_blocks, 0);
    assert_eq!(top(&imp), ["ha_call_service", "ha_call_service", "light_power", "light_power"]);
    let StmtKind::CallService { service, target: Some(t), data: Some(d), response_var: None } =
        &body(&p)[0].kind
    else {
        panic!()
    };
    assert_eq!(service, "light.turn_on");
    let ExprKind::List { items } = &t.kind else { panic!() };
    assert_eq!(lit(&items[0]), &json!("light.a,light.b"));
    assert_eq!(lit(&items[1]), &json!({"area_id": ["kitchen"]}));
    assert_eq!(lit(&items[2]), &json!({"label_id": ["evening", "night"]}));
    let mut d = dict(d);
    d.sort_by(|a, b| a.0.cmp(&b.0));
    // data_template wins over data; 2.0 stays a float.
    assert_eq!(
        d,
        [
            ("brightness".into(), json!(120)),
            ("color_name".into(), json!("red")),
            ("flash".into(), json!("long")),
            ("transition".into(), json!(2.0)),
        ]
    );
    assert!(d[3].1.is_f64());

    let StmtKind::CallService { service, target: Some(t), data: None, .. } = &body(&p)[1].kind else {
        panic!()
    };
    assert_eq!(service, "switch.turn_on");
    assert_eq!(lit(t), &json!("switch.fan,switch.heater"));
    // A top-level entity_id replaces the target's.
    let StmtKind::CallService { target: Some(t), .. } = &body(&p)[2].kind else { panic!() };
    assert_eq!(lit(t), &json!("light.y"));
    let StmtKind::CallService { target: Some(t), .. } = &body(&p)[3].kind else { panic!() };
    let mut t = dict(t);
    t.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        t,
        [
            ("area_id".into(), json!(["hall"])),
            // The template became the "trigger entity" block.
            ("entity_id".into(), json!("TriggerData { path: \"entity_id\" }"))
        ]
    );
}

#[test]
fn service_calls_that_stay_raw() {
    let (imp, _) = script(
        r#"
  - action: "{{ 'light.turn_off' }}"
  - action: light.turn_off
    target: {entity_id: all}
  - action: light.turn_off
    target: "{{ {'entity_id': 'light.a'} }}"
  - action: light.turn_off
    target: {entity_id: 0123456789abcdef0123456789abcdef}
  - service_template: light.turn_off
  - action: light.turn_off
    service_data: {transition: 1}
  - action: light.turn_off
    data: "{{ {'transition': 1} }}"
  - action: light.turn_off
    data: {"{{ 'transition' }}": 1}
  - action: weather.get_forecasts
    data: {type: daily}
    response_variable: trigger
"#,
    );
    assert_eq!(top(&imp), ["ha_action_raw"; 9]);
    assert_eq!(imp.raw_blocks, 9);
    assert_eq!(raw_config(&imp, 8)["response_variable"], "trigger");
}

#[test]
fn response_variables_keep_the_response() {
    let (imp, p) = script(
        r#"
  - delay: 1
  - action: weather.get_forecasts
    target: {entity_id: weather.home}
    data: {type: daily}
    response_variable: forecast
  - action: light.turn_on
    target: {entity_id: light.a}
  - action: notify.notify
    data: {message: "{{ forecast }}"}
  - action: light.turn_on
    target: {entity_id: light.a}
    response_variable: unusual
"#,
    );
    assert_eq!(
        top(&imp),
        ["ha_wait", "ha_call_service_response", "light_power", "notify_send", "ha_call_service_response"]
    );
    assert_eq!(imp.raw_blocks, 0);
    let StmtKind::CallService { service, target: Some(t), data: Some(d), response_var } = &body(&p)[1].kind
    else {
        panic!()
    };
    assert_eq!((service.as_str(), response_var.as_deref()), ("weather.get_forecasts", Some("forecast")));
    assert_eq!(lit(t), &json!("weather.home"));
    assert_eq!(dict(d), [("type".to_owned(), json!("daily"))]);
    // No helper block keeps a response.
    assert!(
        matches!(&body(&p)[4].kind, StmtKind::CallService { response_var: Some(r), .. } if r == "unusual")
    );
}

#[test]
fn variables_set_in_a_nested_raw_block_widen_it() {
    // The loop's count is a template, so it is raw; its variable is read after the "if".
    let (imp, _) = script(
        r#"
  - if: "{{ true }}"
    then:
      - repeat:
          count: "{{ 2 }}"
          sequence:
            - variables: {total: "{{ repeat.index }}"}
  - action: notify.notify
    data: {message: "{{ total }}"}
  - delay: 1
"#,
    );
    assert_eq!(top(&imp), ["ha_action_raw"]);
    assert_eq!(raw_config(&imp, 0).as_array().unwrap().len(), 3);

    // Not read later: only the loop is raw.
    let (imp, _) = script(
        r#"
  - if: []
    then:
      - repeat:
          count: "{{ 2 }}"
          sequence:
            - variables: {total: "{{ repeat.index }}"}
  - action: notify.notify
    data: {message: "{{ totally }}"}
"#,
    );
    assert_eq!(top(&imp), ["controls_if", "notify_send"]);
    assert_eq!(imp.raw_blocks, 1);
}

#[test]
fn stop_inside_a_raw_block() {
    // A stop in a raw block would end only that block, so what follows joins it.
    let (imp, _) = script(
        r#"
  - repeat:
      count: "{{ 2 }}"
      sequence:
        - if: "{{ repeat.index == 2 }}"
          then: [{stop: found}]
  - delay: 1
"#,
    );
    assert_eq!(top(&imp), ["ha_action_raw"]);
    assert_eq!(raw_config(&imp, 0).as_array().unwrap().len(), 2);
    // At the end of the run it doesn't matter.
    let (imp, _) = script(
        r#"
  - delay: 1
  - repeat:
      count: "{{ 2 }}"
      sequence:
        - if: "{{ repeat.index == 2 }}"
          then: [{stop: found}]
"#,
    );
    assert_eq!(top(&imp), ["ha_wait", "ha_action_raw"]);
}

#[test]
fn delays() {
    let (imp, p) = script(
        r#"
  - delay: "00:00:05"
  - delay: "00:05"
  - delay: 2.5
  - delay: "30"
  - delay: {hours: 1, minutes: 30}
  - delay: {days: 1}
  - delay: "{{ 3 }}"
  - delay: {seconds: "{{ 2 }}"}
"#,
    );
    assert_eq!(&top(&imp)[..6], ["ha_wait"; 6]);
    assert_eq!(&top(&imp)[6..], ["ha_action_raw"; 2]);
    let secs: Vec<Value> = body(&p)[..6]
        .iter()
        .map(|s| match &s.kind {
            StmtKind::Wait { seconds } => lit(seconds).clone(),
            _ => panic!(),
        })
        .collect();
    assert_eq!(secs, [json!(5), json!(300), json!(2.5), json!(30), json!(5400), json!(86400)]);
    let d = first_block(&imp).pointer("/next/block/inputs/SECONDS/block").unwrap();
    assert_eq!(d["type"], "duration");
    assert_eq!(d["fields"]["UNIT"], "minutes");
}

#[test]
fn wait_for_trigger() {
    let (imp, p) = script(
        r#"
  - wait_for_trigger:
      - trigger: state
        entity_id: binary_sensor.door
        to: "off"
    timeout: {minutes: 2}
  - if: "{{ wait.trigger is none }}"
    then: [{action: notify.notify, data: {message: open}}]
  - wait_for_trigger: {trigger: event, event_type: confirm}
    timeout: 30
    continue_on_timeout: false
  - delay: 1
"#,
    );
    assert_eq!(
        top(&imp),
        ["ha_wait_for_trigger", "controls_if", "ha_wait_for_trigger", "controls_if", "ha_wait"]
    );
    let StmtKind::WaitForTrigger { triggers, timeout: Some(t), .. } = &body(&p)[0].kind else { panic!() };
    assert_eq!(triggers.len(), 1);
    assert_eq!(lit(t), &json!(120));
    // continue_on_timeout: false stops the run on timeout.
    let StmtKind::If { branches, .. } = &body(&p)[3].kind else { panic!() };
    assert!(matches!(&branches[0].0.kind, ExprKind::Builtin { name, .. } if name == "wait_timed_out"));
    assert!(matches!(branches[0].1[0].kind, StmtKind::Stop));
}

#[test]
fn waits_set_wait_like_home_assistant() {
    // The blocks set all of `wait`, number the triggers like HA and render their templates
    // with the run's variables.
    let (imp, p) = script(
        r#"
  - delay: 1
  - wait_for_trigger:
      - {trigger: event, event_type: x}
      - triggers: [{trigger: template, value_template: "{{ is_state(door, 'on') }}"}]
    timeout: 10
  - action: notify.notify
    data: {message: "{{ wait.remaining }} {{ wait.trigger.id }} {{ wait.trigger.idx }}"}
  - wait_template: "{{ is_state('a.b', 'on') }}"
    timeout: "00:01:00"
    continue_on_timeout: false
  - if: "{{ wait.completed }}"
    then: [{delay: 1}]
"#,
    );
    assert_eq!(
        top(&imp),
        ["ha_wait", "ha_wait_for_trigger", "notify_send", "wait_until", "controls_if", "controls_if"]
    );
    // The template trigger and the "template is true" condition on `wait` are blocks too.
    assert_eq!(imp.raw_blocks, 0);
    let mut last_if = first_block(&imp);
    for _ in 0..5 {
        last_if = last_if.pointer("/next/block").unwrap();
    }
    assert_eq!(last_if["inputs"]["IF0"]["block"]["type"], "ha_template_condition");
    let StmtKind::WaitForTrigger { triggers, .. } = &body(&p)[1].kind else { panic!() };
    assert_eq!(triggers.len(), 2);
    let StmtKind::WaitUntil { cond, hold: None, timeout: Some(t) } = &body(&p)[3].kind else { panic!() };
    // Kept as a template, which is followed change by change.
    assert_eq!(cond.kind, ExprKind::Template { template: "{{ is_state('a.b', 'on') }}".into() });
    assert_eq!(lit(t), &json!(60));
    let StmtKind::If { branches, .. } = &body(&p)[4].kind else { panic!() };
    assert!(matches!(branches[0].1[0].kind, StmtKind::Stop));

    // A trigger split into several blocks shifts HA's numbers.
    let (imp, _) = script(
        r#"
  - wait_for_trigger:
      - {trigger: event, event_type: [x, y]}
      - {trigger: event, event_type: z}
  - action: notify.notify
    data: {message: "{{ wait.trigger.idx }}"}
"#,
    );
    assert_eq!(top(&imp), ["ha_action_raw"]);
    let (imp, _) = script(
        r#"
  - wait_for_trigger:
      - {trigger: event, event_type: [x, y]}
  - action: notify.notify
    data: {message: "{{ wait.trigger.event.event_type }}"}
"#,
    );
    assert_eq!(top(&imp), ["ha_wait_for_trigger", "notify_send"]);
    // A zero timeout times out at once in HA but means "no limit" for the blocks.
    let (imp, _) = script(
        r#"
  - wait_for_trigger: {trigger: event, event_type: x}
    timeout: 0
  - wait_template: "{{ is_state('a.b', 'on') }}"
    timeout: 0
"#,
    );
    assert_eq!(top(&imp), ["ha_action_raw"; 2]);
}

#[test]
fn waits_in_parallel_branches() {
    // HA keeps a branch's `wait` inside the branch.
    let (imp, _) = script(
        r#"
  - parallel:
      - wait_for_trigger: {trigger: event, event_type: x}
      - delay: 1
  - action: notify.notify
    data: {message: "{{ wait.trigger }}"}
"#,
    );
    assert_eq!(top(&imp), ["ha_action_raw"]);
    let (imp, _) = script(
        r#"
  - parallel:
      - - wait_for_trigger: {trigger: event, event_type: x}
        - action: notify.notify
          data: {message: "{{ wait.trigger.event.data }}"}
      - delay: 1
"#,
    );
    assert_eq!(top(&imp), ["control_parallel"]);
    assert_eq!(imp.raw_blocks, 0);
}

#[test]
fn events() {
    let (imp, p) = script(
        r#"
  - event: door_opened
    event_data: {by: door, n: 1}
  - event: ping
  - event: door_opened
    event_data_template: {who: "{{ trigger.entity_id }}"}
"#,
    );
    assert_eq!(top(&imp), ["ha_fire_event", "ha_fire_event", "ha_action_raw"]);
    let StmtKind::FireEvent { event_type, data: Some(d) } = &body(&p)[0].kind else { panic!() };
    assert_eq!(lit(event_type), &json!("door_opened"));
    assert_eq!(dict(d), [("by".into(), json!("door")), ("n".into(), json!(1))]);
    assert!(matches!(&body(&p)[1].kind, StmtKind::FireEvent { data: None, .. }));
}

#[test]
fn choose_and_if() {
    let (imp, p) = script(
        r#"
  - choose:
      - conditions: "{{ 1 == 1 }}"
        sequence:
          - delay: 1
      - alias: Night
        conditions:
          - condition: state
            entity_id: sun.sun
            state: below_horizon
        sequence: []
      - conditions: []
        sequence: [{delay: 3}]
    default:
      - delay: 2
  - if: "{{ true }}"
    then: [{delay: 1}]
    else: [{delay: 2}]
  - if: []
    then: [{delay: 1}]
"#,
    );
    assert_eq!(top(&imp), ["controls_if"; 3]);
    let StmtKind::If { branches, otherwise: Some(o) } = &body(&p)[0].kind else { panic!() };
    assert_eq!(branches.len(), 3);
    assert!(branches[1].1.is_empty());
    assert_eq!(lit(&branches[2].0), &json!(true));
    assert!(matches!(o[0].kind, StmtKind::Wait { .. }));
    assert!(matches!(&body(&p)[1].kind, StmtKind::If { otherwise: Some(_), .. }));
    assert!(matches!(&body(&p)[2].kind, StmtKind::If { otherwise: None, .. }));
}

#[test]
fn condition_actions_end_only_their_sequence() {
    let (imp, p) = script(
        r#"
  - if: "{{ true }}"
    then:
      - condition: state
        entity_id: input_boolean.x
        state: "on"
      - delay: 1
  - delay: 2
  - condition: "{{ is_state('a.b', 'on') }}"
  - and: ["{{ true }}"]
  - delay: 3
"#,
    );
    assert_eq!(top(&imp), ["controls_if", "ha_wait", "controls_if"]);
    let StmtKind::If { branches, .. } = &body(&p)[0].kind else { panic!() };
    let StmtKind::If { branches: inner, .. } = &branches[0].1[0].kind else { panic!() };
    assert!(matches!(inner[0].1[0].kind, StmtKind::Wait { .. }));
    // The top-level condition guards everything after it.
    let StmtKind::If { branches, .. } = &body(&p)[2].kind else { panic!() };
    let StmtKind::If { branches: inner, .. } = &branches[0].1[0].kind else { panic!() };
    assert!(matches!(inner[0].1[0].kind, StmtKind::Wait { .. }));
}

#[test]
fn repeats() {
    let (imp, p) = script(
        r#"
  - repeat:
      count: 3
      sequence: [{delay: 1}]
  - repeat:
      while: "{{ is_state('light.a', 'on') }}"
      sequence: [{delay: 1}]
  - repeat:
      until:
        - condition: state
          entity_id: light.a
          state: "off"
      sequence: [{delay: 1}]
  - repeat:
      for_each: [a, b]
      sequence: [{delay: 1}]
  - repeat:
      for_each: [a, b]
      sequence:
        - event: x
          event_data: {item: "{{ repeat.item }}"}
  - repeat:
      count: 2
      sequence:
        - repeat:
            while: "{{ repeat.index < 3 }}"
            sequence: [{delay: 1}]
  - repeat:
      count: "{{ 2 }}"
      sequence: [{delay: 1}]
"#,
    );
    assert_eq!(
        top(&imp),
        [
            "controls_repeat_ext",
            "controls_whileUntil",
            "controls_whileUntil",
            "controls_repeat_ext",
            "controls_forEach",
            "controls_repeat_ext",
            "ha_action_raw"
        ]
    );
    let b = body(&p);
    assert!(matches!(&b[0].kind, StmtKind::Repeat { times, .. } if lit(times) == &json!(3)));
    assert!(matches!(&b[1].kind, StmtKind::While { until: false, .. }));
    // `until` is checked after each round.
    let StmtKind::While { cond, body: inner, until: false } = &b[2].kind else { panic!() };
    assert_eq!(lit(cond), &json!(true));
    assert!(matches!(inner[0].kind, StmtKind::Wait { .. }));
    let StmtKind::If { branches, .. } = &inner[1].kind else { panic!() };
    assert!(matches!(branches[0].1[0].kind, StmtKind::Break));
    assert!(matches!(&b[3].kind, StmtKind::Repeat { times, .. } if lit(times) == &json!(2)));
    // `repeat.item` needs the items themselves.
    let StmtKind::ForEach { var, list, body: inner } = &b[4].kind else { panic!() };
    assert_eq!((var.as_str(), plain(list)), ("repeat_item", json!(["a", "b"])));
    // (An event with templates in its data has no block, but runs with `repeat` too.)
    assert!(matches!(inner[0].kind, StmtKind::HaScript { .. }));
    // The inner loop's condition sees its own `repeat`.
    let StmtKind::Repeat { body: inner, .. } = &b[5].kind else { panic!() };
    let StmtKind::While { cond, .. } = &inner[0].kind else { panic!() };
    let ExprKind::Binary { left, .. } = &cond.kind else { panic!("{cond:?}") };
    assert!(
        matches!(&left.kind, ExprKind::Builtin { name, args } if name == "loop_info" && lit(&args[0]) == "index")
    );
}

#[test]
fn for_each_over_templates_and_lists() {
    let (imp, p) = script(
        r#"
  - repeat:
      for_each: "{{ states.light | map(attribute='entity_id') | list }}"
      sequence:
        - action: light.turn_off
          target: {entity_id: "{{ repeat.item }}"}
  - variables: {repeat_item: 1}
  - repeat:
      for_each: [{name: a}, 2]
      sequence:
        - variables: {n: "{{ repeat.index }}", last: "{{ repeat.last }}", it: "{{ repeat.item }}"}
  - repeat:
      for_each: [light.a, {area_id: kitchen}]
      sequence:
        - variables: {it: "{{ repeat.item }}"}
"#,
    );
    assert_eq!(top(&imp), ["controls_forEach", "variables_set", "controls_forEach", "ha_action_raw"]);
    let b = body(&p);
    let StmtKind::ForEach { list, .. } = &b[0].kind else { panic!() };
    assert!(matches!(list.kind, ExprKind::Template { .. }));
    // A name the script doesn't use.
    let StmtKind::ForEach { var, body: inner, .. } = &b[2].kind else { panic!() };
    assert_eq!(var, "repeat_item_2");
    let set = |i: usize| match &inner[i].kind {
        StmtKind::Set { name, value, .. } => (name.clone(), plain(value)),
        other => panic!("{other:?}"),
    };
    let (name, index) = set(0);
    assert!(name == "n" && index.as_str().unwrap().contains(r#"loop_info", args: [Expr { id: "#), "{index}");
    assert!(index.as_str().unwrap().contains(r#"String("index")"#), "{index}");
    assert!(set(1).1.as_str().unwrap().contains(r#"String("last")"#));
    assert!(set(2).1.as_str().unwrap().contains(r#"String("item")"#));

    // A while loop has no `last` or `item` (undefined in HA): those stay templates.
    let (_, p) = script(
        r#"
  - repeat:
      while: "{{ repeat.index < 3 }}"
      sequence:
        - variables: {last: "{{ repeat.last }}", it: "{{ repeat.item }}", first: "{{ repeat.first }}"}
  - variables: {outside: "{{ repeat.index }}"}
"#,
    );
    // Outside any loop `repeat` is whatever HA has (nothing).
    assert!(
        matches!(&body(&p)[1].kind, StmtKind::Set { value, .. } if matches!(value.kind, ExprKind::Template { .. }))
    );
    let StmtKind::While { body: inner, .. } = &body(&p)[0].kind else { panic!() };
    let values: Vec<Value> = inner
        .iter()
        .map(|s| match &s.kind {
            StmtKind::Set { value, .. } => plain(value),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(values[0], json!({"template": "{{ repeat.last }}"}));
    assert_eq!(values[1], json!({"template": "{{ repeat.item }}"}));
    assert!(values[2].as_str().unwrap().contains(r#"String("first")"#));
}

#[test]
fn loop_bodies_that_hide_something_from_the_next_round() {
    let (imp, _) = script(
        r#"
  - repeat:
      count: 3
      sequence:
        - if: "{{ n is defined }}"
          then: [{delay: 1}]
        - repeat:
            count: "{{ 1 }}"
            sequence: [{variables: {n: 1}}]
"#,
    );
    assert_eq!(top(&imp), ["ha_action_raw"]);
}

#[test]
fn parallel_and_sequence() {
    let (imp, p) = script(
        r#"
  - parallel:
      - action: light.turn_on
        target: {entity_id: light.a}
      - sequence:
          - delay: 1
          - action: light.turn_on
            target: {entity_id: light.b}
      - - delay: 2
        - delay: 3
  - sequence:
      - delay: 4
      - delay: 5
"#,
    );
    assert_eq!(top(&imp), ["control_parallel", "ha_wait", "ha_wait"]);
    let StmtKind::Parallel { branches } = &body(&p)[0].kind else { panic!() };
    assert_eq!(branches.iter().map(Vec::len).collect::<Vec<_>>(), [1, 2, 2]);
    // A branch's lost variable read by another branch: the whole parallel is raw.
    let (imp, _) = script(
        r#"
  - parallel:
      - repeat:
          count: "{{ 1 }}"
          sequence: [{variables: {f: 1}}]
      - action: notify.notify
        data: {message: "{{ f }}"}
"#,
    );
    assert_eq!(top(&imp), ["ha_action_raw"]);
}

#[test]
fn variables_and_stop() {
    let (imp, p) = script(
        r#"
  - variables:
      count: 1
      ratio: 2.0
      label: "{{ count }} items"
      nested: {a: [1, "x"]}
  - variables: {trigger: 1}
  - stop: Done
    response_variable: count
  - delay: 1
"#,
    );
    assert_eq!(
        top(&imp),
        ["variables_set", "variables_set", "variables_set", "variables_set", "ha_action_raw", "control_stop"]
    );
    let b = body(&p);
    assert!(
        matches!(&b[0].kind, StmtKind::Set { name, value, .. } if name == "count" && lit(value) == &json!(1))
    );
    assert!(matches!(&b[1].kind, StmtKind::Set { value, .. } if lit(value).is_f64()));
    assert!(
        matches!(&b[2].kind, StmtKind::Set { value, .. } if matches!(value.kind, ExprKind::Template { .. }))
    );
    assert!(matches!(b[5].kind, StmtKind::Stop));
    assert_eq!(b.len(), 6);

    let (_, p) = script(
        r#"
  - stop: "Broken {{ x }}"
    error: true
"#,
    );
    let StmtKind::Throw { message } = &body(&p)[0].kind else { panic!() };
    assert_eq!(lit(message), &json!("Broken {{ x }}"));
}

#[test]
fn scenes_scripts_and_devices() {
    let (imp, p) = script(
        r#"
  - scene: scene.evening
  - action: script.turn_on
    target: {entity_id: script.welcome}
    data:
      variables: {who: "{{ trigger.to_state.name }}"}
  - action: script.welcome
    data: {who: guest}
  - device_id: abc123
    domain: light
    entity_id: 0123456789abcdef0123456789abcdef
    type: turn_on
  - set_conversation_response: "{{ 'ok' }}"
"#,
    );
    assert_eq!(
        top(&imp),
        ["scene_activate", "script_run", "ha_call_service", "device_action", "ha_action_raw"]
    );
    let b = body(&p);
    let StmtKind::CallService { service, target: Some(t), .. } = &b[0].kind else { panic!() };
    assert_eq!((service.as_str(), lit(t)), ("scene.turn_on", &json!("scene.evening")));
    let StmtKind::CallService { service, data: Some(d), .. } = &b[1].kind else { panic!() };
    assert_eq!(service, "script.turn_on");
    assert!(matches!(&dict(d)[..], [(k, _)] if k == "variables"));
    let StmtKind::HaScript { sequence, .. } = &b[3].kind else { panic!() };
    assert_eq!(sequence[0]["type"], "turn_on");
    assert_eq!(sequence[0]["device_id"], "abc123");
}

#[test]
fn helper_blocks_for_exact_matches() {
    let (imp, p) = script(
        r#"
  - action: notify.mobile_app_phone
    data: {title: Door, message: Open}
  - action: persistent_notification.create
    data: {message: Open}
  - action: notify.send_message
    target: {entity_id: notify.display}
    data: {message: Open}
  - action: tts.speak
    target: {entity_id: tts.piper}
    data: {message: Hi, media_player_entity_id: media_player.kitchen}
  - action: input_number.set_value
    target: {entity_id: input_number.level}
    data: {value: 3}
  - action: input_boolean.turn_on
    target: {entity_id: input_boolean.guest}
  - action: light.turn_on
    target: {entity_id: light.a}
    data: {brightness_pct: 40, transition: 3}
  - action: light.turn_on
    target: {entity_id: light.a}
    data: {brightness_pct: 40.5}
  - action: light.turn_on
    target: {entity_id: light.a}
    data: {color_temp_kelvin: 2700}
  - action: light.turn_on
    target: {entity_id: light.a}
    data: {rgb_color: [255, 128, 0], brightness_pct: 80}
  - action: light.turn_on
    target: {entity_id: light.a}
    data: {brightness_step_pct: -10}
  - action: light.turn_off
    target: {entity_id: light.a}
  - action: light.turn_off
    target: {entity_id: light.a}
    data: {transition: 0}
  - action: cover.set_cover_position
    target: {entity_id: cover.blind}
    data: {position: "{{ 50 }}"}
  - action: climate.set_hvac_mode
    target: {entity_id: climate.lounge}
    data: {hvac_mode: heat}
  - action: media_player.volume_set
    target: {entity_id: media_player.kitchen}
    data: {volume_level: 0.25}
  - action: homeassistant.turn_off
    target: {label_id: away}
  - action: fan.set_percentage
    target: {entity_id: fan.ceiling}
    data: {percentage: 30}
"#,
    );
    assert_eq!(
        top(&imp),
        [
            "notify_send",
            "notify_send",
            "ha_call_service",
            "media_say",
            "helper_set",
            "ha_call_service",
            "light_set_brightness",
            "ha_call_service",
            "light_set_white",
            "light_set_colour",
            "light_step",
            "light_power",
            "ha_call_service",
            "cover_set_position",
            "climate_set_mode",
            "media_set_volume",
            "device_power",
            "fan_set_speed",
        ]
    );
    let services: Vec<&str> = body(&p)
        .iter()
        .map(|s| match &s.kind {
            StmtKind::CallService { service, .. } => service.as_str(),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        services,
        [
            "notify.mobile_app_phone",
            "persistent_notification.create",
            "notify.send_message",
            "tts.speak",
            "input_number.set_value",
            "input_boolean.turn_on",
            "light.turn_on",
            "light.turn_on",
            "light.turn_on",
            "light.turn_on",
            "light.turn_on",
            "light.turn_off",
            "light.turn_off",
            "cover.set_cover_position",
            "climate.set_hvac_mode",
            "media_player.volume_set",
            "homeassistant.turn_off",
            "fan.set_percentage",
        ]
    );
    let StmtKind::CallService { data: Some(d), target: Some(t), .. } = &body(&p)[4].kind else { panic!() };
    assert_eq!(dict(d), [("value".into(), json!(3))]);
    assert_eq!(lit(t), &json!({"entity_id": "input_number.level"}));
}

#[test]
fn options_disabled_alias_and_errors() {
    let (imp, p) = script(
        r#"
  - alias: Careful
    action: light.turn_on
    target: {entity_id: light.a}
    continue_on_error: true
  - alias: Lights on
    action: light.turn_on
    target: {entity_id: light.a}
    continue_on_error: false
  - delay: 1
    continue_on_error: true
  - action: light.turn_on
    target: {entity_id: light.z}
    enabled: false
  - action: light.turn_on
    target: {entity_id: light.y}
    enabled: "{{ true }}"
  - stop: never
    enabled: false
  - delay: 2
"#,
    );
    assert_eq!(
        top(&imp),
        [
            "ha_action_raw",
            "light_power",
            "ha_wait",
            "light_power",
            "ha_action_raw",
            "ha_action_raw",
            "ha_wait"
        ]
    );
    assert_eq!(raw_config(&imp, 0)["continue_on_error"], true);
    let second = first_block(&imp).pointer("/next/block").unwrap();
    assert_eq!(second["icons"]["comment"]["text"], "Lights on");
    // Disabled actions are kept but don't run.
    let b = body(&p);
    assert_eq!(b.len(), 5);
    assert!(matches!(b[3].kind, StmtKind::HaScript { .. }));
    assert!(matches!(b[4].kind, StmtKind::Wait { .. }));
}

#[test]
fn every_action_fixture_compiles() {
    let text = include_str!("fixtures/actions/every_action.yaml");
    let out = import_yaml(text).unwrap();
    let imp = &out[0];
    let p = compile(&imp.workspace, &NoExternals).unwrap_or_else(|e| panic!("{e:?}"));
    assert_eq!(p.handlers.len(), 1);
    let types = top(imp);
    eprintln!("{types:?} raw={}", imp.raw_blocks);
    for expected in [
        "ha_call_service",
        "notify_send",
        "media_say",
        "helper_set",
        "light_set_brightness",
        "light_set_white",
        "light_set_colour",
        "light_step",
        "light_power",
        "cover_command",
        "cover_set_position",
        "climate_set_mode",
        "climate_set_temperature",
        "media_set_volume",
        "media_command",
        "device_power",
        "lock_command",
        "vacuum_command",
        "counter_command",
        "fan_set_speed",
        "script_run",
        "scene_activate",
        "ha_wait",
        "ha_wait_for_trigger",
        "ha_fire_event",
        "controls_if",
        "controls_repeat_ext",
        "controls_whileUntil",
        "control_parallel",
        "variables_set",
        "device_action",
        "ha_action_raw",
    ] {
        assert!(types.iter().any(|t| t == expected), "{expected} missing from {types:?}");
    }
    // Nothing after the stop survives.
    assert!(!serde_json::to_string(&imp.workspace).unwrap().contains("light.never"));
}
