//! Values the way Home Assistant templates (Python and Jinja) treat them, for blocks that
//! must give exactly the same result: `float()`/`int()`, `| round`, printing and comparisons
//! where text is never read as a number.

use std::cmp::Ordering;

use chrono::format::{Item, StrftimeItems};
use serde_json::{Value, json};

use crate::value::{VResult, num, parse_time, to_text};

/// `builtin` for the names this module owns.
pub fn builtin(name: &str, args: &[Value]) -> Option<VResult> {
    let a = |i: usize| args.get(i).unwrap_or(&Value::Null);
    Some(match name {
        "py_float" | "py_int" => {
            let parsed = if name == "py_int" { py_int(a(0)) } else { py_float(a(0)) };
            match (parsed, args.get(1)) {
                (Some(f), _) => Ok(num(f)),
                (None, Some(default)) => Ok(default.clone()),
                (None, None) => Err(format!("{} isn't a number", describe(a(0)))),
            }
        }
        "py_round" => py_round(a(0), a(1), &to_text(a(2))),
        "py_text" => Ok(json!(py_str(a(0), to_text(a(1)) == "float"))),
        "py_compare" => py_compare(a(0), a(1), &to_text(a(2))),
        "is_null" => Ok(json!(a(0).is_null())),
        "time_format" => time_format(a(0), &to_text(a(1))),
        _ => return None,
    })
}

fn describe(v: &Value) -> String {
    match v {
        Value::Null => "nothing".into(),
        Value::String(s) => format!("\"{s}\""),
        other => other.to_string(),
    }
}

/// Python's `str.strip()` whitespace.
fn py_space(c: char) -> bool {
    c.is_whitespace() || ('\x1c'..='\x1f').contains(&c)
}

/// Python allows single underscores between digits ("1_000"); they are dropped.
fn without_underscores(s: &str) -> Option<String> {
    let b = s.as_bytes();
    for (i, c) in b.iter().enumerate() {
        if *c == b'_' && !(i > 0 && b[i - 1].is_ascii_digit() && b.get(i + 1).is_some_and(u8::is_ascii_digit))
        {
            return None;
        }
    }
    Some(s.replace('_', ""))
}

/// Python's `float(text)`. Non-ASCII digits, which Python also accepts, aren't.
fn parse_float(s: &str) -> Option<f64> {
    let t = without_underscores(s.trim_matches(py_space))?;
    // Rust's grammar matches Python's otherwise ("1.", ".5", "1e3", "inf", "nan").
    t.parse().ok()
}

/// Python's `int(text, 10)`.
fn parse_int(s: &str) -> Option<f64> {
    let t = without_underscores(s.trim_matches(py_space))?;
    let digits = t.strip_prefix(['+', '-']).unwrap_or(&t);
    if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    t.parse::<i64>().ok().map(|n| n as f64).or_else(|| t.parse().ok())
}

/// `float(v)` in a template, or `None` where it raises. Infinity and NaN, which Python
/// accepts, count as not a number because Puzzlebox values can't hold them.
pub fn py_float(v: &Value) -> Option<f64> {
    let f = match v {
        Value::Number(n) => n.as_f64(),
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        Value::String(s) => parse_float(s),
        _ => None,
    }?;
    f.is_finite().then_some(f)
}

/// `int(v)` in a template (Jinja's `do_int`): text in base 10, else through `float()`,
/// cutting off decimals toward zero.
pub fn py_int(v: &Value) -> Option<f64> {
    let f = match v {
        Value::String(s) => parse_int(s).or_else(|| parse_float(s)),
        Value::Number(n) if n.is_i64() || n.is_u64() => n.as_f64(),
        other => py_float(other),
    }?;
    f.is_finite().then(|| f.trunc())
}

