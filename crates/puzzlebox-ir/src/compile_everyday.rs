//! Everyday blocks: presence, remembering and restoring states, alarm panels, buttons and
//! notifications that ask a question. Each compiles to the same trigger, action or condition
//! config Home Assistant documents for it.

use serde_json::{Value, json};

use super::triggers::fixed;
use super::{Block, Compiler, duration_or_none};
use crate::ir::*;

/// The run variable "the reply" reads.
const REPLY_VAR: &str = "__reply";
/// The run variable holding the current question's action id prefix (unique per question).
const REPLY_ID_VAR: &str = "__reply_id";

const ALARM_MODES: [(&str, &str); 5] = [
    ("away", "alarm_control_panel.alarm_arm_away"),
    ("home", "alarm_control_panel.alarm_arm_home"),
    ("night", "alarm_control_panel.alarm_arm_night"),
    ("vacation", "alarm_control_panel.alarm_arm_vacation"),
    ("custom_bypass", "alarm_control_panel.alarm_arm_custom_bypass"),
];

const ARMED: [&str; 5] = ["armed_away", "armed_home", "armed_night", "armed_vacation", "armed_custom_bypass"];

/// A scene id as HA's `cv.slug` accepts it: lower case letters, digits and single underscores.
fn scene_slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    out.trim_matches('_').to_owned()
}

/// The button labels of "ask": comma-separated, blanks dropped.
fn button_labels(text: &str) -> Vec<String> {
    text.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect()
}

fn stmt(b: &Block<'_>, kind: StmtKind) -> Stmt {
    Stmt { id: b.id().into(), kind }
}

fn local(b: &Block<'_>, name: &str) -> Expr {
    Expr { id: b.id().into(), kind: ExprKind::Var { name: name.into(), scope: VarScope::Local } }
}

fn call(service: &str, target: Option<Expr>, data: Option<Expr>) -> StmtKind {
    StmtKind::CallService { service: service.into(), target, data, response_var: None }
}

