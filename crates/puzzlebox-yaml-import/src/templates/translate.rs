//! Jinja expressions → blocks, only where the blocks give the same result as Home Assistant.
//!
//! Every translated value carries the Python type it has in the template, because Jinja is
//! strictly typed where Puzzlebox is loose: `'5' == 5` is false in Jinja but true in a
//! Puzzlebox comparison, `'9' < '10'` compares as text in Jinja and as numbers in Puzzlebox,
//! `True` prints as "True", `20.0` as "20.0" (Puzzlebox: "20"), and `round` rounds half to
//! even. The plain blocks are used where the types prove they agree, the "like Home
//! Assistant" blocks (strict compare, rounding, printing, float/int) where they don't; if
//! neither is exact, translation fails and the caller keeps the template.

use serde_json::{Value, json};

use super::parse::{Args, Cmp, E, Op};
use crate::Builder;

/// The Python type of a translated value, as far as it is known.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Ty {
    Bool,
    Int,
    /// A float in Jinja; Puzzlebox may hold whole numbers as integers.
    Float,
    /// Int or float.
    Num,
    Str,
    /// A string, or undefined in Jinja where the block gives null (absent trigger details,
    /// `states.x.y.state` of a missing entity). Both print as "" and are false.
    UStr,
    None,
    /// Any JSON value (variables, attributes); in Jinja possibly undefined or None.
    Any,
}

impl Ty {
    fn numeric(self) -> bool {
        matches!(self, Ty::Int | Ty::Float | Ty::Num)
    }

    fn join(self, other: Ty) -> Ty {
        match (self, other) {
            (a, b) if a == b => a,
            (a, b) if a.numeric() && b.numeric() => Ty::Num,
            (Ty::Str | Ty::UStr, Ty::Str | Ty::UStr) => Ty::UStr,
            _ => Ty::Any,
        }
    }

    /// Prints the same with Python's `str()` as with Puzzlebox's text conversion.
    fn prints_same(self) -> bool {
        matches!(self, Ty::Int | Ty::Str | Ty::UStr)
    }

    /// Ordering these two raises in Python whatever the values are.
    fn never_ordered(self, other: Ty) -> bool {
        let num = |t: Ty| t.numeric() || t == Ty::Bool;
        let known = |t: Ty| !matches!(t, Ty::Any | Ty::UStr);
        self == Ty::None
            || other == Ty::None
            || known(self)
                && known(other)
                && !(num(self) && num(other) || self == Ty::Str && other == Ty::Str)
    }
}

/// Undefined in Jinja where the block gives null, which `is none` tells apart.
fn maybe_undefined(t: &Tr, e: &E) -> bool {
    t.ty == Ty::UStr || t.ty == Ty::Any && !matches!(call_name(e), Some(("state_attr", _)))
}

/// strftime codes that Python (glibc) and the engine (chrono) write the same way.
fn strftime_ok(f: &str) -> bool {
    let mut chars = f.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            continue;
        }
        let ok = match chars.next() {
            Some('-') => chars.next().is_some_and(|c| "dmHIMSjy".contains(c)),
            Some(c) => "HMSdmYyIpAaBbjuwez%".contains(c),
            None => false,
        };
        if !ok {
            return false;
        }
    }
    true
}

pub(crate) struct Tr {
    pub b: Value,
    pub ty: Ty,
}

/// Names HA defines in scripts that aren't Puzzlebox variables.
const RESERVED: [&str; 6] = ["trigger", "this", "context", "repeat", "wait", "states"];

/// Dictionary methods: `x.items` is the method, not the key.
const DICT_METHODS: [&str; 11] =
    ["items", "keys", "values", "get", "update", "pop", "copy", "clear", "setdefault", "popitem", "fromkeys"];

/// What Puzzlebox treats as a number when comparing text (so comparisons turn numeric).
fn numeric_text(s: &str) -> bool {
    s.trim().parse::<f64>().is_ok()
}

fn valid_entity(s: &str) -> bool {
    let ok = |p: &str| {
        !p.is_empty() && p.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
    };
    matches!(s.split_once('.'), Some((d, o)) if ok(d) && ok(o))
}

fn ident(s: &str) -> bool {
    !s.is_empty()
        && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
        && !s.as_bytes()[0].is_ascii_digit()
        && !DICT_METHODS.contains(&s)
}

fn lit_str(e: &E) -> Option<&str> {
    match e {
        E::Str(s) => Some(s),
        _ => None,
    }
}

fn lit_num(e: &E) -> Option<f64> {
    match e {
        E::Int(n) => Some(*n as f64),
        E::Float(f) => Some(*f),
        E::Neg(inner) => lit_num(inner).map(|f| -f),
        _ => None,
    }
}

fn lit_int(e: &E) -> Option<i64> {
    match e {
        E::Int(n) => Some(*n),
        E::Neg(inner) => lit_int(inner).map(|n| -n),
        _ => None,
    }
}

fn entity_arg(e: &E) -> Option<&str> {
    lit_str(e).filter(|s| valid_entity(s))
}

fn call_name(e: &E) -> Option<(&str, &Args)> {
    match e {
        E::Call(f, args) => match &**f {
            E::Name(n) => Some((n.as_str(), args)),
            _ => None,
        },
        _ => None,
    }
}

