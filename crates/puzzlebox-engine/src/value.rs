//! Runtime value semantics. Values are plain JSON so they flow to and from HA untouched;
//! HA entity states are strings, so numeric ops coerce numeric strings.

use std::cmp::Ordering;

use chrono::{DateTime, Datelike, FixedOffset, Timelike};

use crate::clock;
use puzzlebox_ir::BinOp;
use rand::Rng;
use serde_json::{Map, Value, json};

pub type VResult = Result<Value, String>;

pub fn num(f: f64) -> Value {
    if f.is_finite() && f.fract() == 0.0 && f.abs() < 9.0e15 {
        json!(f as i64)
    } else {
        serde_json::Number::from_f64(f).map(Value::Number).unwrap_or(Value::Null)
    }
}

pub fn as_num(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        Value::Null => Some(0.0),
        _ => None,
    }
}

pub fn to_num(v: &Value) -> Result<f64, String> {
    as_num(v).ok_or_else(|| match v.as_str().map(str::trim) {
        Some("unknown" | "unavailable" | "") => format!(
            "expected a number, got {}: the sensor has no value right now. Use \"… as a number (or 0 if unknown)\" or check \"has a value\" first",
            short(v)
        ),
        _ => format!("expected a number, got {}", short(v)),
    })
}

pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

pub fn to_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn short(v: &Value) -> String {
    let s = v.to_string();
    if s.len() > 60 { format!("{}…", &s[..s.floor_char_boundary(60)]) } else { s }
}

/// Numbers compare numerically, also when both are HA state strings like "9.5" and "10.2".
fn loose_cmp(a: &Value, b: &Value) -> Option<Ordering> {
    let numeric_side = matches!(a, Value::Number(_)) || matches!(b, Value::Number(_));
    let numeric_text = |v: &Value| v.as_str().is_some_and(|s| s.trim().parse::<f64>().is_ok());
    if (numeric_side || (numeric_text(a) && numeric_text(b)))
        && let (Some(x), Some(y)) = (as_num(a), as_num(b))
    {
        return x.partial_cmp(&y);
    }
    match (a, b) {
        (Value::String(x), Value::String(y)) => Some(x.cmp(y)),
        (Value::Bool(x), Value::Bool(y)) => Some(x.cmp(y)),
        _ => (a == b).then_some(Ordering::Equal),
    }
}

pub fn loose_eq(a: &Value, b: &Value) -> bool {
    loose_cmp(a, b) == Some(Ordering::Equal)
}

pub fn binary(op: BinOp, a: &Value, b: &Value) -> VResult {
    use BinOp::*;
    Ok(match op {
        Add => num(to_num(a)? + to_num(b)?),
        Sub => num(to_num(a)? - to_num(b)?),
        Mul => num(to_num(a)? * to_num(b)?),
        Div => {
            let d = to_num(b)?;
            if d == 0.0 {
                return Err("division by zero".into());
            }
            num(to_num(a)? / d)
        }
        Pow => num(to_num(a)?.powf(to_num(b)?)),
        Mod => {
            let d = to_num(b)?;
            if d == 0.0 {
                return Err("modulo by zero".into());
            }
            num(to_num(a)?.rem_euclid(d))
        }
        Eq => json!(loose_eq(a, b)),
        Neq => json!(!loose_eq(a, b)),
        Lt => json!(loose_cmp(a, b) == Some(Ordering::Less)),
        Lte => json!(matches!(loose_cmp(a, b), Some(Ordering::Less | Ordering::Equal))),
        Gt => json!(loose_cmp(a, b) == Some(Ordering::Greater)),
        Gte => json!(matches!(loose_cmp(a, b), Some(Ordering::Greater | Ordering::Equal))),
        And | Or => unreachable!("short-circuit ops are handled by the interpreter"),
    })
}

fn list(v: &Value) -> Result<&Vec<Value>, String> {
    v.as_array().ok_or_else(|| format!("expected a list, got {}", short(v)))
}

fn nums(v: &Value) -> Result<Vec<f64>, String> {
    list(v)?.iter().map(to_num).collect()
}

/// Resolves a 0-based index where negatives count from the end.
fn resolve_index(len: usize, idx: f64) -> Result<usize, String> {
    let i = idx as i64;
    let real = if i < 0 { len as i64 + i } else { i };
    if real < 0 || real >= len as i64 {
        return Err(format!("index {} out of range for list of {len}", if i < 0 { i } else { i + 1 }));
    }
    Ok(real as usize)
}

pub fn list_get(l: &Value, idx: &Value) -> VResult {
    let items = list(l)?;
    Ok(items[resolve_index(items.len(), to_num(idx)?)?].clone())
}

