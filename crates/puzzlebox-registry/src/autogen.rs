//! Generates blocks from Home Assistant's own descriptions: `get_services`,
//! `trigger_platforms/subscribe`, `condition_platforms/subscribe`, plus translations.

use serde_json::{Map, Value, json};

use crate::model::*;

/// `frontend/get_translations` resources merged across categories.
pub type Translations = Map<String, Value>;

pub struct HaDescriptions<'a> {
    pub services: &'a Value,
    pub triggers: &'a Value,
    pub conditions: &'a Value,
    pub translations: &'a Translations,
    /// `config_entries/get` result, for config-entry selectors (may be `Null`).
    pub config_entries: &'a Value,
}

pub fn humanize(key: &str) -> String {
    let s = key.replace(['_', '.'], " ");
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

fn tr<'a>(t: &'a Translations, key: &str) -> Option<&'a str> {
    t.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}

pub fn domain_title(t: &Translations, domain: &str) -> String {
    tr(t, &format!("component.{domain}.title")).map(str::to_owned).unwrap_or_else(|| humanize(domain))
}

/// Flattens HA's collapsible field sections (`{"advanced_fields": {"fields": {...}}}`).
fn flatten_fields(fields: &Value) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    if let Some(map) = fields.as_object() {
        for (k, v) in map {
            match v.get("fields") {
                Some(inner) if v.get("selector").is_none() => out.extend(flatten_fields(inner)),
                _ => out.push((k.clone(), v.clone())),
            }
        }
    }
    out
}

fn domains_of(v: &Value) -> Vec<String> {
    match v {
        Value::String(s) => vec![s.clone()],
        Value::Array(a) => a.iter().filter_map(Value::as_str).map(str::to_owned).collect(),
        _ => vec![],
    }
}

fn target_spec(target: &Value) -> TargetSpec {
    let (mut domains, mut device_classes) = (Vec::new(), Vec::new());
    for e in target.get("entity").and_then(Value::as_array).into_iter().flatten() {
        for d in domains_of(e.get("domain").unwrap_or(&Value::Null)) {
            if !domains.contains(&d) {
                domains.push(d);
            }
        }
        for c in domains_of(e.get("device_class").unwrap_or(&Value::Null)) {
            if !device_classes.contains(&c) {
                device_classes.push(c);
            }
        }
    }
    TargetSpec { domains, device_classes, required: false }
}

