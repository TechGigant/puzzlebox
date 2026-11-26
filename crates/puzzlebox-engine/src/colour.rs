//! Colours as "#rrggbb" text, the form colour blocks produce, and conversions to what
//! Home Assistant's light actions take.

use serde_json::{Map, Value, json};

/// A few common colour names; any other name goes to HA as `color_name`.
const NAMES: &[(&str, [u8; 3])] = &[
    ("white", [255, 255, 255]),
    ("black", [0, 0, 0]),
    ("red", [255, 0, 0]),
    ("green", [0, 128, 0]),
    ("lime", [0, 255, 0]),
    ("blue", [0, 0, 255]),
    ("yellow", [255, 255, 0]),
    ("orange", [255, 165, 0]),
    ("purple", [128, 0, 128]),
    ("pink", [255, 192, 203]),
    ("cyan", [0, 255, 255]),
    ("magenta", [255, 0, 255]),
    ("warmwhite", [255, 214, 170]),
];

fn byte(v: &Value) -> Option<u8> {
    v.as_f64().map(|n| n.round().clamp(0.0, 255.0) as u8)
}

/// Accepts "#rrggbb", "#rgb", "rrggbb", [r, g, b], {"r", "g", "b"} and common names.
pub fn parse(v: &Value) -> Option<[u8; 3]> {
    match v {
        Value::String(s) => {
            let s = s.trim();
            let lower = s.to_lowercase().replace([' ', '_'], "");
            if let Some((_, rgb)) = NAMES.iter().find(|(n, _)| *n == lower) {
                return Some(*rgb);
            }
            let hex = s.trim_start_matches('#');
            let full: String = match hex.len() {
                3 => hex.chars().flat_map(|c| [c, c]).collect(),
                6 => hex.to_owned(),
                _ => return None,
            };
            let p = |i: usize| u8::from_str_radix(&full[i..i + 2], 16).ok();
            Some([p(0)?, p(2)?, p(4)?])
        }
        Value::Array(a) if a.len() == 3 => Some([byte(&a[0])?, byte(&a[1])?, byte(&a[2])?]),
        Value::Object(m) => Some([byte(m.get("r")?)?, byte(m.get("g")?)?, byte(m.get("b")?)?]),
        _ => None,
    }
}

pub fn hex(rgb: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
}

