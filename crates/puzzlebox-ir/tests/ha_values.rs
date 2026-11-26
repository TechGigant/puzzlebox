//! The "like Home Assistant" value blocks lower to their Python-style builtins.

use puzzlebox_ir::*;
use serde_json::{Value, json};

/// Compiles `set x to <value>` and returns the value's expression.
fn lower(value: Value) -> Result<ExprKind, Vec<CompileError>> {
    let ws = json!({
        "blocks": {"languageVersion": 0, "blocks": [{
            "type": "ha_on_run", "id": "run",
            "inputs": {"DO": {"block": {"type": "variables_set", "id": "set", "fields": {"VAR": {"id": "v"}},
                "inputs": {"VALUE": {"block": value}}}}}
        }]},
        "variables": [{"name": "x", "id": "v"}]
    });
    let p = compile(&ws, &NoExternals)?;
    let StmtKind::Set { value, .. } = &p.handlers[0].body[0].kind else { panic!() };
    Ok(value.kind.clone())
}

fn text(s: &str) -> Value {
    json!({"shadow": {"type": "text", "id": format!("t{s}"), "fields": {"TEXT": s}}})
}

fn builtin(kind: &ExprKind) -> (&str, Vec<Value>) {
    let ExprKind::Builtin { name, args } = kind else { panic!("{kind:?}") };
    let args = args
        .iter()
        .map(|a| match &a.kind {
            ExprKind::Lit { value } => value.clone(),
            other => json!(format!("{other:?}").split_whitespace().next().unwrap_or("")),
        })
        .collect();
    (name, args)
}

#[test]
fn lowers_to_python_style_builtins() {
    let cases = [
        (
            json!({"type": "to_number_ha", "id": "b", "fields": {"KIND": "int"}, "inputs": {"VALUE": text("2.5")}}),
            "py_int",
            vec![json!("2.5")],
        ),
        (
            json!({"type": "to_number_ha", "id": "b", "fields": {"KIND": "float"},
                "inputs": {"VALUE": text("x"), "DEFAULT": text("0")}}),
            "py_float",
            vec![json!("x"), json!("0")],
        ),
        (
            json!({"type": "math_round_ha", "id": "b", "fields": {"DECIMALS": 2, "METHOD": "half"}, "inputs": {"NUM": text("2.5")}}),
            "py_round",
            vec![json!("2.5"), json!(2), json!("half")],
        ),
        (
            json!({"type": "to_text_ha", "id": "b", "fields": {"NUMBERS": "float"}, "inputs": {"VALUE": text("a")}}),
            "py_text",
            vec![json!("a"), json!("float")],
        ),
        (
            json!({"type": "logic_compare_strict", "id": "b", "fields": {"OP": "LT"}, "inputs": {"A": text("10"), "B": text("9")}}),
            "py_compare",
            vec![json!("10"), json!("9"), json!("LT")],
        ),
        (
            json!({"type": "logic_is_null", "id": "b", "inputs": {"VALUE": text("")}}),
            "is_null",
            vec![json!("")],
        ),
        (
            json!({"type": "time_format", "id": "b", "fields": {"FORMAT": "%H"}, "inputs": {"DATETIME": text("07:00")}}),
            "time_format",
            vec![json!("07:00"), json!("%H")],
        ),
    ];
    for (block, name, args) in cases {
        let kind = lower(block.clone()).unwrap();
        assert_eq!(builtin(&kind), (name, args), "{block}");
    }
}

#[test]
fn empty_slots_are_errors_except_the_default() {
    assert!(lower(json!({"type": "to_number_ha", "id": "b", "fields": {"KIND": "float"}})).is_err());
    assert!(lower(json!({"type": "logic_compare_strict", "id": "b", "fields": {"OP": "EQ"}, "inputs": {"A": text("1")}})).is_err());
    let kind = lower(json!({"type": "to_number_ha", "id": "b", "inputs": {"VALUE": text("1")}})).unwrap();
    assert_eq!(builtin(&kind), ("py_float", vec![json!("1")]));
}