/// `states('x.y')`.
fn states_call(e: &E) -> Option<&str> {
    let (name, args) = call_name(e)?;
    (name == "states" && args.kw.is_empty() && args.pos.len() == 1).then(|| entity_arg(&args.pos[0]))?
}

fn is_now(e: &E) -> bool {
    matches!(call_name(e), Some(("now", a)) if a.pos.is_empty() && a.kw.is_empty())
}

/// A `Name.attr.attr` chain.
fn path(e: &E) -> Option<(&str, Vec<&str>)> {
    match e {
        E::Name(n) => Some((n, vec![])),
        E::Attr(base, attr) => {
            let (root, mut parts) = path(base)?;
            parts.push(attr);
            Some((root, parts))
        }
        _ => None,
    }
}

/// The positional argument `i`, or keyword `kw`.
fn arg<'a>(args: &'a Args, i: usize, kw: &str) -> Option<&'a E> {
    args.pos.get(i).or_else(|| args.kw.iter().find(|(k, _)| k == kw).map(|(_, v)| v))
}

fn only_args(args: &Args, max_pos: usize, kws: &[&str]) -> bool {
    args.pos.len() <= max_pos && args.kw.iter().all(|(k, _)| kws.contains(&k.as_str()))
}

/// Evaluating it can't fail, so evaluating it when Jinja wouldn't changes nothing.
fn infallible(e: &E) -> bool {
    match e {
        E::Str(_) | E::Int(_) | E::Float(_) | E::Bool(_) | E::None | E::Name(_) | E::Attr(..) => true,
        E::Neg(inner) => lit_num(inner).is_some(),
        _ => states_call(e).is_some(),
    }
}

pub(crate) struct Tx<'a> {
    pub b: &'a mut Builder,
}