/// Home Assistant's `round` filter (`precision`, `method`), which rounds halves to even.
fn py_round(v: &Value, precision: &Value, method: &str) -> VResult {
    let x = py_float(v).ok_or_else(|| format!("can't round {}: it isn't a number", describe(v)))?;
    let p = precision.as_f64().unwrap_or(0.0).clamp(0.0, 300.0) as usize;
    let scale: f64 = format!("1e{p}").parse().unwrap_or(1.0);
    let r = match method {
        "ceil" => (x * scale).ceil() / scale,
        "floor" => (x * scale).floor() / scale,
        "half" => (x * 2.0).round_ties_even() / 2.0,
        // Formatting rounds the exact binary value, like Python's round(x, n).
        _ if p > 0 => format!("{x:.p$}").parse().unwrap_or(x),
        _ => x.round_ties_even(),
    };
    // Precision 0 gives a whole number, cutting off what "half" leaves.
    Ok(num(if p == 0 { r.trunc() } else { r }))
}

/// Python's `repr()` of a float: the shortest text that reads back as the same number,
/// "20.0" for whole numbers and an exponent outside 1e-4 … 1e16.
pub fn float_repr(f: f64) -> String {
    if f.is_nan() {
        return "nan".into();
    }
    if f.is_infinite() {
        return if f > 0.0 { "inf" } else { "-inf" }.into();
    }
    let sci = format!("{f:e}");
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let (sign, mantissa) = match mantissa.strip_prefix('-') {
        Some(m) => ("-", m),
        None => ("", mantissa),
    };
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    if (-4..16).contains(&exp) {
        if exp < 0 {
            return format!("{sign}0.{}{digits}", "0".repeat((-exp - 1) as usize));
        }
        let point = exp as usize + 1;
        let (int, frac) = if digits.len() > point {
            (digits[..point].to_owned(), digits[point..].to_owned())
        } else {
            (format!("{digits}{}", "0".repeat(point - digits.len())), "0".into())
        };
        return format!("{sign}{int}.{frac}");
    }
    let sign_e = if exp < 0 { '-' } else { '+' };
    format!("{sign}{mantissa}e{sign_e}{:02}", exp.abs())
}

/// Characters Python's `str.isprintable()` rejects, for the ones that matter in practice.
fn unprintable(c: char) -> bool {
    c.is_control()
        || matches!(
            c as u32,
            0xa0 | 0xad
                | 0x600..=0x605
                | 0x61c
                | 0x6dd
                | 0x70f
                | 0x1680
                | 0x180e
                | 0x2000..=0x200f
                | 0x2028..=0x202f
                | 0x205f..=0x2064
                | 0x2066..=0x206f
                | 0x3000
                | 0xe000..=0xf8ff
                | 0xfeff
                | 0xfff9..=0xfffb
                | 0xf0000..
        )
}

fn str_repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut out = String::from(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if unprintable(c) => {
                let n = c as u32;
                out.push_str(&if n <= 0xff {
                    format!("\\x{n:02x}")
                } else if n <= 0xffff {
                    format!("\\u{n:04x}")
                } else {
                    format!("\\U{n:08x}")
                });
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

fn repr(v: &Value, float: bool) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(b) => if *b { "True" } else { "False" }.into(),
        Value::Number(n) if !float && (n.is_i64() || n.is_u64()) => n.to_string(),
        Value::Number(n) => float_repr(n.as_f64().unwrap_or(0.0)),
        Value::String(s) => str_repr(s),
        Value::Array(items) => {
            format!("[{}]", items.iter().map(|x| repr(x, float)).collect::<Vec<_>>().join(", "))
        }
        Value::Object(m) => format!(
            "{{{}}}",
            m.iter()
                .map(|(k, x)| format!("{}: {}", str_repr(k), repr(x, float)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Python's `str()`, which is how a template prints a value. JSON keeps no difference
/// between 20 and 20.0 once Puzzlebox has calculated with it, so `float` prints every
/// number as a float.
pub fn py_str(v: &Value, float: bool) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => repr(other, float),
    }
}

fn numeric(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        _ => None,
    }
}

/// Python's `==`: text equals only text, `True == 1`, lists and dictionaries item by item.
pub fn py_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) if x.is_i64() && y.is_i64() => x.as_i64() == y.as_i64(),
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| py_eq(p, q)),
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, p)| y.get(k).is_some_and(|q| py_eq(p, q)))
        }
        _ => match (numeric(a), numeric(b)) {
            (Some(x), Some(y)) => x == y,
            _ => a == b,
        },
    }
}

