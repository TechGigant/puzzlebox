//! One-block helpers for common devices (covers, climate, media, fans, locks, vacuums,
//! scenes, scripts, notifications, input helpers). Each compiles to an ordinary action call
//! or state read, so it behaves exactly like the generated Home Assistant blocks.

use serde_json::{Value, json};

use super::{Block, Compiler};
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
                    self.err(b, "choose a text-to-speech entity (tts.…)");
                    return Some(StmtKind::Stop);
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
            "input_datetime" => call("input_datetime.set_datetime", "datetime", value),
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
                StmtKind::If { branches: vec![(value, vec![on])], otherwise: Some(vec![off]) }
            }
            _ => {
                self.err(b, format!("'{entity}' isn't a helper this block can set (input_number, input_text, input_select, input_boolean, input_datetime, counter …)"));
                StmtKind::Stop
            }
        }
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
            _ => return None,
        })
    }
}
