//! Runtime value semantics. Values are plain JSON so they flow to and from HA untouched;
//! HA entity states are strings, so numeric ops coerce numeric strings.

use std::cmp::Ordering;

use chrono::{Datelike, Local, Timelike};
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
    as_num(v).ok_or_else(|| format!("expected a number, got {}", short(v)))
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

/// Numbers compare numerically even when one side is an HA state string like "21.5".
fn loose_cmp(a: &Value, b: &Value) -> Option<Ordering> {
    let numeric_side = matches!(a, Value::Number(_)) || matches!(b, Value::Number(_));
    if numeric_side && let (Some(x), Some(y)) = (as_num(a), as_num(b)) {
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

pub fn set_item(container: &mut Value, key: &Value, value: Value) -> Result<(), String> {
    match container {
        Value::Array(items) => {
            let i = resolve_index(items.len(), to_num(key)?)?;
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

fn deg(f: f64) -> f64 {
    f.to_radians()
}

pub fn builtin(name: &str, args: &[Value]) -> VResult {
    let a = |i: usize| args.get(i).unwrap_or(&Value::Null);
    let n = |i: usize| to_num(a(i));
    Ok(match name {
        "neg" => num(-n(0)?),
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
                num(if v.len() % 2 == 0 { (v[m - 1] + v[m]) / 2.0 } else { v[m] })
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
            l.sort_by(|x, y| loose_cmp(x, y).unwrap_or(Ordering::Equal));
            if name == "list_sort_desc" {
                l.reverse();
            }
            Value::Array(l)
        }
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
        "now" => json!(Local::now().to_rfc3339()),
        "time_hour" => json!(Local::now().hour()),
        "time_minute" => json!(Local::now().minute()),
        "time_second" => json!(Local::now().second()),
        "time_weekday" => json!(Local::now().weekday().number_from_monday()),
        "time_day" => json!(Local::now().day()),
        "time_month" => json!(Local::now().month()),
        "time_year" => json!(Local::now().year()),
        "time_timestamp" => num(Local::now().timestamp_millis() as f64 / 1000.0),
        _ => return Err(format!("unknown function '{name}'")),
    })
}

/// Accepts an entity id, comma-separated ids, a list of ids, or a full target dict.
pub fn to_target(v: Value) -> VResult {
    Ok(match v {
        Value::String(s) => {
            let ids: Vec<&str> = s.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
            json!({"entity_id": ids})
        }
        Value::Array(items) => {
            let ids: Vec<String> = items.iter().map(to_text).collect();
            json!({"entity_id": ids})
        }
        v @ Value::Object(_) => v,
        Value::Null => json!({}),
        other => return Err(format!("invalid target {other}")),
    })
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
    fn set_item_creates_dicts() {
        let mut v = Value::Null;
        set_item(&mut v, &json!("a"), json!(1)).unwrap();
        assert_eq!(v, json!({"a": 1}));
        let mut l = json!([1, 2]);
        set_item(&mut l, &json!(-1), json!(9)).unwrap();
        assert_eq!(l, json!([1, 9]));
    }
}
