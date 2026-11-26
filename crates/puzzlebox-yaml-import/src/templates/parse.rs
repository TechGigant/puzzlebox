//! A small parser for the Jinja expressions Home Assistant templates use. Anything it doesn't
//! understand (statements, comments, unusual literals) is an error, so the caller keeps the
//! template as it is.

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum E {
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    None,
    List(Vec<E>),
    Name(String),
    Attr(Box<E>, String),
    Call(Box<E>, Args),
    Filter(Box<E>, String, Args),
    Neg(Box<E>),
    Pos(Box<E>),
    Not(Box<E>),
    Bin(Op, Box<E>, Box<E>),
    Cmp(Cmp, Box<E>, Box<E>),
    /// `then if cond else otherwise`.
    Cond {
        cond: Box<E>,
        then: Box<E>,
        otherwise: Option<Box<E>>,
    },
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Args {
    pub pos: Vec<E>,
    pub kw: Vec<(String, E)>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Op {
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv,
    Mod,
    Pow,
    Concat,
    And,
    Or,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Cmp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    In,
    NotIn,
}

/// A template split into literal text and `{{ … }}` expressions.
#[derive(Debug, PartialEq)]
pub(crate) enum Part {
    Text(String),
    Expr(E),
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Name(String),
    Str(String),
    Int(i64),
    Float(f64),
    Op(&'static str),
}

const OPS: [&str; 26] = [
    "**", "//", "==", "!=", "<=", ">=", "(", ")", "[", "]", "{", "}", ",", ".", ":", "|", "~", "+", "-", "*",
    "/", "%", "<", ">", "=", "!",
];

/// Splits a template into parts; `None` for statements (`{% %}`), comments or syntax errors.
pub(crate) fn parse_template(src: &str) -> Option<Vec<Part>> {
    if src.contains("{%") || src.contains("{#") {
        return None;
    }
    let mut parts = Vec::new();
    let mut rest = src;
    let mut strip_next = false;
    while let Some(start) = rest.find("{{") {
        let mut text = &rest[..start];
        if strip_next {
            text = text.trim_start();
        }
        let mut after = &rest[start + 2..];
        if let Some(a) = after.strip_prefix('-') {
            text = text.trim_end();
            after = a;
        }
        if !text.is_empty() {
            parts.push(Part::Text(text.to_owned()));
        }
        let (toks, used, strip) = lex(after)?;
        let mut p = Parser { toks, pos: 0 };
        let e = p.expr()?;
        if p.pos != p.toks.len() {
            return None;
        }
        parts.push(Part::Expr(e));
        strip_next = strip;
        rest = &after[used..];
    }
    let text = if strip_next { rest.trim_start() } else { rest };
    if !text.is_empty() {
        parts.push(Part::Text(text.to_owned()));
    }
    Some(parts)
}

/// Tokens up to the closing `}}`, the bytes consumed and whether it was `-}}`.
fn lex(s: &str) -> Option<(Vec<Tok>, usize, bool)> {
    let b = s.as_bytes();
    let mut i = 0;
    let mut toks = Vec::new();
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if s[i..].starts_with("-}}") {
            return Some((toks, i + 3, true));
        }
        if s[i..].starts_with("}}") {
            return Some((toks, i + 2, false));
        }
        if c == b'\'' || c == b'"' {
            let (lit, len) = string(&s[i..])?;
            // Adjacent string literals are joined, as in Python.
            if let Some(Tok::Str(prev)) = toks.last_mut() {
                prev.push_str(&lit);
            } else {
                toks.push(Tok::Str(lit));
            }
            i += len;
            continue;
        }
        if c.is_ascii_digit() {
            let start = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            let mut float = false;
            if i + 1 < b.len() && b[i] == b'.' && b[i + 1].is_ascii_digit() {
                float = true;
                i += 1;
                while i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
            }
            if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
                let mut j = i + 1;
                if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
                    j += 1;
                }
                if j < b.len() && b[j].is_ascii_digit() {
                    float = true;
                    i = j;
                    while i < b.len() && b[i].is_ascii_digit() {
                        i += 1;
                    }
                }
            }
            if i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                return None;
            }
            let text = &s[start..i];
            toks.push(if float { Tok::Float(text.parse().ok()?) } else { Tok::Int(text.parse().ok()?) });
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            toks.push(Tok::Name(s[start..i].to_owned()));
            continue;
        }
        let op = OPS.iter().find(|o| s[i..].starts_with(**o))?;
        toks.push(Tok::Op(op));
        i += op.len();
    }
    None
}

