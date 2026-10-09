//! Reading and writing [`Value`]s as JSON, the text form of scenes.
//!
//! JSON because everything can read it: an editor, a script, a plugin in another language.
//! An entity reference is written as `{"$entity": n}` so it can be told from a number.

use std::fmt::Write;

use super::value::Value;

const ENTITY_KEY: &str = "$entity";

/// Writes a value as JSON, indented for reading and for clean diffs.
pub fn to_string(value: &Value) -> String {
    let mut out = String::new();
    write_value(&mut out, value, 0);
    out.push('\n');
    out
}

fn indent(out: &mut String, depth: usize) {
    out.push('\n');
    for _ in 0..depth {
        out.push_str("  ");
    }
}

/// Short lists of plain values (a vector, a colour) stay on one line.
fn is_short(items: &[Value]) -> bool {
    items.len() <= 16
        && items.iter().all(|v| {
            matches!(
                v,
                Value::Null | Value::Bool(_) | Value::Int(_) | Value::Float(_)
            )
        })
}

fn write_value(out: &mut String, value: &Value, depth: usize) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Int(i) => write!(out, "{i}").unwrap(),
        // JSON has no way to write these; `null` at least says "no number here".
        Value::Float(f) if !f.is_finite() => out.push_str("null"),
        // `{:?}` gives the shortest text that reads back as exactly this number, and always
        // has a `.` or an exponent, so it reads back as a float and not an integer.
        Value::Float(f) => write!(out, "{f:?}").unwrap(),
        Value::Text(text) => write_text(out, text),
        Value::Entity(bits) => write!(out, "{{\"{ENTITY_KEY}\": {bits}}}").unwrap(),
        Value::List(items) if items.is_empty() => out.push_str("[]"),
        Value::List(items) if is_short(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_value(out, item, depth);
            }
            out.push(']');
        }
        Value::List(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                indent(out, depth + 1);
                write_value(out, item, depth + 1);
            }
            indent(out, depth);
            out.push(']');
        }
        Value::Map(fields) if fields.is_empty() => out.push_str("{}"),
        Value::Map(fields) => {
            out.push('{');
            for (i, (key, field)) in fields.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                indent(out, depth + 1);
                write_text(out, key);
                out.push_str(": ");
                write_value(out, field, depth + 1);
            }
            indent(out, depth);
            out.push('}');
        }
    }
}

fn write_text(out: &mut String, text: &str) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => write!(out, "\\u{:04x}", c as u32).unwrap(),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Where and why JSON couldn't be read.
#[derive(Clone, Debug, PartialEq)]
pub struct ParseError {
    pub line: usize,
    pub column: usize,
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "line {}, column {}: {}",
            self.line, self.column, self.message
        )
    }
}

impl std::error::Error for ParseError {}

/// Reads JSON into a value.
pub fn parse(text: &str) -> Result<Value, ParseError> {
    let mut parser = Parser {
        text,
        bytes: text.as_bytes(),
        at: 0,
        depth: 0,
    };
    let value = parser.value()?;
    parser.space();
    if parser.at != parser.bytes.len() {
        return Err(parser.error("unexpected text after the value"));
    }
    Ok(value)
}

struct Parser<'a> {
    text: &'a str,
    bytes: &'a [u8],
    at: usize,
    depth: usize,
}

/// Deeper nesting than any real scene has; a limit keeps a hostile file from overflowing the
/// stack.
const MAX_DEPTH: usize = 128;