/// "random" (Blockly's "random" position) or a 0-based / negative-from-end index.
fn list_position(len: usize, key: &Value) -> Result<usize, String> {
    if key.as_str() == Some("random") {
        if len == 0 {
            return Err("the list is empty".into());
        }
        return Ok(rand::random_range(0..len));
    }
    resolve_index(len, to_num(key)?)
}

pub fn set_item(container: &mut Value, key: &Value, value: Value) -> Result<(), String> {
    match container {
        Value::Array(items) => {
            let i = list_position(items.len(), key)?;
            items[i] = value;
        }
        Value::Object(map) => {
            map.insert(to_text(key), value);
        }
        Value::Null => {
            let mut map = Map::new();
            map.insert(to_text(key), value);
            *container = Value::Object(map);
        }
        other => return Err(format!("cannot set an item on {}", short(other))),
    }
    Ok(())
}

/// Numbers, and text that is a number, for sorting.
fn sort_num(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn deg(f: f64) -> f64 {
    f.to_radians()
}

pub fn builtin(name: &str, args: &[Value]) -> VResult {
    if let Some(r) = crate::colour::builtin(name, args) {
        return r;
    }
    if let Some(r) = crate::python::builtin(name, args) {
        return r;
    }
    let a = |i: usize| args.get(i).unwrap_or(&Value::Null);
    let n = |i: usize| to_num(a(i));
    Ok(match name {
        "neg" | "math_neg" => num(-n(0)?),
        "math_root" => num(n(0)?.sqrt()),
        "math_abs" => num(n(0)?.abs()),
        "math_ln" => num(n(0)?.ln()),
        "math_log10" => num(n(0)?.log10()),
        "math_exp" => num(n(0)?.exp()),
        "math_pow10" => num(10f64.powf(n(0)?)),
        "math_round" => num(n(0)?.round()),
        "math_roundup" => num(n(0)?.ceil()),
        "math_rounddown" => num(n(0)?.floor()),
        "math_sin" => num(deg(n(0)?).sin()),
        "math_cos" => num(deg(n(0)?).cos()),
        "math_tan" => num(deg(n(0)?).tan()),
        "math_asin" => num(n(0)?.asin().to_degrees()),
        "math_acos" => num(n(0)?.acos().to_degrees()),
        "math_atan" => num(n(0)?.atan().to_degrees()),
        "is_even" => json!(n(0)? % 2.0 == 0.0),
        "is_odd" => json!(n(0)?.rem_euclid(2.0) == 1.0),
        "is_whole" => json!(n(0)?.fract() == 0.0),
        "is_positive" => json!(n(0)? > 0.0),
        "is_negative" => json!(n(0)? < 0.0),
        "is_divisible_by" => {
            let d = n(1)?;
            json!(d != 0.0 && n(0)? % d == 0.0)
        }
        "is_prime" => {
            let x = n(0)?;
            let p = x.fract() == 0.0
                && x >= 2.0
                && (2..=(x.sqrt() as u64)).all(|d| !(x as u64).is_multiple_of(d));
            json!(p)
        }
        "constrain" => num(n(0)?.max(n(1)?).min(n(2)?)),
        "random_int" => {
            let (lo, hi) = (n(0)? as i64, n(1)? as i64);
            let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
            json!(rand::rng().random_range(lo..=hi))
        }
        "random_float" => num(rand::rng().random::<f64>()),
        "unique_id" => json!(format!("{:016x}", rand::rng().random::<u64>())),
        "list_sum" => num(nums(a(0))?.iter().sum()),
        "list_min" => num(nums(a(0))?.into_iter().fold(f64::INFINITY, f64::min)),
        "list_max" => num(nums(a(0))?.into_iter().fold(f64::NEG_INFINITY, f64::max)),
        "list_average" => {
            let v = nums(a(0))?;
            if v.is_empty() { Value::Null } else { num(v.iter().sum::<f64>() / v.len() as f64) }
        }
        "list_median" => {
            let mut v = nums(a(0))?;
            if v.is_empty() {
                Value::Null
            } else {
                v.sort_by(|x, y| x.total_cmp(y));
                let m = v.len() / 2;
                num(if v.len().is_multiple_of(2) { (v[m - 1] + v[m]) / 2.0 } else { v[m] })
            }
        }
        "list_std_dev" => {
            let v = nums(a(0))?;
            if v.is_empty() {
                Value::Null
            } else {
                let mean = v.iter().sum::<f64>() / v.len() as f64;
                num((v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / v.len() as f64).sqrt())
            }
        }
        "list_random" => {
            let l = list(a(0))?;
            if l.is_empty() { Value::Null } else { l[rand::rng().random_range(0..l.len())].clone() }
        }
        "list_mode" => {
            let l = list(a(0))?;
            let mut best: Vec<Value> = Vec::new();
            let mut best_n = 0;
            for x in l {
                let c = l.iter().filter(|y| *y == x).count();
                if c > best_n {
                    best_n = c;
                    best = vec![x.clone()];
                } else if c == best_n && !best.contains(x) {
                    best.push(x.clone());
                }
            }
            Value::Array(best)
        }
        "text_join" => json!(args.iter().map(to_text).collect::<String>()),
        "length" => json!(match a(0) {
            Value::String(s) => s.chars().count(),
            Value::Array(l) => l.len(),
            Value::Object(o) => o.len(),
            Value::Null => 0,
            other => to_text(other).chars().count(),
        }),
        "is_empty" => json!(!truthy(a(0)) && !matches!(a(0), Value::Number(_) | Value::Bool(_))),
        "text_uppercase" => json!(to_text(a(0)).to_uppercase()),
        "text_lowercase" => json!(to_text(a(0)).to_lowercase()),
        "text_titlecase" => json!(
            to_text(a(0))
                .split(' ')
                .map(|w| {
                    let mut c = w.chars();
                    c.next()
                        .map(|f| f.to_uppercase().chain(c.flat_map(char::to_lowercase)).collect())
                        .unwrap_or_default()
                })
                .collect::<Vec<String>>()
                .join(" ")
        ),
        "text_trim" | "text_both" => json!(to_text(a(0)).trim()),
        "text_left" => json!(to_text(a(0)).trim_start()),
        "text_right" => json!(to_text(a(0)).trim_end()),
        "index_of" | "last_index_of" => {
            let last = name == "last_index_of";
            let pos = match a(0) {
                Value::Array(l) => {
                    if last {
                        l.iter().rposition(|x| loose_eq(x, a(1)))
                    } else {
                        l.iter().position(|x| loose_eq(x, a(1)))
                    }
                }
                other => {
                    let (h, needle) = (to_text(other), to_text(a(1)));
                    let byte = if last { h.rfind(&needle) } else { h.find(&needle) };
                    byte.map(|b| h[..b].chars().count())
                }
            };
            json!(pos.map_or(0, |p| p + 1))
        }
        "contains" => json!(match a(0) {
            Value::Array(l) => l.iter().any(|x| loose_eq(x, a(1))),
            Value::Object(o) => o.contains_key(&to_text(a(1))),
            other => to_text(other).contains(&to_text(a(1))),
        }),
        "to_number" => match as_num(a(0)) {
            Some(f) => num(f),
            None => Value::Null,
        },
        "to_text" => json!(to_text(a(0))),
        "list_get" => list_get(a(0), a(1))?,
        "to_target" => to_target(a(0).clone())?,
        "list_repeat" => Value::Array(vec![a(0).clone(); n(1)?.max(0.0) as usize]),
        "list_reverse" => Value::Array(list(a(0))?.iter().rev().cloned().collect()),
        "list_sort" | "list_sort_desc" => {
            let mut l = list(a(0))?.clone();
            // Optional 2nd arg: "TEXT" or "IGNORE_CASE"; otherwise numbers (also numeric
            // text) come first in numeric order, then everything else as text.
            match args.get(1).map(to_text).as_deref() {
                Some("TEXT") => l.sort_by_key(to_text),
                Some("IGNORE_CASE") => l.sort_by_key(|v| to_text(v).to_lowercase()),
                _ => l.sort_by(|x, y| match (sort_num(x), sort_num(y)) {
                    (Some(p), Some(q)) => p.total_cmp(&q),
                    (Some(_), None) => Ordering::Less,
                    (None, Some(_)) => Ordering::Greater,
                    (None, None) => to_text(x).cmp(&to_text(y)),
                }),
            }
            if name == "list_sort_desc" {
                l.reverse();
            }
            Value::Array(l)
        }
        "list_insert" => {
            // Inserts before the item at the position; "LAST" (4th arg) appends.
            let mut l = list(a(0))?.clone();
            let at = if args.get(3).map(to_text).as_deref() == Some("LAST") {
                l.len()
            } else if a(1).as_str() == Some("random") {
                rand::random_range(0..=l.len())
            } else {
                let i = n(1)? as i64;
                (if i < 0 { l.len() as i64 + i } else { i }).clamp(0, l.len() as i64) as usize
            };
            l.insert(at, a(2).clone());
            Value::Array(l)
        }
        // "add/remove … to/from list …" and "list … contains …": an empty value is an empty
        // list, and items compare like "find" does ("5" equals 5).
        "list_append" => {
            let mut l = if a(0).is_null() { vec![] } else { list(a(0))?.clone() };
            l.push(a(1).clone());
            Value::Array(l)
        }
        "list_remove_value" => match a(0) {
            Value::Null => json!([]),
            v => Value::Array(list(v)?.iter().filter(|x| !loose_eq(x, a(1))).cloned().collect()),
        },
        "list_contains" => match a(0) {
            Value::Null => json!(false),
            v => json!(list(v)?.iter().any(|x| loose_eq(x, a(1)))),
        },
        "list_remove" => {
            let mut l = list(a(0))?.clone();
            let i = list_position(l.len(), a(1))?;
            l.remove(i);
            Value::Array(l)
        }
        "random_index" => json!("random"),
        "list_join" => json!(list(a(0))?.iter().map(to_text).collect::<Vec<_>>().join(&to_text(a(1)))),
        "text_split" => {
            let delim = to_text(a(1));
            let s = to_text(a(0));
            if delim.is_empty() {
                Value::Array(s.chars().map(|c| json!(c.to_string())).collect())
            } else {
                Value::Array(s.split(&delim).map(|p| json!(p)).collect())
            }
        }
        "dict_get" => match a(0) {
            Value::Object(o) => o.get(&to_text(a(1))).cloned().unwrap_or(Value::Null),
            Value::Array(_) => list_get(a(0), &num(to_num(a(1))? - 1.0))?,
            Value::Null => Value::Null,
            other => return Err(format!("expected a dictionary, got {}", short(other))),
        },
        "dict_keys" => match a(0) {
            Value::Object(o) => Value::Array(o.keys().map(|k| json!(k)).collect()),
            _ => json!([]),
        },
        "dict_values" => match a(0) {
            Value::Object(o) => Value::Array(o.values().cloned().collect()),
            _ => json!([]),
        },
        "dict_has" => json!(a(0).as_object().is_some_and(|o| o.contains_key(&to_text(a(1))))),
        "json_parse" => serde_json::from_str(&to_text(a(0))).map_err(|e| format!("invalid JSON: {e}"))?,
        "json_stringify" => json!(a(0).to_string()),
        "now" => json!(clock::now().to_rfc3339()),
        "time_of_day" => json!(clock::now().format("%H:%M:%S").to_string()),
        "is_on" => json!(match a(0) {
            Value::String(s) =>
                matches!(s.trim().to_lowercase().as_str(), "on" | "true" | "yes" | "1" | "home" | "open"),
            other => truthy(other),
        }),
        // input_datetime.set_datetime takes time, date or datetime depending on the helper.
        "datetime_data" => {
            let text = to_text(a(0));
            let t = text.trim();
            let key = if t.contains(':') && !t.contains('-') {
                "time"
            } else if t.contains('-') && !t.contains(':') {
                "date"
            } else {
                "datetime"
            };
            json!({ key: t })
        }
        "time_hour" => json!(clock::now().hour()),
        "time_minute" => json!(clock::now().minute()),
        "time_second" => json!(clock::now().second()),
        "time_weekday" => json!(clock::now().weekday().number_from_monday()),
        "time_day" => json!(clock::now().day()),
        "time_month" => json!(clock::now().month()),
        "time_year" => json!(clock::now().year()),
        "today_at" => json!(today_at(&to_text(a(0)))?.to_rfc3339()),
        "datetime_add" => {
            let t = parse_time(a(0)).ok_or_else(|| format!("not a date/time: {}", short(a(0))))?;
            let ms = (n(1)? * 1000.0) as i64;
            json!((t + chrono::Duration::milliseconds(ms)).to_rfc3339())
        }
        "seconds_between" => {
            let x = parse_time(a(0)).ok_or_else(|| format!("not a date/time: {}", short(a(0))))?;
            let y = parse_time(a(1)).ok_or_else(|| format!("not a date/time: {}", short(a(1))))?;
            num((y - x).num_milliseconds() as f64 / 1000.0)
        }
        "format_duration" => json!(format_duration(n(0)?, &to_text(a(1)))),
        // `wait` is HA's {"remaining", "completed", "trigger"} (no "trigger" after "wait until").
        "wait_timed_out" => json!(match a(0) {
            Value::Object(m) if m.contains_key("completed") => !truthy(&m["completed"]),
            Value::Object(m) => m.get("trigger").is_none_or(Value::is_null),
            _ => false,
        }),
        "wait_entity" => a(0).pointer("/trigger/entity_id").cloned().unwrap_or(Value::Null),
        "case_matches" => match a(1) {
            // A list case matches any of its values.
            Value::Array(options) => json!(options.iter().any(|o| loose_eq(a(0), o))),
            other => json!(loose_eq(a(0), other)),
        },
        "or_default" => {
            let v = a(0).clone();
            let empty = match &v {
                Value::Null => true,
                Value::String(s) => matches!(s.trim(), "" | "unknown" | "unavailable" | "none"),
                _ => false,
            };
            if empty { a(1).clone() } else { v }
        }
        "between" => {
            let (x, lo, hi) = (n(0)?, n(1)?, n(2)?);
            json!(x >= lo.min(hi) && x <= lo.max(hi))
        }
        "text_before" | "text_after" => {
            let (text, sep) = (to_text(a(0)), to_text(a(1)));
            match text.find(&sep) {
                Some(i) if !sep.is_empty() => {
                    json!(if name == "text_before" { &text[..i] } else { &text[i + sep.len()..] })
                }
                _ => json!(if name == "text_before" { text.as_str() } else { "" }),
            }
        }
        "regex_test" | "regex_match" | "regex_all" | "regex_replace" => {
            let re = regex::Regex::new(&to_text(a(1))).map_err(|e| format!("invalid pattern: {e}"))?;
            let text = to_text(a(0));
            match name {
                "regex_test" => json!(re.is_match(&text)),
                // The first group if the pattern has one, else the whole match.
                "regex_match" => re
                    .captures(&text)
                    .and_then(|c| c.get(1).or_else(|| c.get(0)).map(|m| m.as_str().to_owned()))
                    .map_or(Value::Null, |m| json!(m)),
                "regex_all" => json!(re.find_iter(&text).map(|m| m.as_str()).collect::<Vec<_>>()),
                _ => json!(re.replace_all(&text, to_text(a(2)).as_str())),
            }
        }
        "duration_dict" => {
            let total = n(0)?.max(0.0) as u64;
            json!({"hours": total / 3600, "minutes": (total / 60) % 60, "seconds": total % 60})
        }
        "time_timestamp" => num(clock::now().timestamp_millis() as f64 / 1000.0),
        _ => return Err(format!("unknown function '{name}'")),
    })
}

/// `"HH:MM"`/`"HH:MM:SS"` today in local time.
pub fn today_at(t: &str) -> Result<DateTime<FixedOffset>, String> {
    let parts: Vec<u32> = t
        .trim()
        .split(':')
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()
        .ok_or_else(|| format!("expected HH:MM, got '{t}'"))?;
    let (h, m, sec) = (
        parts.first().copied().unwrap_or(0),
        parts.get(1).copied().unwrap_or(0),
        parts.get(2).copied().unwrap_or(0),
    );
    let naive =
        clock::now().date_naive().and_hms_opt(h, m, sec).ok_or_else(|| format!("invalid time '{t}'"))?;
    clock::from_local(naive).ok_or_else(|| format!("'{t}' does not exist today"))
}

/// RFC 3339 / ISO date-time, or `HH:MM` meaning today.
pub fn parse_time(v: &Value) -> Option<DateTime<FixedOffset>> {
    let s = to_text(v);
    if let Ok(t) = chrono::DateTime::parse_from_rfc3339(s.trim()) {
        return Some(clock::local(t));
    }
    if let Some(f) = v.as_f64().filter(|_| v.is_number()) {
        return chrono::DateTime::from_timestamp_millis((f * 1000.0) as i64).map(clock::local);
    }
    // input_datetime states: "2026-11-26 07:30:00" or "2026-11-26" in local time.
    for fmt in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M"] {
        if let Ok(n) = chrono::NaiveDateTime::parse_from_str(s.trim(), fmt) {
            return clock::from_local(n);
        }
    }
    if let Ok(d) = chrono::NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d") {
        return clock::from_local(d.and_hms_opt(0, 0, 0)?);
    }
    today_at(&s).ok()
}

