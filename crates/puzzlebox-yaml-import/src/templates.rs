//! Jinja templates → blocks where they map exactly (otherwise a template block HA renders).

mod parse;
#[cfg(test)]
mod tests;
mod translate;

use serde_json::{Value, json};

use crate::*;
use parse::{E, Part, parse_template};
use translate::Tx;

/// Python's `str.strip()` whitespace (which also counts the ASCII separators 0x1c–0x1f).
fn py_space(c: char) -> bool {
    c.is_whitespace() || ('\x1c'..='\x1f').contains(&c)
}

fn word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// HA turns a rendered value that reads as a Python literal (`5`, `True`, `[1]`…) into that
/// value. True if `s` certainly isn't one, so it stays this text: it starts with a name
/// that isn't True/False/None and isn't a string prefix like `b'…'`.
fn starts_plain(s: &str, whole: bool) -> bool {
    let Some(first) = s.chars().next() else { return whole };
    if !first.is_alphabetic() {
        return false;
    }
    let word: String =
        s.chars().take_while(|c| word_char(*c) || (!c.is_ascii() && c.is_alphanumeric())).collect();
    let after = s[word.len()..].chars().next();
    if ["True", "False", "None"].contains(&word.as_str()) || matches!(after, Some('\'' | '"')) {
        return false;
    }
    // The word must end inside this text, or a value printed after it could extend it.
    whole || after.is_some()
}

/// True if a rendered text ending with `s` (a literal suffix) can't be a Python literal: a
/// trailing name, a dangling operator or a character Python doesn't accept outside strings.
fn ends_plain(s: &str) -> bool {
    if s.is_empty() || s.contains(['\'', '"']) {
        return false;
    }
    let invalid = |c: char| {
        matches!(c, '$' | '?' | '`')
            || (!c.is_ascii()
                && !c.is_alphanumeric()
                && !py_space(c)
                && !matches!(
                    c,
                    '\u{b7}' | '\u{387}' | '\u{1369}'
                        ..='\u{1371}' | '\u{19da}' | '\u{2118}' | '\u{212e}' | '\u{309b}' | '\u{309c}'
                ))
    };
    if s.chars().any(invalid)
        || s.ends_with(['%', '+', '-', '*', '/', '&', '|', '^', '<', '>', '=', '!', '~', '@'])
    {
        return true;
    }
    let word: String =
        s.chars().rev().take_while(|c| word_char(*c)).collect::<Vec<_>>().into_iter().rev().collect();
    let before = s[..s.len() - word.len()].chars().next_back();
    !word.is_empty()
        && !word.starts_with(|c: char| c.is_ascii_digit())
        && !["True", "False", "None"].contains(&word.as_str())
        && before.is_some_and(py_space)
}

/// The expression if the template is a single `{{ … }}` (whitespace around it is stripped).
fn single(parts: &[Part]) -> Option<&E> {
    let mut exprs = parts.iter().filter_map(|p| match p {
        Part::Expr(e) => Some(e),
        Part::Text(_) => None,
    });
    let e = exprs.next()?;
    let only_space = parts
        .iter()
        .all(|p| matches!(p, Part::Expr(_)) || matches!(p, Part::Text(t) if t.chars().all(py_space)));
    (exprs.next().is_none() && only_space).then_some(e)
}

/// Literal texts a single-expression template can produce stay texts after HA's parsing.
fn literals_stay_text(e: &E) -> bool {
    match e {
        E::Str(s) => s.is_empty() || starts_plain(s, true) && s.trim_matches(py_space) == s,
        E::Cond { then, otherwise, .. } => {
            literals_stay_text(then) && otherwise.as_deref().is_none_or(literals_stay_text)
        }
        E::Call(f, args) if matches!(&**f, E::Name(n) if n == "iif") => {
            args.pos.iter().skip(1).all(literals_stay_text)
        }
        _ => true,
    }
}

impl Builder {
    /// A value block for a template string.
    ///
    /// Native blocks when every part translates exactly, else an `ha_template` block. One
    /// deliberate approximation: HA parses a rendered value that looks like a number (or
    /// `True`, a list…) into that type, while the blocks keep entity states, trigger details
    /// and variables as they are, e.g. the text "21.5"; Puzzlebox treats such text as the
    /// number everywhere, and HA's actions accept either.
    pub(crate) fn template_value(&mut self, s: &str) -> Value {
        let saved = self.next_id;
        match self.translate_value(s) {
            Some(b) => b,
            None => {
                self.next_id = saved;
                self.block("ha_template", json!({"TEMPLATE": s}), vec![], None)
            }
        }
    }

    /// A true/false block equivalent to a template condition (`condition: template`, or the
    /// `"{{ … }}"` shorthand), or `None` when no blocks give exactly the same result, in which
    /// case the caller keeps the raw template condition.
    ///
    /// HA passes a template condition only when the rendered text is "true" (any case), so
    /// this translates templates that are one expression of certainly boolean type
    /// (comparisons, `is_state(…)`, `and`/`or`/`not` of those…). Templates that would raise
    /// an error in HA either fail the same way as blocks or aren't translated.
    pub(crate) fn template_condition(&mut self, s: &str) -> Option<Value> {
        let saved = self.next_id;
        let out = parse_template(s).and_then(|parts| {
            let e = single(&parts)?;
            Tx { b: self }.boolean_expr(e)
        });
        if out.is_none() {
            self.next_id = saved;
        }
        out
    }

    fn translate_value(&mut self, s: &str) -> Option<Value> {
        let parts = parse_template(s)?;
        if let Some(e) = single(&parts) {
            if !literals_stay_text(e) {
                return None;
            }
            return Tx { b: self }.value(e).map(|t| t.b);
        }
        // Text with values in it: HA strips the result and keeps it as text, unless it reads
        // as a Python literal.
        let n = parts.len();
        let first_ok = matches!(parts.first(), Some(Part::Text(t)) if starts_plain(t.trim_start_matches(py_space), false));
        let last_ok = matches!(parts.last(), Some(Part::Text(t)) if ends_plain(t.trim_end_matches(py_space)));
        if !first_ok && !last_ok {
            return None;
        }
        let mut tx = Tx { b: self };
        let mut items = Vec::new();
        for (i, p) in parts.iter().enumerate() {
            match p {
                Part::Text(t) => {
                    let mut t = t.as_str();
                    if i == 0 {
                        t = t.trim_start_matches(py_space);
                    }
                    if i == n - 1 {
                        t = t.trim_end_matches(py_space);
                    }
                    if !t.is_empty() {
                        items.push(tx.text_block(t));
                    }
                }
                Part::Expr(e) => items.extend(tx.text_of(e)?),
            }
        }
        Some(tx.text_join(items))
    }
}

impl Tx<'_> {
    fn text_block(&mut self, s: &str) -> Value {
        self.b.block("text", json!({"TEXT": s}), vec![], None)
    }
}
