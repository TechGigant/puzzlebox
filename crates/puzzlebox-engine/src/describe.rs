//! Triggers in words: what a program waits for ("Movement turns on"), what started a run
//! ("Movement turned on") and plausible data for running a program as if a trigger fired.

use puzzlebox_ir::{HandlerKind, Program, Trigger};
use serde_json::{Value, json};

use crate::ha::HaApi;

fn name(ha: &dyn HaApi, entity_id: &str) -> String {
    ha.state(entity_id)
        .and_then(|s| s.pointer("/attributes/friendly_name").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_else(|| entity_id.to_owned())
}

fn entities(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::String(s)) => {
            s.split(',').map(|e| e.trim().to_owned()).filter(|e| !e.is_empty()).collect()
        }
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_owned).collect(),
        _ => vec![],
    }
}

fn names(ha: &dyn HaApi, v: Option<&Value>) -> String {
    let list = entities(v);
    match list.as_slice() {
        [] => "something".into(),
        [one] => name(ha, one),
        [first, rest @ ..] => format!("{} (or {} more)", name(ha, first), rest.len()),
    }
}

fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().map(text).collect::<Vec<_>>().join(" or "),
        other => other.to_string(),
    }
}

fn duration(v: &Value) -> Option<String> {
    let secs = match v {
        Value::Number(n) => n.as_f64()?,
        Value::Object(o) => {
            let get = |k: &str| o.get(k).and_then(Value::as_f64).unwrap_or(0.0);
            get("days") * 86400.0 + get("hours") * 3600.0 + get("minutes") * 60.0 + get("seconds")
        }
        Value::String(s) => {
            let parts: Vec<f64> = s.split(':').filter_map(|p| p.parse().ok()).collect();
            parts.iter().fold(0.0, |acc, p| acc * 60.0 + p)
        }
        _ => return None,
    };
    Some(seconds(secs))
}

/// "30 s", "5 min", "1 h 30 min".
pub fn seconds(secs: f64) -> String {
    let s = secs.round() as u64;
    match s {
        0..60 => format!("{} s", if secs < 10.0 { (secs * 10.0).round() / 10.0 } else { s as f64 }),
        60..3600 if s.is_multiple_of(60) => format!("{} min", s / 60),
        60..3600 => format!("{} min {} s", s / 60, s % 60),
        _ if s.is_multiple_of(3600) => format!("{} h", s / 3600),
        _ => format!("{} h {} min", s / 3600, (s % 3600) / 60),
    }
}

/// How a state reads after "turns"/"turned": `on`, `off`, or "changes to X".
fn state_change(to: &str, past: bool) -> String {
    let verb = |present: &str, past_form: &str| if past { past_form.to_owned() } else { present.to_owned() };
    match to {
        "on" => verb("turns on", "turned on"),
        "off" => verb("turns off", "turned off"),
        "unavailable" => verb("becomes unavailable", "became unavailable"),
        other => format!("{} {other}", verb("changes to", "changed to")),
    }
}