/// Maps an HA selector to an editor input.
fn selector_input(
    selector: &Value,
    t: &Translations,
    entries: &Value,
    domain: &str,
    mode: &str,
) -> ParamInput {
    let Some((kind, cfg)) = selector.as_object().and_then(|m| m.iter().next()) else {
        return ParamInput::Any;
    };
    let f = |k: &str| cfg.get(k).and_then(Value::as_f64);
    let multiple = cfg.get("multiple").and_then(Value::as_bool).unwrap_or(false);
    match kind.as_str() {
        "number" => ParamInput::Number {
            min: f("min"),
            max: f("max"),
            step: f("step"),
            unit: cfg.get("unit_of_measurement").and_then(Value::as_str).map(str::to_owned),
        },
        "color_temp" => {
            ParamInput::Number { min: f("min"), max: f("max"), step: Some(1.0), unit: Some("K".into()) }
        }
        "boolean" => ParamInput::Boolean,
        "text" | "template" => {
            if multiple {
                ParamInput::Select { options: vec![], multiple: true }
            } else {
                ParamInput::Text { multiline: cfg.get("multiline").and_then(Value::as_bool).unwrap_or(false) }
            }
        }
        "select" => {
            let key = cfg.get("translation_key").and_then(Value::as_str);
            let options = cfg
                .get("options")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|o| {
                    let (value, label) = match o {
                        Value::String(s) => (s.clone(), None),
                        Value::Object(m) => (
                            m.get("value")?.as_str()?.to_owned(),
                            m.get("label").and_then(Value::as_str).map(str::to_owned),
                        ),
                        _ => return None,
                    };
                    let translated = key
                        .and_then(|k| tr(t, &format!("component.{domain}.selector.{k}.options.{value}")))
                        .map(str::to_owned);
                    Some(SelectOption {
                        label: translated.or(label).unwrap_or_else(|| humanize(&value)),
                        value,
                    })
                })
                .collect::<Vec<_>>();
            let custom = cfg.get("custom_value").and_then(Value::as_bool) == Some(true);
            if options.is_empty() && !multiple {
                ParamInput::Text { multiline: false }
            } else {
                // Free values (custom_value) need a text field; suggestions still list the options.
                ParamInput::Select { options, multiple: multiple || custom }
            }
        }
        "automation_behavior" => {
            let opts: &[(&str, &str)] =
                if mode == "condition" || cfg.get("mode").and_then(Value::as_str) == Some("condition") {
                    &[("any", "any of them"), ("all", "all of them")]
                } else {
                    &[("each", "any of them"), ("first", "the first one"), ("all", "all of them")]
                };
            ParamInput::Select {
                multiple: false,
                options: opts
                    .iter()
                    .map(|(v, l)| SelectOption { label: (*l).into(), value: (*v).into() })
                    .collect(),
            }
        }
        "entity" => {
            let mut domains = domains_of(cfg.get("domain").unwrap_or(&Value::Null));
            let mut device_classes = domains_of(cfg.get("device_class").unwrap_or(&Value::Null));
            for flt in cfg.get("filter").and_then(Value::as_array).into_iter().flatten() {
                domains.extend(domains_of(flt.get("domain").unwrap_or(&Value::Null)));
                device_classes.extend(domains_of(flt.get("device_class").unwrap_or(&Value::Null)));
            }
            ParamInput::Entity { domains, multiple, device_classes }
        }
        "area" | "floor" | "label" | "device" => ParamInput::Registry { kind: kind.clone(), multiple },
        "config_entry" => {
            let integration = cfg.get("integration").and_then(Value::as_str);
            let options: Vec<SelectOption> = entries
                .as_array()
                .into_iter()
                .flatten()
                .filter(|e| integration.is_none_or(|i| e.get("domain").and_then(Value::as_str) == Some(i)))
                .filter_map(|e| {
                    Some(SelectOption {
                        label: e.get("title").and_then(Value::as_str).unwrap_or("entry").to_owned(),
                        value: e.get("entry_id")?.as_str()?.to_owned(),
                    })
                })
                .collect();
            if options.is_empty() {
                ParamInput::Text { multiline: false }
            } else {
                ParamInput::Select { options, multiple: false }
            }
        }
        "duration" => ParamInput::Duration,
        "numeric_threshold" => ParamInput::Threshold {
            mode: cfg.get("mode").and_then(Value::as_str).unwrap_or("is").to_owned(),
            units: cfg
                .get("unit_of_measurement")
                .and_then(Value::as_array)
                .map(|u| u.iter().map(|v| v.as_str().map(str::to_owned)).collect())
                .unwrap_or_default(),
        },
        "state" => ParamInput::StateValue {
            attribute: cfg.get("attribute").and_then(Value::as_str).map(str::to_owned),
            multiple,
        },
        "time" => ParamInput::Time,
        "color_rgb" => ParamInput::Color,
        "date" | "datetime" | "icon" | "conversation_agent" if !multiple => {
            ParamInput::Text { multiline: false }
        }
        _ => ParamInput::Any,
    }
}

fn params_from(
    fields: &Value,
    t: &Translations,
    entries: &Value,
    prefix: &str,
    domain: &str,
    mode: &str,
    // The legacy `entity_id` field duplicates the target slot.
    skip: &[&str],
) -> Vec<Param> {
    let mut params: Vec<Param> = flatten_fields(fields)
        .into_iter()
        .filter(|(name, _)| !skip.contains(&name.as_str()))
        .map(|(name, spec)| {
            let input =
                selector_input(spec.get("selector").unwrap_or(&Value::Null), t, entries, domain, mode);
            let mut default = spec.get("default").cloned();
            if matches!(input, ParamInput::Duration) {
                default = default.as_ref().and_then(duration_seconds).map(|s| json!(s));
            }
            if default.is_none() && spec.get("required").and_then(Value::as_bool) == Some(true) {
                default = match &input {
                    ParamInput::Select { options, multiple: false } => {
                        options.first().map(|o| json!(o.value))
                    }
                    _ => None,
                };
            }
            Param {
                label: tr(t, &format!("{prefix}.fields.{name}.name"))
                    .map(str::to_owned)
                    .unwrap_or_else(|| humanize(&name)),
                description: tr(t, &format!("{prefix}.fields.{name}.description"))
                    .or_else(|| spec.get("description").and_then(Value::as_str))
                    .unwrap_or("")
                    .to_owned(),
                required: spec.get("required").and_then(Value::as_bool).unwrap_or(false),
                input,
                default,
                name,
            }
        })
        .collect();
    params.sort_by_key(|p| !p.required);
    params
}

/// `"01:02:03"`, `{"hours": 1, ...}` or a number → seconds.
fn duration_seconds(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => {
            let parts: Vec<f64> = s.split(':').map(|p| p.trim().parse().ok()).collect::<Option<_>>()?;
            Some(parts.iter().fold(0.0, |acc, p| acc * 60.0 + p))
        }
        Value::Object(m) => {
            let g = |k: &str| m.get(k).and_then(Value::as_f64).unwrap_or(0.0);
            Some(g("days") * 86400.0 + g("hours") * 3600.0 + g("minutes") * 60.0 + g("seconds"))
        }
        _ => None,
    }
}

