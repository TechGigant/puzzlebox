//! Facts about a workspace that aren't errors, for the program list.

use serde_json::Value;

use crate::compile::Block;

/// True when nothing in the program can ever run: no "when" with a trigger, start or "run"
/// block has any (enabled) block to do. Loose stacks and functions don't count, since they
/// only run when asked to.
pub fn does_nothing(workspace: &Value) -> bool {
    let tops = workspace.pointer("/blocks/blocks").and_then(Value::as_array);
    !tops.into_iter().flatten().map(Block).any(|b| {
        let runs = match b.ty() {
            "ha_when" => has_enabled(b.input("TRIGGERS")),
            "ha_on_start" | "ha_on_run" => true,
            _ => false,
        };
        b.enabled() && runs && has_enabled(b.input("DO"))
    })
}

fn has_enabled(mut b: Option<Block<'_>>) -> bool {
    while let Some(x) = b {
        if x.enabled() {
            return true;
        }
        b = x.next();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ws(blocks: Value) -> Value {
        json!({"blocks": {"languageVersion": 0, "blocks": blocks}})
    }

    #[test]
    fn empty_and_unfinished_programs_do_nothing() {
        assert!(does_nothing(&json!({})));
        assert!(does_nothing(&ws(json!([{"type": "ha_when", "id": "w"}]))));
        let no_do = json!([{"type": "ha_when", "id": "w",
            "inputs": {"TRIGGERS": {"block": {"type": "ha_trigger_state", "id": "t"}}}}]);
        assert!(does_nothing(&ws(no_do)));
        let loose = json!([{"type": "ha_wait", "id": "a"}]);
        assert!(does_nothing(&ws(loose)));
        let disabled = json!([{"type": "ha_on_start", "id": "s",
            "inputs": {"DO": {"block": {"type": "ha_wait", "id": "a", "enabled": false}}}}]);
        assert!(does_nothing(&ws(disabled)));
    }

    #[test]
    fn a_when_with_trigger_and_action_does_something() {
        let full = json!([{"type": "ha_when", "id": "w", "inputs": {
            "TRIGGERS": {"block": {"type": "ha_trigger_state", "id": "t"}},
            "DO": {"block": {"type": "ha_wait", "id": "a", "enabled": false,
                "next": {"block": {"type": "ha_wait", "id": "b"}}}}}}]);
        assert!(!does_nothing(&ws(full)));
        let run = json!([{"type": "ha_on_run", "id": "r",
            "inputs": {"DO": {"block": {"type": "ha_wait", "id": "a"}}}}]);
        assert!(!does_nothing(&ws(run)));
    }
}