pub fn format_duration(secs: f64, style: &str) -> String {
    let total = secs.max(0.0).round() as u64;
    let (d, h, m, s) = (total / 86400, (total / 3600) % 24, (total / 60) % 60, total % 60);
    if style == "clock" {
        let hours = total / 3600;
        return format!("{hours}:{m:02}:{s:02}");
    }
    let mut parts = Vec::new();
    for (v, unit) in [(d, "d"), (h, "h"), (m, "min"), (s, "s")] {
        if v > 0 {
            parts.push(format!("{v} {unit}"));
        }
    }
    if parts.is_empty() { "0 s".into() } else { parts.into_iter().take(2).collect::<Vec<_>>().join(" ") }
}

/// An entity id, comma-separated ids, or a list of ids.
pub fn entity_list(v: &Value) -> Vec<String> {
    match v {
        Value::Array(a) => a.iter().map(to_text).filter(|s| !s.is_empty()).collect(),
        Value::Null => vec![],
        other => to_text(other).split(',').map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).collect(),
    }
}

fn usable(state: &str) -> bool {
    !matches!(state, "unknown" | "unavailable" | "")
}

/// Last-activation time of a scene (its state), if it has one.
fn scene_time(state: Option<Value>) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    let s = state?.get("state")?.as_str()?.to_owned();
    chrono::DateTime::parse_from_rfc3339(&s).ok()
}

