use puzzlebox_ir::*;
use serde_json::{Value, json};

fn run(first: Value) -> Vec<Stmt> {
    let ws = json!({"blocks": {"blocks": [{"type": "ha_on_run", "id": "run", "inputs": {"DO": {"block": first}}}]}});
    let p = compile(&ws, &NoExternals).unwrap_or_else(|e| panic!("{e:?}"));
    p.handlers[0].body.clone()
}

fn num(id: &str, n: f64) -> Value {
    json!({"block": {"type": "math_number", "id": id, "fields": {"NUM": n}}})
}

fn wait_trigger(id: &str, fields: Value, timeout: Option<Value>) -> Value {
    let mut b = json!({"type": "ha_wait_for_trigger", "id": id, "fields": fields, "inputs": {"TRIGGERS": {"block":
        {"type": "ha_trigger_raw", "id": format!("{id}t"), "fields": {"CONFIG": "trigger: event\nevent_type: a"}}}}});
    if let Some(t) = timeout {
        b["inputs"]["TIMEOUT"] = t;
    }
    b
}

fn wait_until(id: &str, fields: Value, timeout: Option<Value>) -> Value {
    let mut b = json!({"type": "wait_until", "id": id, "fields": fields, "inputs": {
        "COND": {"block": {"type": "logic_boolean", "id": format!("{id}c"), "fields": {"BOOL": "TRUE"}}}}});
    if let Some(t) = timeout {
        b["inputs"]["TIMEOUT"] = t;
    }
    b
}

fn chain(mut blocks: Vec<Value>) -> Value {
    let mut next = blocks.pop().unwrap();
    while let Some(mut b) = blocks.pop() {
        b["next"] = json!({"block": next});
        next = b;
    }
    next
}

#[test]
fn time_limits_and_older_waits() {
    let body = run(chain(vec![
        wait_trigger("a", json!({"LIMIT": "limit"}), Some(num("n1", 0.0))),
        wait_trigger("b", json!({"LIMIT": "none"}), Some(num("n2", 30.0))),
        // Saved before "no time limit": 0 meant no limit.
        wait_trigger("c", json!({}), Some(num("n3", 0.0))),
        wait_until("d", json!({"LIMIT": "limit"}), Some(num("n4", 0.0))),
        wait_until("e", json!({"LIMIT": "none"}), Some(num("n5", 5.0))),
        wait_until("f", json!({}), Some(num("n6", 0.0))),
    ]));
    let lim = |s: &Stmt| match &s.kind {
        StmtKind::WaitForTrigger { timeout, zero_is_no_limit, .. }
        | StmtKind::WaitUntil { timeout, zero_is_no_limit, .. } => {
            (timeout.as_ref().map(|t| t.kind.clone()), *zero_is_no_limit)
        }
        other => panic!("{other:?}"),
    };
    let zero = Some(ExprKind::Lit { value: json!(0) });
    assert_eq!(lim(&body[0]), (zero.clone(), false));
    assert_eq!(lim(&body[1]), (None, false));
    assert_eq!(lim(&body[2]), (zero.clone(), true));
    assert_eq!(lim(&body[3]), (zero.clone(), false));
    assert_eq!(lim(&body[4]), (None, false));
    assert_eq!(lim(&body[5]), (zero, true));
}

#[test]
fn carry_on_if_this_fails() {
    let call =
        |id: &str| json!({"type": "ha_call_service", "id": id, "fields": {"SERVICE": "light.turn_on"}});
    let carry = |id: &str, inner: Value| json!({"type": "control_carry_on", "id": id, "inputs": {"DO": {"block": inner}}});
    let raw = |id: &str, yaml: &str| json!({"type": "ha_action_raw", "id": id, "fields": {"CONFIG": yaml}});
    let body = run(chain(vec![
        carry("w1", chain(vec![call("c1"), call("c2")])),
        carry("w2", raw("r1", "action: light.turn_on\ntarget: {entity_id: light.a}")),
        carry("w3", raw("r2", "- delay: 1\n- action: light.turn_on")),
        carry("w4", raw("r3", "action: light.turn_on\ncontinue_on_error: false")),
    ]));
    let StmtKind::ContinueOnError { body: inner } = &body[0].kind else { panic!("{:?}", body[0]) };
    assert_eq!(inner.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["c1", "c2"]);
    // A lone native action gets HA's own continue_on_error.
    let native = |s: &Stmt| {
        let StmtKind::If { branches, .. } = &s.kind else { panic!("{s:?}") };
        let [Stmt { id, kind: StmtKind::HaScript { sequence, .. } }] = &branches[0].1[..] else { panic!() };
        (id.clone(), sequence.clone())
    };
    assert_eq!(
        native(&body[1]),
        (
            "r1".into(),
            json!([{"action": "light.turn_on", "target": {"entity_id": "light.a"}, "continue_on_error": true}])
        )
    );
    assert_eq!(
        native(&body[2]).1,
        json!([{"sequence": [{"delay": 1}, {"action": "light.turn_on"}], "continue_on_error": true}])
    );
    assert_eq!(
        native(&body[3]).1,
        json!([{"sequence": [{"action": "light.turn_on", "continue_on_error": false}], "continue_on_error": true}])
    );
}