fn kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "nothing",
        Value::Bool(_) | Value::Number(_) => "a number",
        Value::String(_) => "text",
        Value::Array(_) => "a list",
        Value::Object(_) => "a dictionary",
    }
}

/// Python's `<`: numbers with numbers, text with text (letter by letter), lists item by item.
fn py_order(a: &Value, b: &Value) -> Result<Ordering, String> {
    match (a, b) {
        (Value::String(x), Value::String(y)) => Ok(x.cmp(y)),
        (Value::Array(x), Value::Array(y)) => {
            for (p, q) in x.iter().zip(y) {
                if !py_eq(p, q) {
                    return py_order(p, q);
                }
            }
            Ok(x.len().cmp(&y.len()))
        }
        _ => match (numeric(a), numeric(b)) {
            (Some(x), Some(y)) => Ok(x.partial_cmp(&y).unwrap_or(Ordering::Equal)),
            _ => Err(format!("can't compare {} with {} by size", kind(a), kind(b))),
        },
    }
}

fn py_compare(a: &Value, b: &Value, op: &str) -> VResult {
    if matches!(op, "EQ" | "NEQ") {
        return Ok(json!(py_eq(a, b) == (op == "EQ")));
    }
    let o = py_order(a, b)?;
    Ok(json!(match op {
        "LT" => o.is_lt(),
        "LTE" => o.is_le(),
        "GT" => o.is_gt(),
        _ => o.is_ge(),
    }))
}