/// A trigger's config in words, as in "when … ": "Movement turns on for 5 min".
pub fn trigger_config(config: &Value, ha: &dyn HaApi) -> String {
    let kind = config.get("trigger").or_else(|| config.get("platform")).and_then(Value::as_str).unwrap_or("");
    let for_ = config.get("for").and_then(duration).map(|d| format!(" for {d}")).unwrap_or_default();
    match kind {
        "state" => {
            let who = names(ha, config.get("entity_id"));
            let what = match config.get("attribute").and_then(Value::as_str) {
                Some(a) => format!("{who} {a}"),
                None => who,
            };
            match (config.get("to"), config.get("not_to"), config.get("from")) {
                (Some(to), _, _) if !to.is_null() => match to.as_str() {
                    Some(s) => format!("{what} {}{for_}", state_change(s, false)),
                    None => format!("{what} changes to {}{for_}", text(to)),
                },
                (_, Some(not), _) => format!("{what} changes to anything but {}{for_}", text(not)),
                (_, _, Some(from)) if !from.is_null() => format!("{what} changes from {}{for_}", text(from)),
                _ => format!("{what} changes{for_}"),
            }
        }
        "numeric_state" => {
            let who = names(ha, config.get("entity_id"));
            let num = |k: &str| config.get(k).filter(|v| !v.is_null()).map(text);
            match (num("above"), num("below")) {
                (Some(a), Some(b)) => format!("{who} is between {a} and {b}{for_}"),
                (Some(a), None) => format!("{who} goes above {a}{for_}"),
                (None, Some(b)) => format!("{who} goes below {b}{for_}"),
                _ => format!("{who} changes{for_}"),
            }
        }
        "time" => match config.get("at") {
            Some(Value::String(at)) if at.contains('.') => format!("it is the time in {}", name(ha, at)),
            Some(Value::String(at)) => format!("it is {}", at.trim_end_matches(":00")),
            Some(Value::Object(o)) => {
                format!(
                    "it is the time in {}",
                    name(ha, o.get("entity_id").and_then(Value::as_str).unwrap_or(""))
                )
            }
            Some(other) => format!("it is {}", text(other)),
            None => "it is time".into(),
        },
        "time_pattern" => {
            let every = ["hours", "minutes", "seconds"].into_iter().find_map(|k| {
                config.get(k).and_then(Value::as_str).and_then(|v| v.strip_prefix('/')).map(|n| (k, n))
            });
            match every {
                Some((unit, "1")) => format!("every {}", unit.trim_end_matches('s')),
                Some((unit, n)) => format!("every {n} {unit}"),
                None => "on a time pattern".into(),
            }
        }
        "homeassistant" => match config.get("event").and_then(Value::as_str) {
            Some("shutdown") => "Home Assistant shuts down".into(),
            _ => "Home Assistant starts".into(),
        },
        "sun" => match config.get("event").and_then(Value::as_str) {
            Some(e) => format!("at {e}"),
            None => "the sun rises or sets".into(),
        },
        "zone" => {
            let who = names(ha, config.get("entity_id"));
            let zone = name(ha, config.get("zone").and_then(Value::as_str).unwrap_or("a zone"));
            match config.get("event").and_then(Value::as_str) {
                Some("leave") => format!("{who} leaves {zone}"),
                _ => format!("{who} enters {zone}"),
            }
        }
        "template" => format!("a template becomes true{for_}"),
        "event" => format!("event “{}” fires", config.get("event_type").map(text).unwrap_or_default()),
        "mqtt" => format!("an MQTT message arrives on {}", config.get("topic").map(text).unwrap_or_default()),
        "webhook" => "its webhook is called".into(),
        "tag" => "a tag is scanned".into(),
        "conversation" => match config.get("command") {
            Some(c) => format!("you say “{}”", text(c).split(" or ").next().unwrap_or("")),
            None => "you say something".into(),
        },
        "calendar" => format!(
            "an event in {} {}",
            names(ha, config.get("entity_id")),
            if config.get("event").and_then(Value::as_str) == Some("end") { "ends" } else { "starts" }
        ),
        "device" => "a device does something".into(),
        "" => "something happens".into(),
        // Integration triggers such as `light.turned_on`.
        other => {
            let words = other.replace(['.', '_'], " ");
            match config.pointer("/target/entity_id") {
                Some(e) => {
                    format!("{} {}", names(ha, Some(e)), words.split_once(' ').map_or(&*words, |(_, w)| w))
                }
                None => words,
            }
        }
    }
}

/// Any trigger (HA's or the engine's own) in words.
pub fn trigger(t: &Trigger, ha: &dyn HaApi) -> String {
    match t {
        Trigger::Ha { config, .. } => trigger_config(config, ha),
        Trigger::Cron { expr, .. } => format!("the schedule “{expr}” comes round"),
        Trigger::Webhook { webhook_id, .. } => format!("webhook {webhook_id} is called"),
        Trigger::Mqtt { topic, .. } => format!("an MQTT message arrives on {topic}"),
    }
}

/// The program's entry points in one line: "when Movement turns on (+1)".
pub fn program(p: &Program, ha: &dyn HaApi) -> String {
    let mut parts = p.handlers.iter().filter_map(|h| match &h.kind {
        HandlerKind::Triggers { triggers } => Some(match triggers.as_slice() {
            [] => "when (no trigger)".to_owned(),
            [one] => format!("when {}", trigger(one, ha)),
            [first, rest @ ..] => format!("when {} (+{})", trigger(first, ha), rest.len()),
        }),
        HandlerKind::OnStart => Some("when the program starts".to_owned()),
        _ => None,
    });
    let Some(first) = parts.next() else { return String::new() };
    match parts.count() {
        0 => first,
        n => format!("{first} · {n} more"),
    }
}

/// Whether the program can never do anything: no entry block with both a trigger and blocks
/// to run. Loose stacks and functions don't count, since nothing starts them.
pub fn does_nothing(p: &Program) -> bool {
    !p.handlers.iter().any(|h| match &h.kind {
        HandlerKind::Triggers { triggers } => !triggers.is_empty() && !h.body.is_empty(),
        HandlerKind::OnStart => !h.body.is_empty(),
        _ => false,
    })
}