/// Builtins that read Home Assistant's state cache through `lookup`.
pub fn ha_builtin(name: &str, a: &[Value], lookup: &dyn Fn(&str) -> Option<Value>) -> VResult {
    let arg = |i: usize| a.get(i).cloned().unwrap_or(Value::Null);
    let state_of =
        |e: &str| lookup(e).and_then(|s| s.get("state").and_then(Value::as_str).map(str::to_owned));
    Ok(match name {
        "ha:has_value" => json!(state_of(&to_text(&arg(0))).is_some_and(|s| usable(&s))),
        "ha:light_colour" => lookup(&to_text(&arg(0))).map_or(Value::Null, |s| crate::colour::of_state(&s)),
        "ha:light_brightness" => {
            lookup(&to_text(&arg(0))).map_or(json!(0), |s| crate::colour::brightness_pct(&s))
        }
        "ha:friendly_name" => {
            let e = to_text(&arg(0));
            lookup(&e)
                .and_then(|s| s.pointer("/attributes/friendly_name").cloned())
                .unwrap_or_else(|| json!(e))
        }
        "ha:seconds_since" => {
            let field = to_text(&arg(1));
            let field = match field.as_str() {
                "last_updated" => "last_updated",
                "last_reported" => "last_reported",
                _ => "last_changed",
            };
            match lookup(&to_text(&arg(0)))
                .and_then(|s| s.get(field).and_then(Value::as_str).map(str::to_owned))
                .and_then(|t| chrono::DateTime::parse_from_rfc3339(&t).ok())
            {
                Some(t) => num((chrono::Utc::now() - t.with_timezone(&chrono::Utc)).num_milliseconds()
                    as f64
                    / 1000.0),
                None => Value::Null,
            }
        }
        "ha:number_state" => {
            match state_of(&to_text(&arg(0))).and_then(|s| crate::python::py_float(&json!(s))) {
                Some(f) => num(f),
                None => arg(1),
            }
        }
        "ha:state_check" => {
            let ids = entity_list(&arg(0));
            let (mode, negate, want) = (to_text(&arg(1)), to_text(&arg(2)) == "is_not", to_text(&arg(3)));
            let hits = ids
                .iter()
                .filter(|e| state_of(e).is_some_and(|s| loose_eq(&json!(s), &json!(want)) != negate))
                .count();
            match mode.as_str() {
                "all" => json!(!ids.is_empty() && hits == ids.len()),
                "none" => json!(hits == 0),
                "count" => json!(hits),
                _ => json!(hits > 0),
            }
        }
        "ha:aggregate" => {
            let vals: Vec<f64> = entity_list(&arg(0))
                .iter()
                .filter_map(|e| state_of(e)?.trim().parse::<f64>().ok())
                .filter(|f| f.is_finite())
                .collect();
            if vals.is_empty() {
                return Ok(Value::Null);
            }
            match to_text(&arg(1)).as_str() {
                "min" => num(vals.iter().copied().fold(f64::INFINITY, f64::min)),
                "max" => num(vals.iter().copied().fold(f64::NEG_INFINITY, f64::max)),
                "sum" => num(vals.iter().sum()),
                "median" => {
                    let mut v = vals;
                    v.sort_by(f64::total_cmp);
                    let m = v.len() / 2;
                    num(if v.len().is_multiple_of(2) { (v[m - 1] + v[m]) / 2.0 } else { v[m] })
                }
                _ => num(vals.iter().sum::<f64>() / vals.len() as f64),
            }
        }
        "ha:last_scene" => last_scene(&entity_list(&arg(0)), lookup).map_or(Value::Null, |s| json!(s)),
        "ha:next_scene" => {
            let scenes = entity_list(&arg(0));
            let usable_scenes: Vec<&String> = scenes
                .iter()
                .filter(|s| {
                    lookup(s)
                        .and_then(|v| v.get("state").and_then(Value::as_str).map(|x| x != "unavailable"))
                        .unwrap_or(false)
                })
                .collect();
            if usable_scenes.is_empty() {
                return Err("none of the scenes is available".into());
            }
            let current = last_scene(&scenes, lookup);
            let pos = current.and_then(|c| usable_scenes.iter().position(|s| **s == c));
            let n = usable_scenes.len();
            let next = match (pos, to_text(&arg(1)).as_str()) {
                (None, _) => 0,
                (Some(i), "previous") => (i + n - 1) % n,
                (Some(i), _) => (i + 1) % n,
            };
            json!(usable_scenes[next])
        }
        _ => return Err(format!("unknown function '{name}'")),
    })
}