impl Tx<'_> {
    fn blk(&mut self, ty: &str, fields: Value, inputs: Vec<(&str, Option<Value>)>) -> Value {
        self.b.block(ty, fields, inputs, None)
    }

    fn tr(&mut self, b: Value, ty: Ty) -> Option<Tr> {
        Some(Tr { b, ty })
    }

    fn entity_block(
        &mut self,
        ty: &str,
        entity: &str,
        fields: Value,
        inputs: Vec<(&str, Option<Value>)>,
    ) -> Value {
        let shadow = json!({"type": "ha_entity", "id": self.b.id(), "fields": {"ENTITY": entity}});
        let mut b = self.blk(ty, fields, inputs);
        if b.get("inputs").is_none() {
            b["inputs"] = json!({});
        }
        b["inputs"]["ENTITY"] = json!({"shadow": shadow});
        b
    }

    fn number(&mut self, f: f64) -> Value {
        let n = if f.fract() == 0.0 && f.abs() < 9.0e15 { json!(f as i64) } else { json!(f) };
        self.blk("math_number", json!({"NUM": n}), vec![])
    }

    fn text(&mut self, s: &str) -> Value {
        self.blk("text", json!({"TEXT": s}), vec![])
    }

    fn null(&mut self) -> Value {
        self.blk("logic_null", json!({}), vec![])
    }

    fn boolean(&mut self, v: bool) -> Value {
        self.blk("logic_boolean", json!({"BOOL": if v { "TRUE" } else { "FALSE" }}), vec![])
    }

    fn compare(&mut self, op: &str, a: Value, b: Value) -> Value {
        self.blk("logic_compare", json!({"OP": op}), vec![("A", Some(a)), ("B", Some(b))])
    }

    fn logic(&mut self, op: &str, a: Value, b: Value) -> Value {
        self.blk("logic_operation", json!({"OP": op}), vec![("A", Some(a)), ("B", Some(b))])
    }

    fn negate(&mut self, a: Value) -> Value {
        self.blk("logic_negate", json!({}), vec![("BOOL", Some(a))])
    }

    fn ternary(&mut self, c: Value, a: Value, b: Value) -> Value {
        self.blk("logic_ternary", json!({}), vec![("IF", Some(c)), ("THEN", Some(a)), ("ELSE", Some(b))])
    }

    /// Compares like Python: text is never read as a number.
    fn strict(&mut self, op: &str, a: Value, b: Value) -> Value {
        self.blk("logic_compare_strict", json!({"OP": op}), vec![("A", Some(a)), ("B", Some(b))])
    }

    fn py_text(&mut self, v: Value, float: bool) -> Value {
        let mode = if float { "float" } else { "auto" };
        self.blk("to_text_ha", json!({"NUMBERS": mode}), vec![("VALUE", Some(v))])
    }

    fn arith(&mut self, op: &str, a: Value, b: Value) -> Value {
        self.blk("math_arithmetic", json!({"OP": op}), vec![("A", Some(a)), ("B", Some(b))])
    }

    fn items(&mut self, ty: &str, items: Vec<Value>) -> Value {
        let n = items.len();
        let mut b = self.b.block(ty, json!({}), vec![], Some(json!({"itemCount": n})));
        if n > 0 {
            let ins: serde_json::Map<String, Value> = items
                .into_iter()
                .enumerate()
                .map(|(i, v)| (format!("ADD{i}"), json!({"block": v})))
                .collect();
            b["inputs"] = Value::Object(ins);
        }
        b
    }

    pub fn text_join(&mut self, items: Vec<Value>) -> Value {
        self.items("text_join", items)
    }

    /// The state as HA's `states()` gives it: null for a missing entity in Puzzlebox, but
    /// "unknown" in HA. Only for uses where the two behave the same.
    fn state_plain(&mut self, entity: &str) -> Value {
        self.entity_block("ha_state", entity, json!({}), vec![])
    }

    /// `states()` exactly: "unknown" for a missing entity.
    fn state_exact(&mut self, entity: &str) -> Value {
        let s = self.state_plain(entity);
        let null = self.null();
        let missing = self.compare("EQ", s, null);
        let unknown = self.text("unknown");
        let s = self.state_plain(entity);
        self.ternary(missing, unknown, s)
    }

    fn attribute(&mut self, entity: &str, attr: &str) -> Option<Value> {
        // The engine reads attributes by JSON pointer.
        if attr.is_empty() || attr.contains(['/', '~']) {
            return None;
        }
        Some(self.entity_block("ha_attribute", entity, json!({"ATTRIBUTE": attr}), vec![]))
    }

    /// A value block for an expression.
    pub fn value(&mut self, e: &E) -> Option<Tr> {
        if let Some(entity) = states_call(e) {
            let b = self.state_exact(entity);
            return self.tr(b, Ty::Str);
        }
        match e {
            E::Str(s) => {
                let b = self.text(s);
                self.tr(b, Ty::Str)
            }
            E::Int(n) => {
                let b = self.number(*n as f64);
                self.tr(b, Ty::Int)
            }
            E::Float(f) => {
                let b = self.number(*f);
                self.tr(b, Ty::Float)
            }
            E::Bool(v) => {
                let b = self.boolean(*v);
                self.tr(b, Ty::Bool)
            }
            E::None => {
                let b = self.null();
                self.tr(b, Ty::None)
            }
            E::List(_) => None,
            E::Name(n) => {
                if RESERVED.contains(&n.as_str()) || !self.b.vars.iter().any(|(v, _)| v == n) {
                    return None;
                }
                let id = self.b.var_id(n);
                let b = self.blk("variables_get", json!({"VAR": {"id": id}}), vec![]);
                self.tr(b, Ty::Any)
            }
            E::Attr(base, attr) => {
                if is_now(base) {
                    let part = match attr.as_str() {
                        p @ ("hour" | "minute" | "second" | "day" | "month" | "year") => p,
                        _ => return None,
                    };
                    let b = self.blk("time_part", json!({"PART": part}), vec![]);
                    return self.tr(b, Ty::Int);
                }
                self.path_value(e)
            }
            E::Call(f, args) => self.call(f, args),
            E::Filter(x, name, args) => self.filter(x, name, args),
            E::Neg(inner) => {
                if let Some(f) = lit_num(e) {
                    let b = self.number(f);
                    return self.tr(b, if matches!(**inner, E::Float(_)) { Ty::Float } else { Ty::Int });
                }
                let t = self.value(inner)?;
                if !t.ty.numeric() {
                    return None;
                }
                let b = self.blk("math_single", json!({"OP": "NEG"}), vec![("NUM", Some(t.b))]);
                self.tr(b, t.ty)
            }
            E::Pos(inner) => self.value(inner).filter(|t| t.ty.numeric()),
            E::Not(inner) => {
                let t = self.value(inner)?;
                let b = self.negate(t.b);
                self.tr(b, Ty::Bool)
            }
            E::Bin(op, l, r) => self.binary(*op, l, r),
            E::IsNone(x) => {
                let t = self.value(x)?;
                if maybe_undefined(&t, x) {
                    return None;
                }
                let b = self.blk("logic_is_null", json!({}), vec![("VALUE", Some(t.b))]);
                self.tr(b, Ty::Bool)
            }
            E::Cmp(op, l, r) => {
                let b = self.cmp(*op, l, r)?;
                self.tr(b, Ty::Bool)
            }
            E::Cond { cond, then, otherwise } => {
                let otherwise = otherwise.as_ref()?;
                let c = self.value(cond)?;
                let a = self.value(then)?;
                let b = self.value(otherwise)?;
                let ty = a.ty.join(b.ty);
                let out = self.ternary(c.b, a.b, b.b);
                self.tr(out, ty)
            }
        }
    }

    /// A true/false block, for expressions that are certainly booleans.
    pub fn boolean_expr(&mut self, e: &E) -> Option<Value> {
        self.value(e).filter(|t| t.ty == Ty::Bool).map(|t| t.b)
    }

    /// A block giving the text Jinja prints for the expression.
    pub fn text_of(&mut self, e: &E) -> Option<Vec<Value>> {
        if let E::Bin(Op::Concat, l, r) = e {
            let mut items = self.text_of(l)?;
            items.extend(self.text_of(r)?);
            return Some(items);
        }
        let t = self.value(e)?;
        let b = match t.ty {
            ty if ty.prints_same() => t.b,
            Ty::Bool | Ty::None => self.py_text(t.b, false),
            // Python's timestamps have microseconds, Puzzlebox's milliseconds.
            Ty::Float if !format!("{e:?}").contains("timestamp") => self.py_text(t.b, true),
            _ => return None,
        };
        Some(vec![b])
    }

    /// `trigger.…`, `states.domain.object.…`.
    fn path_value(&mut self, e: &E) -> Option<Tr> {
        let (root, parts) = path(e)?;
        if !parts.iter().all(|p| ident(p)) {
            return None;
        }
        match root {
            "trigger" => {
                let (ty, fields, what) = match parts.as_slice() {
                    ["entity_id"] => {
                        let b = self.blk("trigger_entity", json!({}), vec![]);
                        return self.tr(b, Ty::UStr);
                    }
                    ["to_state", "state"] => (Ty::UStr, json!({"WHAT": "to", "ATTR": ""}), true),
                    ["from_state", "state"] => (Ty::UStr, json!({"WHAT": "from", "ATTR": ""}), true),
                    ["to_state", "attributes", a] => (Ty::Any, json!({"WHAT": "to_attr", "ATTR": a}), true),
                    ["from_state", "attributes", a] => {
                        (Ty::Any, json!({"WHAT": "from_attr", "ATTR": a}), true)
                    }
                    ["id" | "platform"] => (Ty::UStr, json!({"PATH": parts[0]}), false),
                    [] => return None,
                    _ => (Ty::Any, json!({"PATH": parts.join(".")}), false),
                };
                let b = self.blk(if what { "trigger_info" } else { "ha_trigger_data" }, fields, vec![]);
                self.tr(b, ty)
            }
            // Inside a loop block, which sets `repeat` like HA does for that kind of loop.
            "repeat" if !self.b.loops.is_empty() => {
                let mode = self.b.loops.last().copied().unwrap_or_default();
                let (what, ty) = match parts.as_slice() {
                    ["index"] => ("index", Ty::Int),
                    ["first"] => ("first", Ty::Bool),
                    ["last"] if matches!(mode, "count" | "for_each") => ("last", Ty::Bool),
                    ["item"] if mode == "for_each" => ("item", Ty::Any),
                    _ => return None,
                };
                let b = self.blk("loop_info", json!({"WHAT": what}), vec![]);
                self.tr(b, ty)
            }
            "states" => match parts.as_slice() {
                [d, o, "state"] if valid_entity(&format!("{d}.{o}")) => {
                    let b = self.state_plain(&format!("{d}.{o}"));
                    self.tr(b, Ty::UStr)
                }
                [d, o, "attributes", a] if valid_entity(&format!("{d}.{o}")) => {
                    let b = self.attribute(&format!("{d}.{o}"), a)?;
                    self.tr(b, Ty::Any)
                }
                _ => None,
            },
            _ => None,
        }
    }

    fn call(&mut self, f: &E, args: &Args) -> Option<Tr> {
        if let E::Attr(base, method) = f
            && is_now(base)
            && method == "strftime"
            && args.kw.is_empty()
            && let [E::Str(fmt)] = args.pos.as_slice()
        {
            return self.time_format(fmt);
        }
        // now().weekday() / isoweekday() / timestamp()
        if let E::Attr(base, method) = f
            && is_now(base)
            && args.pos.is_empty()
            && args.kw.is_empty()
        {
            return match method.as_str() {
                "isoweekday" => {
                    let b = self.blk("time_part", json!({"PART": "weekday"}), vec![]);
                    self.tr(b, Ty::Int)
                }
                "weekday" => {
                    let d = self.blk("time_part", json!({"PART": "weekday"}), vec![]);
                    let one = self.number(1.0);
                    let b = self.arith("MINUS", d, one);
                    self.tr(b, Ty::Int)
                }
                "timestamp" => {
                    let b = self.blk("time_part", json!({"PART": "timestamp"}), vec![]);
                    self.tr(b, Ty::Float)
                }
                _ => None,
            };
        }
        let E::Name(name) = f else { return None };
        let pos = &args.pos;
        match name.as_str() {
            "is_state" if args.kw.is_empty() && pos.len() == 2 => {
                let entity = entity_arg(&pos[0])?;
                let wanted: Vec<&str> = match &pos[1] {
                    E::Str(s) => vec![s],
                    E::List(items) => items.iter().map(lit_str).collect::<Option<_>>()?,
                    _ => return None,
                };
                // A missing entity is null here, which equals none of the texts, like HA's
                // is_state (also for 'unknown').
                let mut acc: Option<Value> = None;
                for w in wanted {
                    let s = self.state_plain(entity);
                    let t = self.text(w);
                    let eq = if numeric_text(w) { self.strict("EQ", s, t) } else { self.compare("EQ", s, t) };
                    acc = Some(match acc {
                        Some(prev) => self.logic("OR", prev, eq),
                        None => eq,
                    });
                }
                let b = match acc {
                    Some(b) => b,
                    None => self.boolean(false),
                };
                self.tr(b, Ty::Bool)
            }
            "state_attr" if args.kw.is_empty() && pos.len() == 2 => {
                let entity = entity_arg(&pos[0])?;
                let b = self.attribute(entity, lit_str(&pos[1])?)?;
                self.tr(b, Ty::Any)
            }
            "is_state_attr" if args.kw.is_empty() && pos.len() == 3 => {
                let entity = entity_arg(&pos[0])?;
                let attr = lit_str(&pos[1])?;
                // Puzzlebox compares "5" and 5 as equal, Jinja doesn't; text and true/false agree.
                let (v, strict) = match &pos[2] {
                    E::Str(s) => (self.text(s), numeric_text(s)),
                    E::Bool(v) => (self.boolean(*v), false),
                    n => (self.number(lit_num(n)?), true),
                };
                let a = self.attribute(entity, attr)?;
                let b = if strict { self.strict("EQ", a, v) } else { self.compare("EQ", a, v) };
                self.tr(b, Ty::Bool)
            }
            "has_value" if args.kw.is_empty() && pos.len() == 1 => {
                let entity = entity_arg(&pos[0])?;
                let b = self.entity_block("ha_has_value", entity, json!({}), vec![]);
                self.tr(b, Ty::Bool)
            }
            "float" | "int" if only_args(args, 2, &["default"]) && !pos.is_empty() => {
                self.convert(&pos[0], arg(args, 1, "default"), name == "int")
            }
            "as_timestamp" if args.kw.is_empty() && pos.len() == 1 && is_now(&pos[0]) => {
                let b = self.blk("time_part", json!({"PART": "timestamp"}), vec![]);
                self.tr(b, Ty::Float)
            }
            "iif" if args.kw.is_empty() && (1..=3).contains(&pos.len()) => {
                self.iif(&pos[0], pos.get(1), pos.get(2))
            }
            "min" | "max" if args.kw.is_empty() && !pos.is_empty() => {
                let items: &[E] = match pos.as_slice() {
                    [E::List(items)] => items,
                    [_] => return None,
                    many => many,
                };
                self.min_max(name, items)
            }
            "area_entities"
            | "floor_entities"
            | "label_entities"
            | "device_entities"
            | "integration_entities"
                if args.kw.is_empty() && pos.len() == 1 =>
            {
                // The block renders `{{ <func>('<name>') | list }}`, dropping quotes and backslashes.
                let n = lit_str(&pos[0])
                    .filter(|n| !n.is_empty() && !n.contains(['\'', '\\']) && n.trim() == *n)?;
                let kind = name.trim_end_matches("_entities");
                let b = self.blk("ha_entities_in", json!({"DOMAIN": "*", "KIND": kind, "NAME": n}), vec![]);
                self.tr(b, Ty::Any)
            }
            _ => None,
        }
    }

    fn iif(&mut self, c: &E, then: Option<&E>, otherwise: Option<&E>) -> Option<Tr> {
        // iif evaluates both branches; a ternary block only one.
        let (t, f) = (then.unwrap_or(&E::Bool(true)), otherwise.unwrap_or(&E::Bool(false)));
        if !infallible(t) || !infallible(f) {
            return None;
        }
        let c = self.value(c)?;
        let a = self.value(t)?;
        let b = self.value(f)?;
        let ty = a.ty.join(b.ty);
        let out = self.ternary(c.b, a.b, b.b);
        self.tr(out, ty)
    }

    fn min_max(&mut self, name: &str, items: &[E]) -> Option<Tr> {
        if items.is_empty() {
            return None;
        }
        let mut ty: Option<Ty> = None;
        let mut blocks = Vec::new();
        for it in items {
            let t = self.value(it)?;
            if !t.ty.numeric() {
                return None;
            }
            ty = Some(ty.map_or(t.ty, |p| p.join(t.ty)));
            blocks.push(t.b);
        }
        let list = self.items("lists_create_with", blocks);
        let op = if name == "min" { "MIN" } else { "MAX" };
        let mut b = self.blk("math_on_list", json!({"OP": op}), vec![("LIST", Some(list))]);
        b["extraState"] = json!({"op": op});
        self.tr(b, ty?)
    }

    fn filter(&mut self, x: &E, name: &str, args: &Args) -> Option<Tr> {
        let pos = &args.pos;
        match name {
            "float" | "int" if only_args(args, 1, &["default"]) => {
                self.convert(x, arg(args, 0, "default"), name == "int")
            }
            "lower" | "upper" if pos.is_empty() && args.kw.is_empty() => {
                let t = self.value(x)?;
                if !matches!(t.ty, Ty::Str | Ty::UStr) {
                    return None;
                }
                let case = if name == "lower" { "LOWERCASE" } else { "UPPERCASE" };
                let b = self.blk("text_changeCase", json!({"CASE": case}), vec![("TEXT", Some(t.b))]);
                self.tr(b, Ty::Str)
            }
            "abs" if pos.is_empty() && args.kw.is_empty() => {
                let t = self.value(x)?;
                if !t.ty.numeric() {
                    return None;
                }
                let b = self.blk("math_single", json!({"OP": "ABS"}), vec![("NUM", Some(t.b))]);
                self.tr(b, t.ty)
            }
            "string" if pos.is_empty() && args.kw.is_empty() => {
                let t = self.value(x)?;
                match t.ty {
                    Ty::Str => Some(t),
                    Ty::UStr | Ty::Int => {
                        let b = self.blk("to_text", json!({}), vec![("VALUE", Some(t.b))]);
                        self.tr(b, Ty::Str)
                    }
                    _ => None,
                }
            }
            "round" if only_args(args, 2, &["precision", "method"]) => {
                let precision = arg(args, 0, "precision").map_or(Some(0), lit_int)?;
                let method = arg(args, 1, "method").map_or(Some("common"), lit_str)?;
                if !(0..=15).contains(&precision) {
                    return None;
                }
                // Puzzlebox's round goes away from zero, so only floor and ceil use it.
                let op = match method {
                    "floor" => "ROUNDDOWN",
                    "ceil" => "ROUNDUP",
                    _ => "",
                };
                if precision == 0
                    && !op.is_empty()
                    && let Some(t) = self.value_if_numeric(x)
                {
                    let b = self.blk("math_round", json!({"OP": op}), vec![("NUM", Some(t.b))]);
                    return self.tr(b, Ty::Int);
                }
                // HA treats any other method as "common".
                let method = if matches!(method, "floor" | "ceil" | "half") { method } else { "common" };
                let t = self.number_input(x)?;
                let b = self.blk(
                    "math_round_ha",
                    json!({"DECIMALS": precision, "METHOD": method}),
                    vec![("NUM", Some(t))],
                );
                self.tr(b, if precision == 0 { Ty::Int } else { Ty::Float })
            }
            "timestamp_custom" if only_args(args, 2, &["date_format", "local"]) => {
                let is_ts = match x {
                    E::Call(f, a) => {
                        a.pos.is_empty()
                            && a.kw.is_empty()
                            && matches!(&**f, E::Attr(base, m) if m == "timestamp" && is_now(base))
                            || matches!(call_name(x), Some(("as_timestamp", a)) if a.kw.is_empty() && matches!(a.pos.as_slice(), [n] if is_now(n)))
                    }
                    _ => false,
                };
                let local = arg(args, 1, "local").is_none_or(|l| matches!(l, E::Bool(true)));
                match arg(args, 0, "date_format") {
                    Some(E::Str(fmt)) if is_ts && local => self.time_format(fmt),
                    None if is_ts && local => self.time_format("%Y-%m-%d"),
                    _ => None,
                }
            }
            "default" | "d" if pos.len() == 1 && args.kw.is_empty() => {
                // Only applies to undefined values; these never are.
                let never_undefined = states_call(x).is_some()
                    || matches!(call_name(x), Some(("state_attr", _)))
                    || matches!(x, E::Str(_) | E::Int(_) | E::Float(_) | E::Bool(_));
                if !never_undefined {
                    return None;
                }
                self.value(x)
            }
            "min" | "max" if pos.is_empty() && args.kw.is_empty() => match x {
                E::List(items) => self.min_max(name, items),
                _ => None,
            },
            "iif" if args.kw.is_empty() && pos.len() <= 2 => self.iif(x, pos.first(), pos.get(1)),
            _ => None,
        }
    }

    /// `float(x, default)` / `int(x, default)`, also as filters.
    fn convert(&mut self, x: &E, default: Option<&E>, int: bool) -> Option<Tr> {
        if let Some(t) = self.value_if_numeric(x) {
            // float(number) is that number; int() of a float truncates.
            return match (int, t.ty) {
                (false, _) => self.tr(t.b, Ty::Float),
                (true, Ty::Int) => Some(t),
                _ => {
                    let b = self.blk("to_number_ha", json!({"KIND": "int"}), vec![("VALUE", Some(t.b))]);
                    self.tr(b, Ty::Int)
                }
            };
        }
        match default {
            Some(d) if !int => self.float_with_default(x, d),
            _ => self.ha_number(x, default, int),
        }
    }

    /// The input of float() and friends; a missing entity's null fails like "unknown".
    fn number_input(&mut self, x: &E) -> Option<Value> {
        match states_call(x) {
            Some(entity) => Some(self.state_plain(entity)),
            None => self.value(x).map(|t| t.b),
        }
    }

    /// The "as a number like Home Assistant" block: Python's float() or int(), with the
    /// default if it fails, or an error without one.
    fn ha_number(&mut self, x: &E, default: Option<&E>, int: bool) -> Option<Tr> {
        let v = self.number_input(x)?;
        let base = if int { Ty::Int } else { Ty::Float };
        let (d, ty) = match default {
            Some(d) => {
                let t = self.value(d)?;
                let ty = base.join(t.ty);
                (Some(t.b), ty)
            }
            None => (None, base),
        };
        let kind = if int { "int" } else { "float" };
        let b = self.blk("to_number_ha", json!({"KIND": kind}), vec![("VALUE", Some(v)), ("DEFAULT", d)]);
        self.tr(b, ty)
    }

    fn time_format(&mut self, fmt: &str) -> Option<Tr> {
        if !strftime_ok(fmt) {
            return None;
        }
        let now = self.blk("time_now", json!({}), vec![]);
        let b = self.blk("time_format", json!({"FORMAT": fmt}), vec![("DATETIME", Some(now))]);
        self.tr(b, Ty::Str)
    }

    fn value_if_numeric(&mut self, x: &E) -> Option<Tr> {
        if states_call(x).is_some() || matches!(x, E::Str(_)) {
            return None;
        }
        let saved = self.b.next_id;
        let t = self.value(x);
        match t {
            Some(t) if t.ty.numeric() => Some(t),
            _ => {
                self.b.next_id = saved;
                None
            }
        }
    }

    /// `float(x, d)` for a value that isn't certainly a number.
    fn float_with_default(&mut self, x: &E, d: &E) -> Option<Tr> {
        if let (Some(entity), Some(dv)) = (states_call(x), lit_num(d)) {
            let dty = if matches!(d, E::Float(_)) || matches!(d, E::Neg(i) if matches!(**i, E::Float(_))) {
                Ty::Float
            } else {
                Ty::Int
            };
            let default = self.number(dv);
            let b = self.entity_block("ha_number_state", entity, json!({}), vec![("DEFAULT", Some(default))]);
            return self.tr(b, Ty::Float.join(dty));
        }
        self.ha_number(x, Some(d), false)
    }

    fn binary(&mut self, op: Op, l: &E, r: &E) -> Option<Tr> {
        match op {
            Op::And | Op::Or => {
                // Jinja's and/or return one of the operands, so only booleans stay booleans.
                let a = self.boolean_expr(l)?;
                let b = self.boolean_expr(r)?;
                let out = self.logic(if op == Op::And { "AND" } else { "OR" }, a, b);
                self.tr(out, Ty::Bool)
            }
            Op::Concat => {
                let items = self.text_of(&E::Bin(Op::Concat, Box::new(l.clone()), Box::new(r.clone())))?;
                let b = self.text_join(items);
                self.tr(b, Ty::Str)
            }
            _ => {
                let a = self.value(l)?;
                let b = self.value(r)?;
                if op == Op::Add && a.ty == Ty::Str && b.ty == Ty::Str {
                    let out = self.text_join(vec![a.b, b.b]);
                    return self.tr(out, Ty::Str);
                }
                if !a.ty.numeric() || !b.ty.numeric() {
                    return None;
                }
                let both_int = a.ty == Ty::Int && b.ty == Ty::Int;
                let float = a.ty == Ty::Float || b.ty == Ty::Float;
                let ty = if both_int {
                    Ty::Int
                } else if float {
                    Ty::Float
                } else {
                    Ty::Num
                };
                let out = match op {
                    Op::Add => self.arith("ADD", a.b, b.b),
                    Op::Sub => self.arith("MINUS", a.b, b.b),
                    Op::Mul => self.arith("MULTIPLY", a.b, b.b),
                    Op::Div => {
                        let out = self.arith("DIVIDE", a.b, b.b);
                        return self.tr(out, Ty::Float);
                    }
                    Op::FloorDiv => {
                        let q = self.arith("DIVIDE", a.b, b.b);
                        self.blk("math_round", json!({"OP": "ROUNDDOWN"}), vec![("NUM", Some(q))])
                    }
                    Op::Mod => {
                        // Puzzlebox's remainder is never negative; Python's takes the divisor's
                        // sign. They agree for positive divisors.
                        if !lit_num(r).is_some_and(|d| d > 0.0) {
                            return None;
                        }
                        self.blk(
                            "math_modulo",
                            json!({}),
                            vec![("DIVIDEND", Some(a.b)), ("DIVISOR", Some(b.b))],
                        )
                    }
                    Op::Pow => {
                        let out = self.arith("POWER", a.b, b.b);
                        let ty = if both_int && lit_int(r).is_some_and(|n| n >= 0) {
                            Ty::Int
                        } else if float {
                            Ty::Float
                        } else {
                            Ty::Num
                        };
                        return self.tr(out, ty);
                    }
                    Op::And | Op::Or | Op::Concat => unreachable!(),
                };
                self.tr(out, ty)
            }
        }
    }

    fn cmp(&mut self, op: Cmp, l: &E, r: &E) -> Option<Value> {
        let name = match op {
            Cmp::Eq => "EQ",
            Cmp::Ne => "NEQ",
            Cmp::Lt => "LT",
            Cmp::Le => "LTE",
            Cmp::Gt => "GT",
            Cmp::Ge => "GTE",
            Cmp::In | Cmp::NotIn => {
                let b = self.membership(l, r)?;
                return Some(if op == Cmp::NotIn { self.negate(b) } else { b });
            }
        };
        // Keep the literal on the right.
        if lit_str(l).is_some() || lit_num(l).is_some() || matches!(l, E::Bool(_)) {
            let flipped = match op {
                Cmp::Lt => Cmp::Gt,
                Cmp::Le => Cmp::Ge,
                Cmp::Gt => Cmp::Lt,
                Cmp::Ge => Cmp::Le,
                other => other,
            };
            if !(lit_str(r).is_some() || lit_num(r).is_some() || matches!(r, E::Bool(_))) {
                return self.cmp(flipped, r, l);
            }
        }
        if let Some(b) = self.cmp_time(op, l, r) {
            return Some(b);
        }
        if let Some(b) = self.cmp_int(op, l, r) {
            return Some(b);
        }
        if let (Some(entity), Some(s)) = (states_call(l), lit_str(r)) {
            // A missing entity is null in Puzzlebox, "unknown" in HA; for (in)equality with
            // other texts both differ, so the plain state block is enough.
            let a = if matches!(op, Cmp::Eq | Cmp::Ne) && s != "unknown" {
                self.state_plain(entity)
            } else {
                self.state_exact(entity)
            };
            let b = self.text(s);
            return Some(if numeric_text(s) { self.strict(name, a, b) } else { self.compare(name, a, b) });
        }
        if matches!(op, Cmp::Eq | Cmp::Ne) && (matches!(l, E::None) || matches!(r, E::None)) {
            let x = if matches!(r, E::None) { l } else { r };
            let is = self.value(&E::IsNone(Box::new(x.clone())))?;
            return Some(if op == Cmp::Ne { self.negate(is.b) } else { is.b });
        }
        let a = self.value(l)?;
        let b = self.value(r)?;
        let eq = matches!(op, Cmp::Eq | Cmp::Ne);
        let ok = if a.ty.numeric() && b.ty.numeric() || a.ty == Ty::Bool && b.ty == Ty::Bool {
            true
        } else if let Some(s) = lit_str(r) {
            // Text against text compares the same unless both look like numbers.
            !numeric_text(s) && (a.ty == Ty::Str || eq && matches!(a.ty, Ty::UStr | Ty::Any))
        } else if matches!(r, E::Bool(_)) {
            eq && !matches!(a.ty, Ty::None)
        } else {
            false
        };
        if ok {
            return Some(self.compare(name, a.b, b.b));
        }
        if eq {
            // An undefined value isn't none in Jinja but is null here.
            if a.ty == Ty::None && maybe_undefined(&b, r) || b.ty == Ty::None && maybe_undefined(&a, l) {
                return None;
            }
        } else if a.ty.never_ordered(b.ty) {
            // Always an error in HA: more likely a mistake to show than a program to run.
            return None;
        }
        Some(self.strict(name, a.b, b.b))
    }

    /// `x in [..]` and `'text' in x`.
    fn membership(&mut self, l: &E, r: &E) -> Option<Value> {
        if let E::List(items) = r {
            let mut acc: Option<Value> = None;
            for it in items {
                let eq = self.cmp(Cmp::Eq, l, it)?;
                acc = Some(match acc {
                    Some(prev) => self.logic("OR", prev, eq),
                    None => eq,
                });
            }
            return Some(match acc {
                Some(b) => b,
                None => self.boolean(false),
            });
        }
        let needle = lit_str(l)?;
        let hay = self.value(r)?;
        if hay.ty != Ty::Str {
            return None;
        }
        let find = self.text(needle);
        Some(self.blk("text_contains", json!({}), vec![("TEXT", Some(hay.b)), ("FIND", Some(find))]))
    }

    /// `now() < today_at('18:00')` → seconds from 18:00 to now < 0.
    fn cmp_time(&mut self, op: Cmp, l: &E, r: &E) -> Option<Value> {
        if !matches!(op, Cmp::Lt | Cmp::Le | Cmp::Gt | Cmp::Ge) {
            return None;
        }
        let at = |e: &E| -> Option<String> {
            let (name, args) = call_name(e)?;
            let t = lit_str(args.pos.first()?)?;
            (name == "today_at" && args.pos.len() == 1 && args.kw.is_empty() && clock(t))
                .then(|| t.to_owned())
        };
        let (t, op) = if is_now(l) {
            (at(r)?, op)
        } else if is_now(r) {
            let flipped = match op {
                Cmp::Lt => Cmp::Gt,
                Cmp::Le => Cmp::Ge,
                Cmp::Gt => Cmp::Lt,
                _ => Cmp::Le,
            };
            (at(l)?, flipped)
        } else {
            return None;
        };
        let today = self.blk("time_today_at", json!({"TIME": t}), vec![]);
        let now = self.blk("time_now", json!({}), vec![]);
        let secs = self.blk("time_seconds_between", json!({}), vec![("A", Some(today)), ("B", Some(now))]);
        let zero = self.number(0.0);
        let name = match op {
            Cmp::Lt => "LT",
            Cmp::Le => "LTE",
            Cmp::Gt => "GT",
            _ => "GTE",
        };
        Some(self.compare(name, secs, zero))
    }

    /// `x | int(d) > n`: Python truncates toward zero, which no block does, but compared with
    /// a whole number the truncation can be folded into the bound (int(x) > 5 ⇔ x ≥ 6).
    fn cmp_int(&mut self, op: Cmp, l: &E, r: &E) -> Option<Value> {
        let (x, d) = match l {
            E::Filter(x, name, args) if name == "int" && only_args(args, 1, &["default"]) => {
                (&**x, arg(args, 0, "default"))
            }
            E::Call(f, args)
                if matches!(&**f, E::Name(n) if n == "int")
                    && only_args(args, 2, &["default"])
                    && !args.pos.is_empty() =>
            {
                (&args.pos[0], arg(args, 1, "default"))
            }
            _ => return None,
        };
        let n = lit_int(r)?;
        // The default comes back as is, so it must be whole like the truncated values.
        if d.is_some_and(|d| lit_int(d).is_none()) {
            return None;
        }
        let (name, bound) = match op {
            Cmp::Gt | Cmp::Ge => {
                let m = if op == Cmp::Gt { n + 1 } else { n };
                if m > 0 { ("GTE", m) } else { ("GT", m - 1) }
            }
            Cmp::Lt | Cmp::Le => {
                let m = if op == Cmp::Lt { n - 1 } else { n };
                if m >= 0 { ("LT", m + 1) } else { ("LTE", m) }
            }
            _ => return None,
        };
        let t = match self.value_if_numeric(x) {
            Some(t) => t,
            None => self.float_with_default(x, d?)?,
        };
        let b = self.number(bound as f64);
        Some(self.compare(name, t.b, b))
    }
}

/// `HH:MM` or `HH:MM:SS`, which HA's today_at and the block read the same way.
fn clock(t: &str) -> bool {
    let parts: Vec<&str> = t.split(':').collect();
    let num = |p: &str, max: u32| p.len() <= 2 && !p.is_empty() && p.parse::<u32>().is_ok_and(|v| v <= max);
    matches!(parts.as_slice(), [h, m] if num(h, 23) && m.len() == 2 && num(m, 59))
        || matches!(parts.as_slice(), [h, m, s] if num(h, 23) && m.len() == 2 && num(m, 59) && s.len() == 2 && num(s, 59))
}