fn state_text(s: Option<&Value>) -> Option<String> {
    s.and_then(|s| s.get("state")).and_then(Value::as_str).map(str::to_owned)
}

fn state_name(s: Option<&Value>, entity_id: &str, ha: &dyn HaApi) -> String {
    s.and_then(|s| s.pointer("/attributes/friendly_name"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| name(ha, entity_id))
}

/// What started a run, from its trigger data: "Movement turned on", "run manually".
pub fn fired(t: &Value, ha: &dyn HaApi) -> String {
    let what = fired_inner(t, ha);
    if t.get("simulated").and_then(Value::as_bool) == Some(true) { format!("run as if {what}") } else { what }
}

fn fired_inner(t: &Value, ha: &dyn HaApi) -> String {
    let platform = t.get("platform").or_else(|| t.get("trigger")).and_then(Value::as_str).unwrap_or("");
    let s = |k: &str| t.get(k).and_then(Value::as_str).unwrap_or("");
    match platform {
        "manual" => "run manually".into(),
        "start" => "the program started".into(),
        "timer" => {
            let n = s("timer");
            let n = n.split_once(':').map_or(n, |(base, _)| base);
            format!("timer “{n}” went off")
        }
        "state" => {
            let entity = s("entity_id");
            let (from, to) =
                (t.get("from_state").filter(|v| !v.is_null()), t.get("to_state").filter(|v| !v.is_null()));
            let who = state_name(to.or(from), entity, ha);
            if let Some(attr) = t.get("attribute").and_then(Value::as_str) {
                let v =
                    to.and_then(|s| s.pointer(&format!("/attributes/{attr}"))).map(text).unwrap_or_default();
                return format!("{who} {attr} changed to {v}");
            }
            match (state_text(from), state_text(to)) {
                (_, None) => format!("{who} was removed"),
                (Some(f), Some(t)) if f == t => format!("{who} changed (still {t})"),
                (Some(f), Some(t)) if !["on", "off"].contains(&t.as_str()) => {
                    format!("{who} changed from {f} to {t}")
                }
                (_, Some(t)) => format!("{who} {}", state_change(&t, true)),
            }
        }
        "numeric_state" => {
            let entity = s("entity_id");
            let to = t.get("to_state");
            let who = state_name(to, entity, ha);
            let now = state_text(to).unwrap_or_default();
            match (t.get("above").filter(|v| !v.is_null()), t.get("below").filter(|v| !v.is_null())) {
                (Some(a), Some(b)) => format!("{who} is {now}, between {} and {}", text(a), text(b)),
                (Some(a), None) => format!("{who} rose above {} ({now})", text(a)),
                (None, Some(b)) => format!("{who} fell below {} ({now})", text(b)),
                _ => format!("{who} is {now}"),
            }
        }
        "time" => match t.get("now").and_then(Value::as_str) {
            Some(now) => format!("it was {}", now.get(11..16).unwrap_or(now)),
            None => "it was time".into(),
        },
        "time_pattern" => "the time pattern matched".into(),
        "cron" => "the schedule came round".into(),
        "homeassistant" => match s("event") {
            "shutdown" => "Home Assistant is shutting down".into(),
            _ => "Home Assistant started".into(),
        },
        "sun" => format!("it was {}", s("event")),
        "zone" => {
            let who = state_name(t.get("to_state"), s("entity_id"), ha);
            let zone =
                t.pointer("/zone/attributes/friendly_name").and_then(Value::as_str).unwrap_or("a zone");
            if s("event") == "leave" { format!("{who} left {zone}") } else { format!("{who} entered {zone}") }
        }
        "event" => {
            format!("event “{}”", t.pointer("/event/event_type").and_then(Value::as_str).unwrap_or(""))
        }
        "mqtt" => format!("MQTT message on {}", s("topic")),
        "webhook" => format!("webhook {} was called", s("webhook_id")),
        "template" => "a template became true".into(),
        "tag" => "a tag was scanned".into(),
        "conversation" => match t.get("sentence").and_then(Value::as_str) {
            Some(said) => format!("someone said “{said}”"),
            None => "a voice command".into(),
        },
        "calendar" => format!(
            "calendar event “{}”",
            t.pointer("/calendar_event/summary").and_then(Value::as_str).unwrap_or("")
        ),
        _ => match t.get("description").and_then(Value::as_str) {
            Some(d) if !d.is_empty() => d.to_owned(),
            _ if platform.is_empty() => "something".into(),
            _ => platform.replace(['.', '_'], " "),
        },
    }
}

fn with_state(current: Option<Value>, entity_id: &str, state: Option<&str>) -> Value {
    let mut s =
        current.unwrap_or_else(|| json!({"entity_id": entity_id, "state": "unknown", "attributes": {}}));
    if let Some(v) = state {
        s["state"] = json!(v);
    }
    s
}

/// Trigger data that looks like `t` fired, for testing a program from the editor: for a
/// state trigger, the entity's current state as `to_state` (or the state it waits for).
pub fn simulate(t: &Trigger, ha: &dyn HaApi) -> Value {
    let id = t.block_id();
    let mut data = match t {
        Trigger::Ha { config, .. } => {
            let kind = config
                .get("trigger")
                .or_else(|| config.get("platform"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let entity = entities(config.get("entity_id")).into_iter().next().unwrap_or_default();
            let mut d = json!({"platform": kind, "description": format!("{kind} (run from the editor)")});
            match kind {
                "state" => {
                    let current = ha.state(&entity);
                    let to = config.get("to").and_then(|v| v.as_str().or_else(|| v.get(0)?.as_str()));
                    let from = config.get("from").and_then(|v| v.as_str().or_else(|| v.get(0)?.as_str()));
                    let from = from.or(match (to, state_text(current.as_ref()).as_deref()) {
                        (Some("on"), _) => Some("off"),
                        (Some("off"), _) => Some("on"),
                        _ => None,
                    });
                    d["entity_id"] = json!(entity);
                    d["from_state"] = with_state(current.clone(), &entity, from);
                    d["to_state"] = with_state(current, &entity, to);
                    d["for"] = Value::Null;
                    d["attribute"] = config.get("attribute").cloned().unwrap_or(Value::Null);
                }
                "numeric_state" => {
                    let current = ha.state(&entity);
                    let num = |k: &str| config.get(k).and_then(Value::as_f64);
                    let now = state_text(current.as_ref()).and_then(|s| s.parse::<f64>().ok());
                    let value = match (num("above"), num("below"), now) {
                        (a, b, Some(n)) if a.is_none_or(|a| n > a) && b.is_none_or(|b| n < b) => n,
                        (Some(a), Some(b), _) => (a + b) / 2.0,
                        (Some(a), None, _) => a + 1.0,
                        (None, Some(b), _) => b - 1.0,
                        (None, None, n) => n.unwrap_or(0.0),
                    };
                    let value = crate::value::num(value);
                    d["entity_id"] = json!(entity);
                    d["from_state"] = with_state(current.clone(), &entity, None);
                    d["to_state"] = with_state(current, &entity, Some(&crate::value::to_text(&value)));
                    d["above"] = config.get("above").cloned().unwrap_or(Value::Null);
                    d["below"] = config.get("below").cloned().unwrap_or(Value::Null);
                }
                "time" | "time_pattern" => {
                    d["now"] = json!(chrono::Local::now().to_rfc3339());
                }
                "homeassistant" | "sun" | "zone" => {
                    d["event"] = config.get("event").cloned().unwrap_or(Value::Null);
                }
                "event" => {
                    d["event"] = json!({"event_type": config.get("event_type"), "data": config.get("event_data").cloned().unwrap_or(json!({}))});
                }
                "mqtt" => {
                    d["topic"] = config.get("topic").cloned().unwrap_or_default();
                    d["payload"] = config.get("payload").cloned().unwrap_or(json!(""));
                }
                "webhook" => d["webhook_id"] = config.get("webhook_id").cloned().unwrap_or_default(),
                "conversation" => {
                    let first = config.get("command").map(text).unwrap_or_default();
                    d["sentence"] = json!(first.split(" or ").next().unwrap_or(""));
                }
                _ => {}
            }
            d["id"] = config.get("id").cloned().unwrap_or_else(|| json!(id));
            d["idx"] = json!("0");
            d["alias"] = config.get("alias").cloned().unwrap_or(Value::Null);
            d
        }
        Trigger::Cron { .. } => json!({"platform": "cron", "id": id}),
        Trigger::Webhook { webhook_id, .. } => {
            json!({"platform": "webhook", "webhook_id": webhook_id, "id": id, "json": {}, "query": {}, "data": {}})
        }
        Trigger::Mqtt { topic, .. } => json!({"platform": "mqtt", "topic": topic, "payload": "", "id": id}),
    };
    data["simulated"] = json!(true);
    data
}