/// The scene among `scenes` that was activated most recently (ignores never-activated ones).
pub fn last_scene(scenes: &[String], lookup: &dyn Fn(&str) -> Option<Value>) -> Option<String> {
    scenes
        .iter()
        .filter_map(|s| scene_time(lookup(s)).map(|t| (t, s)))
        .max_by_key(|(t, _)| *t)
        .map(|(_, s)| s.clone())
}

/// Statistics over recorded states `(unix seconds, state)` within `[start, end]`.
pub fn history_stat(points: &[(f64, String)], start: f64, end: f64, op: &str, state: &str) -> VResult {
    let mut pts: Vec<(f64, &str)> = points.iter().map(|(t, s)| (t.max(start), s.as_str())).collect();
    pts.sort_by(|a, b| a.0.total_cmp(&b.0));
    // Segment i lasts from its time to the next point (or `end`).
    let seg = |i: usize| -> f64 { pts.get(i + 1).map_or(end, |p| p.0).min(end) - pts[i].0 };
    Ok(match op {
        "state_ago" => pts.first().map_or(Value::Null, |(_, s)| json!(s)),
        "changes" => json!(pts.windows(2).filter(|w| w[0].1 != w[1].1).count()),
        "time_in_state" => num((0..pts.len()).filter(|&i| pts[i].1 == state).map(seg).sum()),
        "min" | "max" | "average" => {
            let numeric: Vec<(usize, f64)> = pts
                .iter()
                .enumerate()
                .filter_map(|(i, (_, s))| s.trim().parse::<f64>().ok().map(|f| (i, f)))
                .collect();
            if numeric.is_empty() {
                return Ok(Value::Null);
            }
            match op {
                "min" => num(numeric.iter().map(|x| x.1).fold(f64::INFINITY, f64::min)),
                "max" => num(numeric.iter().map(|x| x.1).fold(f64::NEG_INFINITY, f64::max)),
                _ => {
                    // Time-weighted: each value counts for as long as it was the state.
                    let (sum, weight) = numeric.iter().fold((0.0, 0.0), |(s, w), (i, v)| {
                        let d = seg(*i).max(0.0);
                        (s + v * d, w + d)
                    });
                    if weight > 0.0 {
                        num((sum / weight * 1000.0).round() / 1000.0)
                    } else {
                        num(numeric[0].1)
                    }
                }
            }
        }
        other => return Err(format!("unknown history statistic '{other}'")),
    })
}