impl Parser<'_> {
    fn error(&self, message: &str) -> ParseError {
        let before = &self.text[..self.at.min(self.text.len())];
        ParseError {
            line: before.matches('\n').count() + 1,
            column: before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1,
            message: message.to_owned(),
        }
    }

    fn space(&mut self) {
        while matches!(self.bytes.get(self.at), Some(b' ' | b'\n' | b'\r' | b'\t')) {
            self.at += 1;
        }
    }

    fn eat(&mut self, byte: u8) -> bool {
        self.space();
        if self.bytes.get(self.at) == Some(&byte) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn word(&mut self, word: &str, value: Value) -> Result<Value, ParseError> {
        if self.text[self.at..].starts_with(word) {
            self.at += word.len();
            Ok(value)
        } else {
            Err(self.error("expected a value"))
        }
    }

    fn value(&mut self) -> Result<Value, ParseError> {
        self.space();
        match self.bytes.get(self.at) {
            None => Err(self.error("expected a value, found the end")),
            Some(b'n') => self.word("null", Value::Null),
            Some(b't') => self.word("true", Value::Bool(true)),
            Some(b'f') => self.word("false", Value::Bool(false)),
            Some(b'"') => self.string().map(Value::Text),
            Some(b'[') => self.nested(Self::list),
            Some(b'{') => self.nested(Self::map),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => Err(self.error("expected a value")),
        }
    }

    fn nested(
        &mut self,
        parse: fn(&mut Self) -> Result<Value, ParseError>,
    ) -> Result<Value, ParseError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(self.error("nested too deeply"));
        }
        let value = parse(self);
        self.depth -= 1;
        value
    }

    fn list(&mut self) -> Result<Value, ParseError> {
        self.at += 1;
        let mut items = Vec::new();
        if self.eat(b']') {
            return Ok(Value::List(items));
        }
        loop {
            items.push(self.value()?);
            if self.eat(b']') {
                return Ok(Value::List(items));
            }
            if !self.eat(b',') {
                return Err(self.error("expected `,` or `]`"));
            }
        }
    }

    fn map(&mut self) -> Result<Value, ParseError> {
        self.at += 1;
        let mut fields = Vec::new();
        if self.eat(b'}') {
            return Ok(Value::Map(fields));
        }
        loop {
            self.space();
            if self.bytes.get(self.at) != Some(&b'"') {
                return Err(self.error("expected a field name in quotes"));
            }
            let key = self.string()?;
            if !self.eat(b':') {
                return Err(self.error("expected `:` after the field name"));
            }
            fields.push((key, self.value()?));
            if self.eat(b'}') {
                break;
            }
            if !self.eat(b',') {
                return Err(self.error("expected `,` or `}`"));
            }
        }
        // `{"$entity": n}` is how an entity reference is written.
        if let [(key, Value::Int(bits))] = fields.as_slice() {
            if key == ENTITY_KEY && *bits >= 0 {
                return Ok(Value::Entity(*bits as u64));
            }
        }
        Ok(Value::Map(fields))
    }

    fn number(&mut self) -> Result<Value, ParseError> {
        let start = self.at;
        let mut float = false;
        while let Some(byte) = self.bytes.get(self.at) {
            match byte {
                b'0'..=b'9' | b'-' | b'+' => {}
                b'.' | b'e' | b'E' => float = true,
                _ => break,
            }
            self.at += 1;
        }
        let text = &self.text[start..self.at];
        let value = if float {
            text.parse().ok().map(Value::Float)
        } else {
            // An integer too big for 64 bits is still a number; keep it, approximately.
            text.parse()
                .ok()
                .map(Value::Int)
                .or_else(|| text.parse().ok().map(Value::Float))
        };
        value.ok_or_else(|| {
            self.at = start;
            self.error("not a number")
        })
    }

    fn string(&mut self) -> Result<String, ParseError> {
        self.at += 1;
        let mut out = String::new();
        loop {
            let rest = &self.text[self.at..];
            let Some(c) = rest.chars().next() else {
                return Err(self.error("text is missing its closing quote"));
            };
            self.at += c.len_utf8();
            match c {
                '"' => return Ok(out),
                '\\' => {
                    let Some(escape) = self.text[self.at..].chars().next() else {
                        return Err(self.error("text is missing its closing quote"));
                    };
                    self.at += escape.len_utf8();
                    match escape {
                        '"' => out.push('"'),
                        '\\' => out.push('\\'),
                        '/' => out.push('/'),
                        'n' => out.push('\n'),
                        'r' => out.push('\r'),
                        't' => out.push('\t'),
                        'b' => out.push('\u{8}'),
                        'f' => out.push('\u{c}'),
                        'u' => {
                            let first = self.hex4()?;
                            // Characters outside the basic plane come as two escapes.
                            let code = if (0xd800..0xdc00).contains(&first)
                                && self.text[self.at..].starts_with("\\u")
                            {
                                self.at += 2;
                                let second = self.hex4()?;
                                0x10000 + ((first - 0xd800) << 10) + second.wrapping_sub(0xdc00)
                            } else {
                                first
                            };
                            out.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                        }
                        _ => return Err(self.error("unknown escape")),
                    }
                }
                c => out.push(c),
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, ParseError> {
        let digits = self
            .text
            .get(self.at..self.at + 4)
            .and_then(|hex| u32::from_str_radix(hex, 16).ok())
            .ok_or_else(|| self.error("expected four hex digits"))?;
        self.at += 4;
        Ok(digits)
    }
}