impl Compiler<'_> {
    fn dict(&self, b: &Block<'_>, entries: Vec<(&str, Expr)>) -> Option<Expr> {
        let entries: Vec<(String, Expr)> = entries.into_iter().map(|(k, v)| (k.to_owned(), v)).collect();
        (!entries.is_empty()).then(|| Expr { id: b.id().into(), kind: ExprKind::Dict { entries } })
    }

    fn target(&mut self, b: &Block<'_>, what: &str) -> Option<Expr> {
        let t = self.value_opt(b, "TARGET");
        if t.is_none() {
            self.err(b, format!("choose {what}"));
        }
        t
    }

    /// An optional code: a fixed empty text means none.
    fn code(&mut self, b: &Block<'_>) -> Vec<(&'static str, Expr)> {
        match self.value_opt(b, "CODE") {
            Some(e)
                if fixed(&e)
                    .is_some_and(|v| v.is_null() || v.as_str().is_some_and(|s| s.trim().is_empty())) =>
            {
                vec![]
            }
            Some(e) => vec![("code", e)],
            None => vec![],
        }
    }

    pub(super) fn everyday_stmt(&mut self, b: &Block<'_>) -> Option<StmtKind> {
        Some(match b.ty() {
            "scene_remember" => {
                let name = scene_slug(b.field_str("NAME").unwrap_or(""));
                if name.is_empty() {
                    self.err(b, "give the remembered state a name");
                }
                let what = self.target(b, "what to remember");
                if let Some(Value::Object(_)) = what.as_ref().and_then(fixed) {
                    self.err(b, "areas, floors, labels and devices can't be remembered; choose entities (or use \"entities in …\")");
                }
                let mut entries = vec![("scene_id", Expr::lit(b.id(), json!(name)))];
                entries.extend(what.map(|w| ("snapshot_entities", w)));
                call("scene.create", None, self.dict(b, entries))
            }
            "scene_restore" => {
                let name = scene_slug(b.field_str("NAME").unwrap_or(""));
                if name.is_empty() {
                    self.err(b, "choose which remembered state to restore");
                }
                call(
                    "scene.turn_on",
                    Some(Expr::lit(b.id(), json!({"entity_id": format!("scene.{name}")}))),
                    None,
                )
            }
            "alarm_arm" => {
                let mode = b.field_str("MODE").unwrap_or("away");
                let service =
                    ALARM_MODES.iter().find(|(m, _)| *m == mode).map_or(ALARM_MODES[0].1, |(_, s)| s);
                let target = self.target(b, "the alarm");
                let data = self.code(b);
                call(service, target, self.dict(b, data))
            }
            "alarm_disarm" => {
                let target = self.target(b, "the alarm");
                let data = self.code(b);
                call("alarm_control_panel.alarm_disarm", target, self.dict(b, data))
            }
            "button_press" => {
                let target = self.target(b, "the button");
                let ids: Vec<String> = match target.as_ref().and_then(fixed) {
                    Some(Value::String(s)) => s.split(',').map(|s| s.trim().to_owned()).collect(),
                    Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect(),
                    _ => vec![],
                };
                let helpers = ids.iter().filter(|e| e.starts_with("input_button.")).count();
                if helpers > 0 && helpers < ids.len() {
                    self.err(b, "press buttons and button helpers (input_button) with separate blocks");
                }
                let service =
                    if helpers > 0 && helpers == ids.len() { "input_button.press" } else { "button.press" };
                call(service, target, None)
            }
            "notify_ask" => return Some(self.notify_ask(b)),
            _ => return None,
        })
    }

    /// Sends a mobile_app notification with action buttons, then waits for the
    /// `mobile_app_notification_action` event of one of them, like HA's "Confirmable
    /// notification" blueprint. The action ids start with a fresh random id each time, so
    /// replies to an earlier question or another run never end this wait.
    fn notify_ask(&mut self, b: &Block<'_>) -> StmtKind {
        let to = b.field_str("TO").map(str::trim).unwrap_or("").trim_start_matches("notify.").to_owned();
        if !to.starts_with("mobile_app_") {
            self.err(
                b,
                "choose a phone with the Home Assistant app (notify.mobile_app_…); only it can show buttons",
            );
        }
        let labels = button_labels(b.field_str("BUTTONS").unwrap_or(""));
        if labels.is_empty() {
            self.err(b, "add at least one button, e.g. \"Yes, No\"");
        }
        let message = self.value(b, "MESSAGE");
        let title = self.value_opt(b, "TITLE");
        let (timeout, _) = self.wait_limit(b);

        let lit = |v: Value| Expr::lit(b.id(), v);
        let builtin = |name: &str, args: Vec<Expr>| Expr {
            id: b.id().into(),
            kind: ExprKind::Builtin { name: name.into(), args },
        };
        let new_id = StmtKind::Set {
            name: REPLY_ID_VAR.into(),
            scope: VarScope::Local,
            value: builtin("text_join", vec![lit(json!("PUZZLEBOX_")), builtin("unique_id", vec![])]),
        };
        let actions = labels
            .iter()
            .enumerate()
            .map(|(i, label)| {
                let action = builtin("text_join", vec![local(b, REPLY_ID_VAR), lit(json!(format!("_{i}")))]);
                let entries = vec![("action".to_owned(), action), ("title".to_owned(), lit(json!(label)))];
                Expr { id: b.id().into(), kind: ExprKind::Dict { entries } }
            })
            .collect();
        let actions = Expr { id: b.id().into(), kind: ExprKind::List { items: actions } };
        let mut entries = vec![("message", message)];
        entries.extend(title.map(|t| ("title", t)));
        entries.push(("data", self.dict(b, vec![("actions", actions)]).expect("not empty")));
        let send = call(&format!("notify.{to}"), None, self.dict(b, entries));

        let triggers = (0..labels.len())
            .map(|i| {
                json!({
                    "trigger": "event",
                    "event_type": "mobile_app_notification_action",
                    "event_data": {"action": format!("{{{{ {REPLY_ID_VAR} }}}}_{i}")},
                })
            })
            .collect();
        let wait = StmtKind::WaitForTrigger { triggers, timeout, result_var: None, zero_is_no_limit: false };
        // HA numbers the triggers in order (`trigger.idx`), so it says which button it was.
        let by_idx: serde_json::Map<String, Value> =
            labels.iter().enumerate().map(|(i, l)| (i.to_string(), json!(l))).collect();
        let fired = builtin("dict_get", vec![local(b, "wait"), lit(json!("trigger"))]);
        let idx = builtin("dict_get", vec![fired, lit(json!("idx"))]);
        let reply = StmtKind::Set {
            name: REPLY_VAR.into(),
            scope: VarScope::Local,
            value: builtin("dict_get", vec![lit(Value::Object(by_idx)), idx]),
        };
        let body = vec![stmt(b, new_id), stmt(b, send), stmt(b, wait), stmt(b, reply)];
        StmtKind::If { branches: vec![(lit(json!(true)), body)], otherwise: None }
    }

    pub(super) fn everyday_trigger(&mut self, b: &Block<'_>) -> Option<Option<Trigger>> {
        Some(match b.ty() {
            "ha_trigger_occupancy" => {
                let zone = self.required_str(b, "ZONE");
                let which = if b.field_str("EVENT") == Some("leave") {
                    "occupancy_cleared"
                } else {
                    "occupancy_detected"
                };
                let mut options = json!({"zone": zone});
                if let Some(d) = duration_or_none(self.fixed_number(b, "FOR")) {
                    options["for"] = d;
                }
                Some(Trigger::Ha {
                    id: b.id().into(),
                    config: json!({"trigger": format!("zone.{which}"), "options": options}),
                })
            }
            _ => return None,
        })
    }

    pub(super) fn everyday_expr(&mut self, b: &Block<'_>) -> Option<ExprKind> {
        Some(match b.ty() {
            "notify_reply" => ExprKind::Var { name: REPLY_VAR.into(), scope: VarScope::Local },
            "alarm_is" => {
                let states: Vec<&str> = match b.field_str("STATE").unwrap_or("armed") {
                    "armed" => ARMED.to_vec(),
                    s => vec![s],
                };
                let mut args = std::collections::BTreeMap::new();
                args.insert("entity".to_owned(), self.entity_expr(b));
                let state = if states.len() == 1 { json!(states[0]) } else { json!(states) };
                ExprKind::Condition {
                    config: json!({"condition": "state", "entity_id": arg_placeholder("entity"), "state": state}),
                    args,
                }
            }
            _ => return None,
        })
    }
}