/// Accepts an entity id, comma-separated ids, a list of ids, or a full target dict.
pub fn to_target(v: Value) -> VResult {
    puzzlebox_ir::normalize_target(&v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_coercion_of_ha_states() {
        assert_eq!(binary(BinOp::Gt, &json!("21.5"), &json!(20)).unwrap(), json!(true));
        assert_eq!(binary(BinOp::Eq, &json!("on"), &json!("on")).unwrap(), json!(true));
        assert_eq!(binary(BinOp::Eq, &json!("5"), &json!(5)).unwrap(), json!(true));
        assert_eq!(binary(BinOp::Add, &json!(1), &json!(2)).unwrap(), json!(3));
        assert_eq!(binary(BinOp::Div, &json!(1), &json!(4)).unwrap(), json!(0.25));
        assert!(binary(BinOp::Add, &json!("on"), &json!(1)).is_err());
    }

    #[test]
    fn lists_and_text() {
        let l = json!([3, 1, 2]);
        assert_eq!(list_get(&l, &json!(0)).unwrap(), json!(3));
        assert_eq!(list_get(&l, &json!(-1)).unwrap(), json!(2));
        assert!(list_get(&l, &json!(3)).is_err());
        assert_eq!(builtin("list_sort", std::slice::from_ref(&l)).unwrap(), json!([1, 2, 3]));
        assert_eq!(builtin("list_median", &[l]).unwrap(), json!(2));
        assert_eq!(builtin("text_join", &[json!("a"), json!(1), Value::Null]).unwrap(), json!("a1"));
        assert_eq!(builtin("index_of", &[json!("héllo"), json!("l")]).unwrap(), json!(3));
        assert_eq!(builtin("text_titlecase", &[json!("hello wORLD")]).unwrap(), json!("Hello World"));
    }

    #[test]
    fn scenes_and_state_checks() {
        let states: std::collections::HashMap<&str, Value> = [
            ("scene.day", json!({"state": "2026-11-25T20:00:00+00:00"})),
            ("scene.sleepy", json!({"state": "2026-11-25T22:00:00+00:00"})),
            ("scene.new", json!({"state": "unknown"})),
            ("light.a", json!({"state": "on", "attributes": {"friendly_name": "Lamp A"}})),
            ("light.b", json!({"state": "off"})),
            ("sensor.t1", json!({"state": "20"})),
            ("sensor.t2", json!({"state": "unavailable"})),
            ("sensor.t3", json!({"state": "24"})),
        ]
        .into_iter()
        .collect();
        let lookup = |e: &str| states.get(e).cloned();
        let scenes = json!(["scene.day", "scene.sleepy", "scene.new"]);
        // `unknown` (never activated) no longer "wins" like it did with text comparison.
        assert_eq!(
            ha_builtin("ha:last_scene", std::slice::from_ref(&scenes), &lookup).unwrap(),
            json!("scene.sleepy")
        );
        assert_eq!(
            ha_builtin("ha:next_scene", &[scenes.clone(), json!("next")], &lookup).unwrap(),
            json!("scene.new")
        );
        assert_eq!(
            ha_builtin("ha:next_scene", &[json!("scene.day,scene.sleepy"), json!("next")], &lookup).unwrap(),
            json!("scene.day")
        );
        assert_eq!(
            ha_builtin(
                "ha:state_check",
                &[json!("light.a,light.b"), json!("count"), json!("is"), json!("on")],
                &lookup
            )
            .unwrap(),
            json!(1)
        );
        assert_eq!(
            ha_builtin(
                "ha:state_check",
                &[json!(["light.a", "light.b"]), json!("all"), json!("is_not"), json!("unavailable")],
                &lookup
            )
            .unwrap(),
            json!(true)
        );
        assert_eq!(
            ha_builtin("ha:aggregate", &[json!("sensor.t1,sensor.t2,sensor.t3"), json!("average")], &lookup)
                .unwrap(),
            json!(22)
        );
        assert_eq!(ha_builtin("ha:has_value", &[json!("sensor.t2")], &lookup).unwrap(), json!(false));
        assert_eq!(
            ha_builtin("ha:number_state", &[json!("sensor.t2"), json!(-1)], &lookup).unwrap(),
            json!(-1)
        );
        assert_eq!(ha_builtin("ha:friendly_name", &[json!("light.a")], &lookup).unwrap(), json!("Lamp A"));
    }

    #[test]
    fn history_statistics() {
        // 0..10 s at 10, 10..30 s at 20, 30..40 s "off".
        let pts = vec![(-5.0, "10".to_owned()), (10.0, "20".to_owned()), (30.0, "off".to_owned())];
        let (start, end) = (0.0, 40.0);
        assert_eq!(history_stat(&pts, start, end, "state_ago", "").unwrap(), json!("10"));
        assert_eq!(history_stat(&pts, start, end, "average", "").unwrap(), json!(16.667));
        assert_eq!(history_stat(&pts, start, end, "max", "").unwrap(), json!(20));
        assert_eq!(history_stat(&pts, start, end, "changes", "").unwrap(), json!(2));
        assert_eq!(history_stat(&pts, start, end, "time_in_state", "off").unwrap(), json!(10));
    }

    #[test]
    fn durations_and_times() {
        assert_eq!(format_duration(3725.0, "words"), "1 h 2 min");
        assert_eq!(format_duration(3725.0, "clock"), "1:02:05");
        let a = json!("2026-11-25T10:00:00+00:00");
        let b = builtin("datetime_add", &[a.clone(), json!(90)]).unwrap();
        assert_eq!(builtin("seconds_between", &[a, b]).unwrap(), json!(90));
    }

    #[test]
    fn set_item_creates_dicts() {
        let mut v = Value::Null;
        set_item(&mut v, &json!("a"), json!(1)).unwrap();
        assert_eq!(v, json!({"a": 1}));
        let mut l = json!([1, 2]);
        set_item(&mut l, &json!(-1), json!(9)).unwrap();
        assert_eq!(l, json!([1, 9]));
    }

    #[test]
    fn review_fixes_lists_and_colours() {
        // Mixed lists sort without panicking: numbers first, then text.
        let l = json!(["kitchen", "10", 3, "bath", 2.5]);
        assert_eq!(
            builtin("list_sort", &[l, json!("NUMERIC")]).unwrap(),
            json!([2.5, 3, "10", "bath", "kitchen"])
        );
        // Insert before the last item ("#1 from end"), append for "last", random works.
        let abc = json!(["a", "b", "c"]);
        assert_eq!(
            builtin("list_insert", &[abc.clone(), json!(-1), json!("X"), json!("FROM_END")]).unwrap(),
            json!(["a", "b", "X", "c"])
        );
        assert_eq!(
            builtin("list_insert", &[abc.clone(), json!(-1), json!("X"), json!("LAST")]).unwrap(),
            json!(["a", "b", "c", "X"])
        );
        assert_eq!(
            builtin("list_remove", &[abc.clone(), json!("random")]).unwrap().as_array().unwrap().len(),
            2
        );
        let mut m = abc;
        set_item(&mut m, &json!("random"), json!("Z")).unwrap();
        assert!(m.as_array().unwrap().contains(&json!("Z")));
        // Text that isn't ASCII is simply not a colour.
        for bad in ["€", "aé€", "ééé"] {
            assert_eq!(crate::colour::parse(&json!(bad)), None);
        }
    }
}