/// A quoted literal: only plain escapes are accepted (others depend on Python's decoder).
fn string(s: &str) -> Option<(String, usize)> {
    let quote = s.chars().next()?;
    let mut out = String::new();
    let mut chars = s.char_indices().skip(1);
    while let Some((i, c)) = chars.next() {
        match c {
            '\\' => match chars.next()?.1 {
                '\\' => out.push('\\'),
                '\'' => out.push('\''),
                '"' => out.push('"'),
                'n' => out.push('\n'),
                't' => out.push('\t'),
                _ => return None,
            },
            c if c == quote => return Some((out, i + 1)),
            c => out.push(c),
        }
    }
    None
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn is_op(&self, op: &str) -> bool {
        matches!(self.peek(), Some(Tok::Op(o)) if *o == op)
    }

    fn is_name(&self, name: &str) -> bool {
        matches!(self.peek(), Some(Tok::Name(n)) if n == name)
    }

    fn eat_op(&mut self, op: &str) -> bool {
        let hit = self.is_op(op);
        if hit {
            self.pos += 1;
        }
        hit
    }

    fn eat_name(&mut self, name: &str) -> bool {
        let hit = self.is_name(name);
        if hit {
            self.pos += 1;
        }
        hit
    }

    fn expect_op(&mut self, op: &str) -> Option<()> {
        self.eat_op(op).then_some(())
    }

    fn expr(&mut self) -> Option<E> {
        let then = self.or()?;
        if self.eat_name("if") {
            let cond = self.or()?;
            let otherwise = if self.eat_name("else") { Some(Box::new(self.expr()?)) } else { None };
            return Some(E::Cond { cond: Box::new(cond), then: Box::new(then), otherwise });
        }
        Some(then)
    }

    fn or(&mut self) -> Option<E> {
        let mut l = self.and()?;
        while self.eat_name("or") {
            l = E::Bin(Op::Or, Box::new(l), Box::new(self.and()?));
        }
        Some(l)
    }

    fn and(&mut self) -> Option<E> {
        let mut l = self.not()?;
        while self.eat_name("and") {
            l = E::Bin(Op::And, Box::new(l), Box::new(self.not()?));
        }
        Some(l)
    }

    fn not(&mut self) -> Option<E> {
        if self.eat_name("not") {
            return Some(E::Not(Box::new(self.not()?)));
        }
        self.compare()
    }

    fn compare(&mut self) -> Option<E> {
        let l = self.math1()?;
        let op = if self.eat_op("==") {
            Cmp::Eq
        } else if self.eat_op("!=") {
            Cmp::Ne
        } else if self.eat_op("<=") {
            Cmp::Le
        } else if self.eat_op(">=") {
            Cmp::Ge
        } else if self.eat_op("<") {
            Cmp::Lt
        } else if self.eat_op(">") {
            Cmp::Gt
        } else if self.eat_name("in") {
            Cmp::In
        } else if self.is_name("not")
            && matches!(self.toks.get(self.pos + 1), Some(Tok::Name(n)) if n == "in")
        {
            self.pos += 2;
            Cmp::NotIn
        } else {
            return Some(l);
        };
        let r = self.math1()?;
        // Chained comparisons (`a < b < c`) aren't supported.
        if matches!(self.peek(), Some(Tok::Op("==" | "!=" | "<" | "<=" | ">" | ">="))) || self.is_name("in") {
            return None;
        }
        Some(E::Cmp(op, Box::new(l), Box::new(r)))
    }

    fn math1(&mut self) -> Option<E> {
        let mut l = self.concat()?;
        loop {
            let op = if self.eat_op("+") {
                Op::Add
            } else if self.eat_op("-") {
                Op::Sub
            } else {
                return Some(l);
            };
            l = E::Bin(op, Box::new(l), Box::new(self.concat()?));
        }
    }

    fn concat(&mut self) -> Option<E> {
        let mut l = self.math2()?;
        while self.eat_op("~") {
            l = E::Bin(Op::Concat, Box::new(l), Box::new(self.math2()?));
        }
        Some(l)
    }

    fn math2(&mut self) -> Option<E> {
        let mut l = self.pow()?;
        loop {
            let op = if self.eat_op("*") {
                Op::Mul
            } else if self.eat_op("//") {
                Op::FloorDiv
            } else if self.eat_op("/") {
                Op::Div
            } else if self.eat_op("%") {
                Op::Mod
            } else {
                return Some(l);
            };
            l = E::Bin(op, Box::new(l), Box::new(self.pow()?));
        }
    }

    fn pow(&mut self) -> Option<E> {
        let mut l = self.unary(true)?;
        while self.eat_op("**") {
            l = E::Bin(Op::Pow, Box::new(l), Box::new(self.unary(true)?));
        }
        Some(l)
    }

    /// Like Jinja: `-x | f` is `(-x) | f`, and filters bind tighter than any operator.
    fn unary(&mut self, with_filter: bool) -> Option<E> {
        let mut node = if self.eat_op("-") {
            E::Neg(Box::new(self.unary(false)?))
        } else if self.eat_op("+") {
            E::Pos(Box::new(self.unary(false)?))
        } else {
            self.primary()?
        };
        node = self.postfix(node)?;
        if with_filter {
            node = self.filters(node)?;
        }
        Some(node)
    }

    fn primary(&mut self) -> Option<E> {
        let tok = self.peek()?.clone();
        self.pos += 1;
        Some(match tok {
            Tok::Str(s) => E::Str(s),
            Tok::Int(n) => E::Int(n),
            Tok::Float(f) => E::Float(f),
            Tok::Name(n) => match n.as_str() {
                "true" | "True" => E::Bool(true),
                "false" | "False" => E::Bool(false),
                "none" | "None" => E::None,
                "and" | "or" | "not" | "in" | "is" | "if" | "else" => return None,
                _ => E::Name(n),
            },
            Tok::Op("(") => {
                let e = self.expr()?;
                self.expect_op(")")?;
                e
            }
            Tok::Op("[") => {
                let mut items = Vec::new();
                while !self.eat_op("]") {
                    if !items.is_empty() {
                        self.expect_op(",")?;
                        if self.eat_op("]") {
                            break;
                        }
                    }
                    items.push(self.expr()?);
                }
                E::List(items)
            }
            _ => return None,
        })
    }

    fn postfix(&mut self, mut node: E) -> Option<E> {
        loop {
            if self.eat_op(".") {
                let Some(Tok::Name(n)) = self.peek().cloned() else { return None };
                self.pos += 1;
                node = E::Attr(Box::new(node), n);
            } else if self.eat_op("[") {
                // Only `x['key']`, which reads the same as `x.key` for dictionaries.
                let Some(Tok::Str(key)) = self.peek().cloned() else { return None };
                self.pos += 1;
                self.expect_op("]")?;
                node = E::Attr(Box::new(node), key);
            } else if self.is_op("(") {
                self.pos += 1;
                node = E::Call(Box::new(node), self.args()?);
            } else {
                return Some(node);
            }
        }
    }

    fn filters(&mut self, mut node: E) -> Option<E> {
        loop {
            if self.eat_op("|") {
                let Some(Tok::Name(name)) = self.peek().cloned() else { return None };
                self.pos += 1;
                let args = if self.eat_op("(") { self.args()? } else { Args::default() };
                node = E::Filter(Box::new(node), name, args);
            } else if self.is_name("is") {
                // Tests (`is defined`, `is number`…) aren't translated.
                return None;
            } else {
                return Some(node);
            }
        }
    }

    /// Call arguments after the opening parenthesis.
    fn args(&mut self) -> Option<Args> {
        let mut args = Args::default();
        while !self.eat_op(")") {
            if !args.pos.is_empty() || !args.kw.is_empty() {
                self.expect_op(",")?;
                if self.eat_op(")") {
                    break;
                }
            }
            if let (Some(Tok::Name(n)), Some(Tok::Op("="))) =
                (self.peek().cloned(), self.toks.get(self.pos + 1).cloned())
            {
                self.pos += 2;
                args.kw.push((n, self.expr()?));
            } else if args.kw.is_empty() {
                args.pos.push(self.expr()?);
            } else {
                return None;
            }
        }
        Some(args)
    }
}