/// Hue 0–360, saturation and value 0–100.
pub fn from_hsv(h: f64, s: f64, v: f64) -> [u8; 3] {
    let (h, s, v) = (h.rem_euclid(360.0), (s / 100.0).clamp(0.0, 1.0), (v / 100.0).clamp(0.0, 1.0));
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    let to = |f: f64| ((f + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    [to(r), to(g), to(b)]
}

pub fn to_hsv(rgb: [u8; 3]) -> (f64, f64, f64) {
    let [r, g, b] = rgb.map(|c| c as f64 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    let s = if max == 0.0 { 0.0 } else { d / max };
    (h.round(), (s * 100.0).round(), (max * 100.0).round())
}

fn num(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// Colour builtins; `None` if `name` isn't one of them.
pub fn builtin(name: &str, a: &[Value]) -> Option<Result<Value, String>> {
    let arg = |i: usize| a.get(i).cloned().unwrap_or(Value::Null);
    let n = |i: usize| num(&arg(i)).unwrap_or(0.0);
    let colour = |i: usize| {
        parse(&arg(i)).ok_or_else(|| format!("'{}' is not a colour", crate::value::to_text(&arg(i))))
    };
    Some(match name {
        "colour_rgb" => Ok(json!(hex([n(0), n(1), n(2)].map(|c| c.round().clamp(0.0, 255.0) as u8)))),
        "colour_hsv" => Ok(json!(hex(from_hsv(n(0), n(1), n(2))))),
        "colour_blend" => (|| {
            let (x, y) = (colour(0)?, colour(1)?);
            let t = (n(2) / 100.0).clamp(0.0, 1.0);
            let mix = |i: usize| (x[i] as f64 + (y[i] as f64 - x[i] as f64) * t).round() as u8;
            Ok(json!(hex([mix(0), mix(1), mix(2)])))
        })(),
        "colour_random" => Ok(json!(hex(from_hsv(rand::random::<f64>() * 360.0, 100.0, 100.0)))),
        "colour_part" => (|| {
            let c = colour(0)?;
            let (h, s, v) = to_hsv(c);
            Ok(match crate::value::to_text(&arg(1)).as_str() {
                "red" => json!(c[0]),
                "green" => json!(c[1]),
                "blue" => json!(c[2]),
                "hue" => json!(h),
                "saturation" => json!(s),
                "value" => json!(v),
                "rgb" => json!(c),
                _ => json!(hex(c)),
            })
        })(),
        "light_data" => Ok(light_data(&arg(0), &arg(1), &arg(2), &arg(3))),
        "light_step" => {
            let mut d = Map::new();
            let sign = if crate::value::to_text(&arg(1)) == "dim" { -1.0 } else { 1.0 };
            d.insert("brightness_step_pct".into(), json!(sign * n(0).abs().clamp(0.0, 100.0)));
            add_transition(&mut d, &arg(2));
            Ok(Value::Object(d))
        }
        _ => return None,
    })
}

fn add_transition(d: &mut Map<String, Value>, t: &Value) {
    if let Some(t) = num(t).filter(|t| *t > 0.0) {
        d.insert("transition".into(), json!(t));
    }
}

/// Data for `light.turn_on`: `kind` is "colour" (value = a colour or a colour name), "white"
/// (value = kelvin) or anything else (no colour). Empty brightness/transition are left out.
pub fn light_data(kind: &Value, value: &Value, brightness: &Value, transition: &Value) -> Value {
    let mut d = Map::new();
    match kind.as_str() {
        Some("colour") => match parse(value) {
            Some(rgb) => {
                d.insert("rgb_color".into(), json!(rgb));
            }
            None => {
                if let Some(name) = value.as_str().filter(|s| !s.trim().is_empty()) {
                    d.insert("color_name".into(), json!(name.trim().to_lowercase()));
                }
            }
        },
        Some("white") => {
            if let Some(k) = num(value) {
                d.insert("color_temp_kelvin".into(), json!(k.round().clamp(1000.0, 12000.0) as u32));
            }
        }
        _ => {}
    }
    if let Some(b) = num(brightness) {
        d.insert("brightness_pct".into(), json!(b.round().clamp(0.0, 100.0)));
    }
    add_transition(&mut d, transition);
    Value::Object(d)
}

/// The colour of a light from its state (`rgb_color`), or null when it's off or has none.
pub fn of_state(state: &Value) -> Value {
    if state.get("state").and_then(Value::as_str) != Some("on") {
        return Value::Null;
    }
    state.pointer("/attributes/rgb_color").and_then(parse).map_or(Value::Null, |c| json!(hex(c)))
}

/// Brightness of a light in percent (0 when off).
pub fn brightness_pct(state: &Value) -> Value {
    if state.get("state").and_then(Value::as_str) != Some("on") {
        return json!(0);
    }
    let b = state.pointer("/attributes/brightness").and_then(Value::as_f64).unwrap_or(255.0);
    json!((b / 255.0 * 100.0).round())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_converts() {
        assert_eq!(parse(&json!("#ff8000")), Some([255, 128, 0]));
        assert_eq!(parse(&json!("#f80")), Some([255, 136, 0]));
        assert_eq!(parse(&json!([1, 2, 3])), Some([1, 2, 3]));
        assert_eq!(parse(&json!({"r": 9, "g": 8, "b": 7})), Some([9, 8, 7]));
        assert_eq!(parse(&json!("Warm White")), Some([255, 214, 170]));
        assert_eq!(parse(&json!("nope")), None);
        assert_eq!(hex(from_hsv(120.0, 100.0, 100.0)), "#00ff00");
        assert_eq!(to_hsv([255, 0, 0]), (0.0, 100.0, 100.0));
        assert_eq!(
            builtin("colour_blend", &[json!("#000000"), json!("#ffffff"), json!(50)]).unwrap().unwrap(),
            json!("#808080")
        );
        assert_eq!(builtin("colour_part", &[json!("#102030"), json!("green")]).unwrap().unwrap(), json!(32));
    }

    #[test]
    fn light_actions() {
        assert_eq!(
            light_data(&json!("colour"), &json!("#ff0000"), &json!(40), &json!(2)),
            json!({"rgb_color": [255, 0, 0], "brightness_pct": 40.0, "transition": 2.0})
        );
        assert_eq!(
            light_data(&json!("colour"), &json!("rebeccapurple"), &Value::Null, &json!(0)),
            json!({"color_name": "rebeccapurple"})
        );
        assert_eq!(
            light_data(&json!("white"), &json!(2700), &json!(100), &Value::Null),
            json!({"color_temp_kelvin": 2700, "brightness_pct": 100.0})
        );
        assert_eq!(
            builtin("light_step", &[json!(10), json!("dim"), json!(1)]).unwrap().unwrap(),
            json!({"brightness_step_pct": -10.0, "transition": 1.0})
        );
        let on = json!({"state": "on", "attributes": {"rgb_color": [255, 0, 0], "brightness": 128}});
        assert_eq!(of_state(&on), json!("#ff0000"));
        assert_eq!(brightness_pct(&on), json!(50.0));
        assert_eq!(of_state(&json!({"state": "off"})), Value::Null);
    }
}
