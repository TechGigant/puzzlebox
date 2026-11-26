//! Home Assistant's YAML tags (`!secret`, `!include…`, `!env_var`, `!input`).
//!
//! We can't read the user's other files, secrets or environment, so a tagged value is
//! kept as a marker while documents are discovered (an `automation: !include …` key is
//! reported, a blueprint's `!input` is substituted) and replaced by a placeholder at the end.

use serde_json::{Number, Value, json};

/// The marker object's only key; NUL can't appear in a YAML key a user writes.
const MARKER: &str = "\u{0}tag";

pub(super) fn to_json(v: serde_yaml::Value) -> Value {
    match v {
        serde_yaml::Value::Null => Value::Null,
        serde_yaml::Value::Bool(b) => Value::Bool(b),
        serde_yaml::Value::Number(n) => number(&n),
        serde_yaml::Value::String(s) => Value::String(s),
        serde_yaml::Value::Sequence(items) => Value::Array(items.into_iter().map(to_json).collect()),
        serde_yaml::Value::Mapping(m) => {
            Value::Object(m.into_iter().map(|(k, v)| (key(k), to_json(v))).collect())
        }
        serde_yaml::Value::Tagged(t) => {
            let tag = t.tag.to_string();
            let inner = to_json(t.value);
            // Core schema tags (`!!str 5`) only change the type; keep the value as written.
            match tag.as_str() {
                "!!str" | "tag:yaml.org,2002:str" => Value::String(scalar_text(&inner)),
                t if t.starts_with("!!") || t.starts_with("tag:yaml.org,2002:") => inner,
                _ => json!({MARKER: [tag, inner]}),
            }
        }
    }
}

fn number(n: &serde_yaml::Number) -> Value {
    if let Some(i) = n.as_i64() {
        Value::from(i)
    } else if let Some(u) = n.as_u64() {
        Value::from(u)
    } else {
        n.as_f64().and_then(Number::from_f64).map_or_else(|| Value::String(n.to_string()), Value::Number)
    }
}

fn key(k: serde_yaml::Value) -> String {
    match to_json(k) {
        Value::String(s) => s,
        v => tag(&v).map_or_else(|| scalar_text(&v), |(t, inner)| describe(t, inner)),
    }
}

fn scalar_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "null".into(),
        other => crate::yaml(other),
    }
}

/// `(tag, value)` when `v` is a tag marker.
pub(super) fn tag(v: &Value) -> Option<(&str, &Value)> {
    let m = v.as_object()?;
    if m.len() != 1 {
        return None;
    }
    let pair = m.get(MARKER)?.as_array()?;
    Some((pair.first()?.as_str()?, pair.get(1)?))
}

/// The tag as the user wrote it, e.g. `!secret api_key`.
pub(super) fn describe(tag: &str, inner: &Value) -> String {
    match inner {
        Value::Null => tag.to_owned(),
        other => format!("{tag} {}", scalar_text(other)),
    }
}

pub(super) fn is_include(tag: &str) -> bool {
    tag.starts_with("!include")
}

/// The name of a blueprint `!input` marker.
pub(super) fn input_name(v: &Value) -> Option<&str> {
    tag(v).filter(|(t, _)| *t == "!input").and_then(|(_, name)| name.as_str())
}

/// Replaces every remaining marker with a placeholder value and explains each in `warnings`.
///
/// Secrets, includes and inputs without a value become the tag's own text (`"!secret api_key"`):
/// it is obvious in the editor, can't be mistaken for a real value, and when saved it reaches
/// Home Assistant as a quoted string, so nothing is resolved behind the user's back.
pub(super) fn finish(v: &mut Value, warnings: &mut Vec<String>) {
    if let Some((t, inner)) = tag(v) {
        let text = describe(t, inner);
        let (replacement, warning) = placeholder(t, inner, &text);
        *v = replacement;
        if let Some(w) = warning {
            push_unique(warnings, w);
        }
        // An unknown tag on a list or map keeps the content, which may hold more tags.
        finish_children(v, warnings);
        return;
    }
    finish_children(v, warnings);
}

fn finish_children(v: &mut Value, warnings: &mut Vec<String>) {
    match v {
        Value::Array(items) => items.iter_mut().for_each(|i| finish(i, warnings)),
        Value::Object(m) => m.values_mut().for_each(|i| finish(i, warnings)),
        _ => {}
    }
}

fn placeholder(tag: &str, inner: &Value, text: &str) -> (Value, Option<String>) {
    let kept = || Value::String(text.to_owned());
    match tag {
        "!secret" => (
            kept(),
            Some(format!(
                "`{text}`: secrets.yaml can't be read here, so the value is the plain text \"{text}\". \
                 Replace it before saving."
            )),
        ),
        "!env_var" => {
            let raw = scalar_text(inner);
            let mut args = raw.split_whitespace();
            let name = args.next().unwrap_or_default();
            let default: Vec<&str> = args.collect();
            if default.is_empty() {
                (
                    kept(),
                    Some(format!(
                        "`{text}`: environment variable {name} can't be read here, so the value is the plain \
                         text \"{text}\". Replace it before saving."
                    )),
                )
            } else {
                let default = default.join(" ");
                let w = format!(
                    "`{text}`: environment variable {name} can't be read here; used its default \"{default}\"."
                );
                (Value::String(default), Some(w))
            }
        }
        "!input" => (
            kept(),
            Some(format!(
                "`{text}`: blueprint input {} has no value, so it is the plain text \"{text}\". \
                 Replace it before saving.",
                scalar_text(inner)
            )),
        ),
        t if is_include(t) => (
            kept(),
            Some(format!(
                "`{text}` points to another file, which can't be read here; the value is the plain text \
                 \"{text}\". Paste that file's content in its place, or replace it."
            )),
        ),
        _ => (inner.clone(), Some(format!("Unknown YAML tag `{tag}` was ignored; its value was kept."))),
    }
}

pub(super) fn push_unique(list: &mut Vec<String>, msg: String) {
    if !list.contains(&msg) {
        list.push(msg);
    }
}