fn sanitize(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect()
}

/// Splits `"light.turned_on"` into (`light`, `turned_on`); bare names belong to `homeassistant`.
fn split_key(key: &str) -> (&str, &str) {
    key.split_once('.').unwrap_or(("homeassistant", key))
}

pub fn generate(d: &HaDescriptions<'_>) -> Vec<BlockDef> {
    let t = d.translations;
    let mut out = Vec::new();

    for (domain, services) in d.services.as_object().into_iter().flatten() {
        let category = vec![domain_title(t, domain)];
        for (svc, spec) in services.as_object().into_iter().flatten() {
            let prefix = format!("component.{domain}.services.{svc}");
            let has_target = spec.get("target").is_some();
            let response = spec.get("response").is_some();
            let optional_response =
                spec.pointer("/response/optional").and_then(Value::as_bool).unwrap_or(true);
            let title = tr(t, &format!("{prefix}.name"))
                .or_else(|| spec.get("name").and_then(Value::as_str))
                .map(str::to_owned)
                .unwrap_or_else(|| humanize(svc));
            let params = params_from(
                spec.get("fields").unwrap_or(&Value::Null),
                t,
                d.config_entries,
                &prefix,
                domain,
                "action",
                if has_target { &["entity_id"] } else { &[] },
            );
            let tooltip = tr(t, &format!("{prefix}.description"))
                .or_else(|| spec.get("description").and_then(Value::as_str))
                .unwrap_or("")
                .to_owned();
            let target = spec.get("target").map(target_spec);
            if optional_response {
                out.push(BlockDef {
                    block_type: format!("ha_svc__{}__{}", sanitize(domain), sanitize(svc)),
                    kind: BlockKind::Action,
                    category: category.clone(),
                    style: "action".into(),
                    title: title.clone(),
                    tooltip: format!("{tooltip}\n\n{domain}.{svc}"),
                    help_url: None,
                    target: target.clone(),
                    params: params.clone(),
                    lowering: Lowering::Service { service: format!("{domain}.{svc}"), response: false },
                    source: "ha".into(),
                });
            }
            if response {
                out.push(BlockDef {
                    block_type: format!("ha_svcr__{}__{}", sanitize(domain), sanitize(svc)),
                    kind: BlockKind::Value,
                    category: category.clone(),
                    style: "action".into(),
                    title: format!("{title} → result"),
                    tooltip: format!("{tooltip}\n\nReturns the action's response. {domain}.{svc}"),
                    help_url: None,
                    target,
                    params,
                    lowering: Lowering::Service { service: format!("{domain}.{svc}"), response: true },
                    source: "ha".into(),
                });
            }
        }
    }

    for (key, spec) in d.triggers.as_object().into_iter().flatten() {
        let (domain, name) = split_key(key);
        let prefix = format!("component.{domain}.triggers.{name}");
        out.push(BlockDef {
            block_type: format!("ha_trg__{}__{}", sanitize(domain), sanitize(name)),
            kind: BlockKind::Trigger,
            category: vec![domain_title(t, domain)],
            style: "trigger".into(),
            title: tr(t, &format!("{prefix}.name")).map(str::to_owned).unwrap_or_else(|| humanize(name)),
            tooltip: tr(t, &format!("{prefix}.description")).unwrap_or("").to_owned(),
            help_url: None,
            target: spec.get("target").map(|tg| TargetSpec { required: true, ..target_spec(tg) }),
            params: params_from(
                spec.get("fields").unwrap_or(&Value::Null),
                t,
                d.config_entries,
                &prefix,
                domain,
                "trigger",
                &[],
            ),
            lowering: Lowering::HaTriggerPlatform { trigger: key.clone() },
            source: "ha".into(),
        });
    }

    for (key, spec) in d.conditions.as_object().into_iter().flatten() {
        let (domain, name) = split_key(key);
        let prefix = format!("component.{domain}.conditions.{name}");
        out.push(BlockDef {
            block_type: format!("ha_cnd__{}__{}", sanitize(domain), sanitize(name)),
            kind: BlockKind::Condition,
            category: vec![domain_title(t, domain)],
            style: "condition".into(),
            title: tr(t, &format!("{prefix}.name")).map(str::to_owned).unwrap_or_else(|| humanize(name)),
            tooltip: tr(t, &format!("{prefix}.description")).unwrap_or("").to_owned(),
            help_url: None,
            target: spec.get("target").map(|tg| TargetSpec { required: true, ..target_spec(tg) }),
            params: params_from(
                spec.get("fields").unwrap_or(&Value::Null),
                t,
                d.config_entries,
                &prefix,
                domain,
                "condition",
                &[],
            ),
            lowering: Lowering::HaConditionPlatform { condition: key.clone() },
            source: "ha".into(),
        });
    }

    out
}
