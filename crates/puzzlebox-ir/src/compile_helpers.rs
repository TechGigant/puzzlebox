//! One-block helpers for common devices (covers, climate, media, fans, locks, vacuums,
//! scenes, scripts, notifications, input helpers). Each compiles to an ordinary action call
//! or state read, so it behaves exactly like the generated Home Assistant blocks.

use serde_json::{Value, json};

use super::{Block, Compiler, arg_placeholder};
use crate::ir::*;

impl Compiler<'_> {
    fn service_call(&mut self, b: &Block<'_>, service: &str, data: Vec<(&str, Expr)>) -> StmtKind {
        let target = self.value_opt(b, "TARGET");
        if target.is_none() {
            self.err(b, "choose what to control");
        }
        let entries = data.into_iter().map(|(k, v)| (k.to_owned(), v)).collect::<Vec<_>>();
        let data =
            (!entries.is_empty()).then(|| Expr { id: b.id().into(), kind: ExprKind::Dict { entries } });
        StmtKind::CallService { service: service.into(), target, data, response_var: None }
    }

    fn pick<'a>(&self, b: &Block<'_>, field: &str, options: &[(&str, &'a str)], default: &'a str) -> &'a str {
        let v = b.field_str(field).unwrap_or("");
        options.iter().find(|(k, _)| *k == v).map_or(default, |(_, s)| s)
    }

    /// Helper action blocks; `None` if `b` isn't one.
    pub(super) fn helper_stmt(&mut self, b: &Block<'_>) -> Option<StmtKind> {
        let lit = |v: Value| Expr::lit(b.id(), v);
        Some(match b.ty() {
            "device_power" => {
                let s = self.pick(
                    b,
                    "MODE",
                    &[("off", "homeassistant.turn_off"), ("toggle", "homeassistant.toggle")],
                    "homeassistant.turn_on",
                );
                self.service_call(b, s, vec![])
            }
            "cover_command" => {
                let s = self.pick(
                    b,
                    "MODE",
                    &[
                        ("close", "cover.close_cover"),
                        ("stop", "cover.stop_cover"),
                        ("open_tilt", "cover.open_cover_tilt"),
                        ("close_tilt", "cover.close_cover_tilt"),
                    ],
                    "cover.open_cover",
                );
                self.service_call(b, s, vec![])
            }
            "cover_set_position" => {
                let v = self.value(b, "POSITION");
                let s = if b.field_str("WHAT") == Some("tilt") {
                    "cover.set_cover_tilt_position"
                } else {
                    "cover.set_cover_position"
                };
                let key = if s.ends_with("tilt_position") { "tilt_position" } else { "position" };
                self.service_call(b, s, vec![(key, v)])
            }
            "climate_set_temperature" => {
                let v = self.value(b, "TEMPERATURE");
                self.service_call(b, "climate.set_temperature", vec![("temperature", v)])
            }
            "climate_set_mode" => {
                let mode = b.field_str("MODE").unwrap_or("heat").to_owned();
                self.service_call(b, "climate.set_hvac_mode", vec![("hvac_mode", lit(json!(mode)))])
            }
            "climate_set_preset" => {
                let v = self.value(b, "PRESET");
                self.service_call(b, "climate.set_preset_mode", vec![("preset_mode", v)])
            }
            "media_command" => {
                let mode = b.field_str("MODE").unwrap_or("play");
                match mode {
                    "mute" | "unmute" => self.service_call(
                        b,
                        "media_player.volume_mute",
                        vec![("is_volume_muted", lit(json!(mode == "mute")))],
                    ),
                    _ => {
                        let s = self.pick(
                            b,
                            "MODE",
                            &[
                                ("pause", "media_player.media_pause"),
                                ("play_pause", "media_player.media_play_pause"),
                                ("stop", "media_player.media_stop"),
                                ("next", "media_player.media_next_track"),
                                ("previous", "media_player.media_previous_track"),
                                ("on", "media_player.turn_on"),
                                ("off", "media_player.turn_off"),
                            ],
                            "media_player.media_play",
                        );
                        self.service_call(b, s, vec![])
                    }
                }
            }
            "media_set_volume" => {
                // Blocks use percent; HA wants 0–1.
                let v = self.value(b, "VOLUME");
                let level = Expr {
                    id: b.id().into(),
                    kind: ExprKind::Binary {
                        bin: BinOp::Div,
                        left: Box::new(v),
                        right: Box::new(lit(json!(100))),
                    },
                };
                self.service_call(b, "media_player.volume_set", vec![("volume_level", level)])
            }
            "media_play" => {
                let id = self.value(b, "MEDIA");
                let ty = b.field_str("TYPE").unwrap_or("music").to_owned();
                self.service_call(
                    b,
                    "media_player.play_media",
                    vec![("media_content_id", id), ("media_content_type", lit(json!(ty)))],
                )
            }
            "media_say" => {
                let message = self.value(b, "MESSAGE");
                let players = self.value_opt(b, "TARGET").unwrap_or_else(|| lit(Value::Null));
                let tts = b.field_str("TTS").map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned);
                let Some(tts) = tts else {
                    self.err(b, "choose a voice (a text-to-speech entity, tts.…)");
                    return Some(StmtKind::Expr { expr: lit(Value::Null) });
                };
                let entries =
                    vec![("message".to_owned(), message), ("media_player_entity_id".to_owned(), players)];
                StmtKind::CallService {
                    service: "tts.speak".into(),
                    target: Some(lit(json!({"entity_id": tts}))),
                    data: Some(Expr { id: b.id().into(), kind: ExprKind::Dict { entries } }),
                    response_var: None,
                }
            }
            "fan_set_speed" => {
                let v = self.value(b, "SPEED");
                self.service_call(b, "fan.set_percentage", vec![("percentage", v)])
            }
            "lock_command" => {
                let s =
                    self.pick(b, "MODE", &[("unlock", "lock.unlock"), ("open", "lock.open")], "lock.lock");
                self.service_call(b, s, vec![])
            }
            "vacuum_command" => {
                let s = self.pick(
                    b,
                    "MODE",
                    &[
                        ("pause", "vacuum.pause"),
                        ("stop", "vacuum.stop"),
                        ("return_to_base", "vacuum.return_to_base"),
                        ("locate", "vacuum.locate"),
                    ],
                    "vacuum.start",
                );
                self.service_call(b, s, vec![])
            }
            "scene_activate" => self.service_call(b, "scene.turn_on", vec![]),
            "script_run" => match self.value_opt(b, "DATA") {
                Some(d) => self.service_call(b, "script.turn_on", vec![("variables", d)]),
                None => self.service_call(b, "script.turn_on", vec![]),
            },
            "counter_command" => {
                let s = self.pick(
                    b,
                    "MODE",
                    &[("decrement", "counter.decrement"), ("reset", "counter.reset")],
                    "counter.increment",
                );
                self.service_call(b, s, vec![])
            }
            "notify_send" => {
                let message = self.value(b, "MESSAGE");
                let mut entries = vec![("message".to_owned(), message)];
                if let Some(t) = self.value_opt(b, "TITLE") {
                    entries.push(("title".to_owned(), t));
                }
                let to = b.field_str("TO").map(str::trim).unwrap_or("").to_owned();
                let service = match to.as_str() {
                    "" | "persistent" => "persistent_notification.create".to_owned(),
                    s if s.starts_with("notify.") => s.to_owned(),
                    s => format!("notify.{s}"),
                };
                StmtKind::CallService {
                    service,
                    target: None,
                    data: Some(Expr { id: b.id().into(), kind: ExprKind::Dict { entries } }),
                    response_var: None,
                }
            }
            "helper_set" => return Some(self.helper_set(b)),
            "device_on_for" => {
                // Turn on now; a timer per target turns that same target off again later.
                let on =
                    Stmt { id: b.id().into(), kind: self.service_call(b, "homeassistant.turn_on", vec![]) };
                let target = self.value_opt(b, "TARGET").unwrap_or_else(|| lit(Value::Null));
                let off = Stmt {
                    id: b.id().into(),
                    kind: StmtKind::CallService {
                        service: "homeassistant.turn_off".into(),
                        target: Some(Expr {
                            id: b.id().into(),
                            kind: ExprKind::TriggerData { path: "data".into() },
                        }),
                        data: None,
                        response_var: None,
                    },
                };
                let name = format!("__on_for_{}", b.id());
                self.timer_handlers.push(Handler {
                    id: b.id().into(),
                    kind: HandlerKind::Timer { name: name.clone() },
                    body: vec![off],
                });
                let start = Stmt {
                    id: b.id().into(),
                    kind: StmtKind::TimerStart {
                        name,
                        seconds: self.value(b, "SECONDS"),
                        if_running: TimerMode::Restart,
                        handler: b.id().into(),
                        data: Some(target.clone()),
                        key: Some(target),
                    },
                };
                StmtKind::If { branches: vec![(lit(json!(true)), vec![on, start])], otherwise: None }
            }
            "wait_until_time" => {
                let at = self.required_str(b, "TIME");
                StmtKind::WaitForTrigger {
                    triggers: vec![json!({"trigger": "time", "at": at})],
                    timeout: None,
                    result_var: None,
                }
            }
            "http_send" => {
                let kind = ExprKind::Http {
                    method: b.field_str("METHOD").unwrap_or("POST").to_owned(),
                    url: Box::new(self.value(b, "URL")),
                    body: self.value_opt(b, "BODY").map(Box::new),
                    headers: None,
                };
                StmtKind::Expr { expr: Expr { id: b.id().into(), kind } }
            }
            _ => return None,
        })
    }

    /// "set <input helper> to <value>": the service depends on the helper's domain.
    fn helper_set(&mut self, b: &Block<'_>) -> StmtKind {
        let entity = self.required_str(b, "ENTITY");
        let value = self.value(b, "VALUE");
        let target = Some(Expr::lit(b.id(), json!({"entity_id": entity})));
        let call = |service: &str, key: &str, v: Expr| StmtKind::CallService {
            service: service.into(),
            target: target.clone(),
            data: Some(Expr {
                id: b.id().into(),
                kind: ExprKind::Dict { entries: vec![(key.to_owned(), v)] },
            }),
            response_var: None,
        };
        let domain = entity.split('.').next().unwrap_or("");
        match domain {
            "input_number" | "number" | "input_text" | "text" | "counter" => {
                call(&format!("{domain}.set_value"), "value", value)
            }
            "input_select" | "select" => call(&format!("{domain}.select_option"), "option", value),
            "input_datetime" => StmtKind::CallService {
                service: "input_datetime.set_datetime".into(),
                target,
                data: Some(Expr {
                    id: b.id().into(),
                    kind: ExprKind::Builtin { name: "datetime_data".into(), args: vec![value] },
                }),
                response_var: None,
            },
            "input_boolean" | "switch" | "light" | "fan" => {
                let on = Stmt {
                    id: b.id().into(),
                    kind: StmtKind::CallService {
                        service: "homeassistant.turn_on".into(),
                        target: target.clone(),
                        data: None,
                        response_var: None,
                    },
                };
                let off = Stmt {
                    id: b.id().into(),
                    kind: StmtKind::CallService {
                        service: "homeassistant.turn_off".into(),
                        target,
                        data: None,
                        response_var: None,
                    },
                };
                // "on", "true", "home", 1 … turn it on; "off", false, 0 … turn it off.
                let is_on = Expr {
                    id: b.id().into(),
                    kind: ExprKind::Builtin { name: "is_on".into(), args: vec![value] },
                };
                StmtKind::If { branches: vec![(is_on, vec![on])], otherwise: Some(vec![off]) }
            }
            _ => {
                self.err(b, format!("'{entity}' isn't a helper this block can set (number, text, dropdown, toggle, date/time or counter helpers)"));
                StmtKind::Expr { expr: Expr::lit(b.id(), Value::Null) }
            }
        }
    }

    /// Helper trigger blocks; `None` if `b` isn't one.
    pub(super) fn helper_trigger(&mut self, b: &Block<'_>) -> Option<Option<Trigger>> {
        let id: BlockId = b.id().into();
        Some(match b.ty() {
            "ha_trigger_becomes" => {
                let entity = self.required_str(b, "ENTITY");
                let state = self.required_str(b, "STATE");
                let states = super::triggers::comma_list(&state).into_iter().map(Value::from).collect();
                let to = super::triggers::one_or_list(states).unwrap_or(json!(state));
                let mut cfg = json!({"trigger": "state", "entity_id": entity, "to": to});
                if let Some(d) = self.fixed_number(b, "FOR").filter(|d| *d > 0.0) {
                    cfg["for"] = json!({"seconds": d});
                }
                Some(Trigger::Ha { id, config: cfg })
            }
            "ha_trigger_zone" => {
                let who = self.required_str(b, "PERSON");
                let zone = self.required_str(b, "ZONE");
                let event = if b.field_str("EVENT") == Some("leave") { "leave" } else { "enter" };
                Some(Trigger::Ha {
                    id,
                    config: json!({"trigger": "zone", "entity_id": who, "zone": zone, "event": event}),
                })
            }
            _ => return None,
        })
    }

    /// Helper value blocks; `None` if `b` isn't one.
    pub(super) fn helper_expr(&mut self, b: &Block<'_>) -> Option<ExprKind> {
        let attr = |c: &mut Self, name: &str| ExprKind::State {
            entity: Box::new(c.entity_expr(b)),
            attribute: Some(Box::new(Expr::lit(b.id(), json!(name)))),
        };
        Some(match b.ty() {
            "cover_position_of" => attr(
                self,
                if b.field_str("WHAT") == Some("tilt") {
                    "current_tilt_position"
                } else {
                    "current_position"
                },
            ),
            "climate_temperature_of" => attr(
                self,
                if b.field_str("WHICH") == Some("target") { "temperature" } else { "current_temperature" },
            ),
            "media_volume_of" => {
                let level = Expr { id: b.id().into(), kind: attr(self, "volume_level") };
                let pct = Expr {
                    id: b.id().into(),
                    kind: ExprKind::Binary {
                        bin: BinOp::Mul,
                        left: Box::new(level),
                        right: Box::new(Expr::lit(b.id(), json!(100))),
                    },
                };
                self.builtin("math_round", vec![pct])
            }
            "media_title_of" => attr(self, "media_title"),
            "fan_speed_of" => attr(self, "percentage"),
            "wait_timed_out" | "wait_entity" => {
                let wait = Expr {
                    id: b.id().into(),
                    kind: ExprKind::Var { name: "wait".into(), scope: VarScope::Local },
                };
                self.builtin(b.ty(), vec![wait])
            }
            "trigger_entity" => ExprKind::TriggerData { path: "entity_id".into() },
            "sun_is" => {
                let state =
                    if b.field_str("WHERE") == Some("down") { "below_horizon" } else { "above_horizon" };
                ExprKind::Condition {
                    config: json!({"condition": "state", "entity_id": "sun.sun", "state": state}),
                    args: Default::default(),
                }
            }
            "someone_home" => {
                let test = match b.field_str("WHO") {
                    Some("nobody") => "== 0",
                    Some("everyone") => "== states.person | count",
                    _ => "> 0",
                };
                ExprKind::Template {
                    template: format!(
                        "{{{{ (states.person | selectattr('state', 'eq', 'home') | list | count) {test} and (states.person | count) > 0 }}}}"
                    ),
                }
            }
            "person_is_home" => {
                let mut args = std::collections::BTreeMap::new();
                args.insert("entity".to_owned(), self.entity_expr(b));
                let state = if b.field_str("WHERE") == Some("away") { "not_home" } else { "home" };
                ExprKind::Condition {
                    config: json!({"condition": "state", "entity_id": arg_placeholder("entity"), "state": state}),
                    args,
                }
            }
            "weekday_is" => {
                let days: Vec<&str> = match b.field_str("WHEN").unwrap_or("weekday") {
                    "weekend" => vec!["sat", "sun"],
                    "weekday" => vec!["mon", "tue", "wed", "thu", "fri"],
                    d => vec![d],
                };
                ExprKind::Condition {
                    config: json!({"condition": "time", "weekday": days}),
                    args: Default::default(),
                }
            }
            "ha_state_count" => {
                let list = self.value(b, "LIST");
                let f = |k: &str, d: &str| Expr::lit(b.id(), json!(b.field_str(k).unwrap_or(d)));
                let (op, state) = (f("OP", "is"), f("STATE", "on"));
                self.builtin("ha:state_check", vec![list, Expr::lit(b.id(), json!("count")), op, state])
            }
            "ha_state_ago" => {
                let e = self.entity_expr(b);
                let secs = self.value(b, "SECONDS");
                self.builtin(
                    "ha:history",
                    vec![e, Expr::lit(b.id(), json!("state_ago")), secs, Expr::lit(b.id(), json!(""))],
                )
            }
            "text_regex_test" => {
                let args = vec![
                    self.value(b, "TEXT"),
                    Expr::lit(b.id(), json!(b.field_str("PATTERN").unwrap_or(""))),
                ];
                self.builtin("regex_test", args)
            }
            "colour_hex" => {
                let args = vec![self.value(b, "COLOUR"), Expr::lit(b.id(), json!("hex"))];
                self.builtin("colour_part", args)
            }
            "time_of_day" => self.builtin("time_of_day", vec![]),
            _ => return None,
        })
    }
}