fn time_format(t: &Value, pattern: &str) -> VResult {
    let t = parse_time(t).ok_or_else(|| format!("not a date/time: {}", describe(t)))?;
    let items: Vec<Item> = StrftimeItems::new(pattern).collect();
    if items.iter().any(|i| matches!(i, Item::Error)) {
        return Err(format!("'{pattern}' isn't a valid time format"));
    }
    Ok(json!(t.format_with_items(items.into_iter()).to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(name: &str, args: &[Value]) -> VResult {
        builtin(name, args).unwrap()
    }

    #[test]
    fn float_and_int_like_python() {
        let f = |v: Value| call("py_float", &[v]);
        let i = |v: Value| call("py_int", &[v]);
        for (v, want) in [
            (json!("5"), json!(5)),
            (json!(" 7 "), json!(7)),
            (json!("1e3"), json!(1000)),
            (json!("1_000"), json!(1000)),
            (json!(".5"), json!(0.5)),
            (json!("1."), json!(1)),
            (json!("-2.5"), json!(-2.5)),
            (json!(true), json!(1)),
            (json!(2.5), json!(2.5)),
        ] {
            assert_eq!(f(v.clone()).unwrap(), want, "float({v})");
        }
        for (v, want) in [
            (json!("5"), json!(5)),
            (json!("+5"), json!(5)),
            (json!("007"), json!(7)),
            (json!("2.5"), json!(2)),
            (json!("-2.7"), json!(-2)),
            (json!("1e3"), json!(1000)),
            (json!("1_000"), json!(1000)),
            (json!(-2.7), json!(-2)),
            (json!(false), json!(0)),
        ] {
            assert_eq!(i(v.clone()).unwrap(), want, "int({v})");
        }
        for bad in [
            json!("abc"),
            json!("unknown"),
            json!(""),
            json!("1__0"),
            json!("_1"),
            json!("1_"),
            json!("0x10"),
            json!(null),
            json!([1]),
            json!("nan"),
            json!("inf"),
            json!("1e400"),
        ] {
            assert!(f(bad.clone()).is_err(), "float({bad})");
            assert!(i(bad.clone()).is_err(), "int({bad})");
        }
        assert_eq!(call("py_float", &[json!("abc"), json!(-1)]).unwrap(), json!(-1));
        assert_eq!(call("py_int", &[json!(null), json!(0.5)]).unwrap(), json!(0.5));
    }

    #[test]
    fn rounding_like_home_assistant() {
        let r = |x: Value, p: i64, m: &str| call("py_round", &[x, json!(p), json!(m)]).unwrap();
        assert_eq!(r(json!(2.5), 0, "common"), json!(2));
        assert_eq!(r(json!(3.5), 0, "common"), json!(4));
        assert_eq!(r(json!(-2.5), 0, "common"), json!(-2));
        assert_eq!(r(json!(2.675), 2, "common"), json!(2.67));
        assert_eq!(r(json!(0.25), 1, "common"), json!(0.2));
        assert_eq!(r(json!(21.55), 1, "common"), json!(21.6));
        assert_eq!(r(json!("2.5"), 0, "common"), json!(2));
        assert_eq!(r(json!(2.31), 1, "ceil"), json!(2.4));
        assert_eq!(r(json!(-2.5), 0, "floor"), json!(-3));
        assert_eq!(r(json!(2.3), 1, "half"), json!(2.5));
        assert_eq!(r(json!(2.3), 0, "half"), json!(2));
        assert_eq!(r(json!(2.8), 0, "other"), json!(3));
        assert!(call("py_round", &[json!("x"), json!(0), json!("common")]).is_err());
    }

    #[test]
    fn prints_like_python() {
        let s = |v: Value, float: bool| py_str(&v, float);
        assert_eq!(s(json!(true), false), "True");
        assert_eq!(s(json!(null), false), "None");
        assert_eq!(s(json!(20), false), "20");
        assert_eq!(s(json!(20), true), "20.0");
        assert_eq!(s(json!(20.0), false), "20.0");
        assert_eq!(s(json!("it's"), false), "it's");
        assert_eq!(
            s(json!([1, "a", null, 2.5, "it's", "\n"]), false),
            "[1, 'a', None, 2.5, \"it's\", '\\n']"
        );
        assert_eq!(s(json!({"a": 1, "b": [true]}), false), "{'a': 1, 'b': [True]}");
        for (f, want) in [
            (1e20, "1e+20"),
            (1e16, "1e+16"),
            (1e15, "1000000000000000.0"),
            (0.0001, "0.0001"),
            (0.00001, "1e-05"),
            (1.0 / 3.0, "0.3333333333333333"),
            (123456789012345678.0, "1.2345678901234568e+17"),
            (-0.0, "-0.0"),
            (-21.5, "-21.5"),
            (1.5e-7, "1.5e-07"),
        ] {
            assert_eq!(float_repr(f), want, "{f}");
        }
        assert_eq!(str_repr("a\u{a0}b"), "'a\\xa0b'");
    }

    #[test]
    fn compares_like_python() {
        let c = |a: Value, b: Value, op: &str| call("py_compare", &[a, b, json!(op)]);
        assert_eq!(c(json!("5"), json!(5), "EQ").unwrap(), json!(false));
        assert_eq!(c(json!("5"), json!("5"), "EQ").unwrap(), json!(true));
        assert_eq!(c(json!(5), json!(5.0), "EQ").unwrap(), json!(true));
        assert_eq!(c(json!(true), json!(1), "EQ").unwrap(), json!(true));
        assert_eq!(c(json!(null), json!(0), "EQ").unwrap(), json!(false));
        assert_eq!(c(json!(null), json!(null), "NEQ").unwrap(), json!(false));
        assert_eq!(c(json!("10"), json!("9"), "LT").unwrap(), json!(true));
        assert_eq!(c(json!([1, 2]), json!([1, 3]), "LT").unwrap(), json!(true));
        assert_eq!(c(json!({"a": 1}), json!({"a": 1.0}), "EQ").unwrap(), json!(true));
        assert!(c(json!("5"), json!(3), "GT").is_err());
        assert!(c(json!(null), json!(3), "LT").is_err());
        assert_eq!(call("is_null", &[json!(0)]).unwrap(), json!(false));
        assert_eq!(call("is_null", &[json!(null)]).unwrap(), json!(true));
    }

    #[test]
    fn formats_times() {
        let t = json!("2026-11-26T07:05:09+00:00");
        let local = parse_time(&t).unwrap();
        assert_eq!(
            call("time_format", &[t.clone(), json!("%H:%M")]).unwrap(),
            json!(local.format("%H:%M").to_string())
        );
        assert_eq!(
            call("time_format", &[t.clone(), json!("%Y-%m-%d %%")]).unwrap(),
            json!(local.format("%Y-%m-%d %%").to_string())
        );
        assert!(call("time_format", &[t, json!("%Q")]).is_err());
        assert!(call("time_format", &[json!("soon"), json!("%H")]).is_err());
    }
}
