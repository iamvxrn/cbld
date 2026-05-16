//! Minimal, dependency-free JSON serialization (and, for reading Clang's own
//! JSON output back, parsing) for cbld's own tooling.
//!
//! cbld already declares `serde` for the manifest/lockfile data model, but
//! pulling in `serde_json` just to emit a handful of flat CI-facing payloads
//! would needlessly grow the dependency footprint (see
//! docs/guides/architecture.md). This covers exactly the closed set of
//! shapes cbld needs: objects, arrays, strings, numbers, bools, and null —
//! enough to both write `cbld build --json` / `cbld doctor --json` /
//! `compile_commands.json`, and to read back Clang's `-ftime-trace` output
//! (Chrome Trace Event Format) for `cbld build --trace`.

use std::fmt::Write as _;

/// A JSON value restricted to what cbld's `--json` payloads need.
#[derive(Debug, Clone)]
pub enum Json {
    Null,
    Bool(bool),
    Number(i64),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    pub fn str(s: impl Into<String>) -> Json {
        Json::String(s.into())
    }

    /// Serialize to a compact, single-line JSON string.
    pub fn render(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }

    /// Serialize to an indented, multi-line JSON string (2-space indent) —
    /// used for artifacts meant to be read or diffed by humans, like
    /// `compile_commands.json`. Empty arrays/objects still render compactly
    /// (`[]`/`{}`) rather than as an empty multi-line block.
    pub fn render_pretty(&self) -> String {
        let mut out = String::new();
        self.write_pretty(&mut out, 0);
        out
    }

    fn write_pretty(&self, out: &mut String, indent: usize) {
        match self {
            Json::Array(items) if !items.is_empty() => {
                out.push_str("[\n");
                for (i, item) in items.iter().enumerate() {
                    push_indent(out, indent + 1);
                    item.write_pretty(out, indent + 1);
                    if i + 1 < items.len() {
                        out.push(',');
                    }
                    out.push('\n');
                }
                push_indent(out, indent);
                out.push(']');
            }
            Json::Object(fields) if !fields.is_empty() => {
                out.push_str("{\n");
                for (i, (key, value)) in fields.iter().enumerate() {
                    push_indent(out, indent + 1);
                    out.push('"');
                    escape_into(key, out);
                    out.push_str("\": ");
                    value.write_pretty(out, indent + 1);
                    if i + 1 < fields.len() {
                        out.push(',');
                    }
                    out.push('\n');
                }
                push_indent(out, indent);
                out.push('}');
            }
            other => other.write(out),
        }
    }

    /// Parse a JSON document. Scoped to exactly what `-ftime-trace` output
    /// needs — no comments, no trailing commas, no NaN/Infinity — but
    /// otherwise a complete, standard JSON parser (objects, arrays, strings
    /// with escapes, numbers, bools, null).
    pub fn parse(input: &str) -> std::result::Result<Json, String> {
        let mut parser = Parser {
            bytes: input.as_bytes(),
            pos: 0,
        };
        let value = parser.parse_value()?;
        parser.skip_ws();
        Ok(value)
    }

    /// Look up a field by key on an object; `None` for any other shape or a
    /// missing key.
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Json::Number(n) => Some(*n),
            _ => None,
        }
    }

    fn write(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Number(n) => {
                let _ = write!(out, "{n}");
            }
            Json::String(s) => {
                out.push('"');
                escape_into(s, out);
                out.push('"');
            }
            Json::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write(out);
                }
                out.push(']');
            }
            Json::Object(fields) => {
                out.push('{');
                for (i, (key, value)) in fields.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push('"');
                    escape_into(key, out);
                    out.push_str("\":");
                    value.write(out);
                }
                out.push('}');
            }
        }
    }
}

/// Escape a string's contents for embedding inside a JSON string literal.
fn escape_into(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
}

fn push_indent(out: &mut String, level: usize) {
    for _ in 0..level {
        out.push_str("  ");
    }
}

/// A byte-cursor recursive-descent JSON parser. Kept private: callers only
/// ever see it through `Json::parse`.
struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

type ParseResult<T> = std::result::Result<T, String>;

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let b = self.peek();
        if b.is_some() {
            self.pos += 1;
        }
        b
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn expect(&mut self, byte: u8) -> ParseResult<()> {
        self.skip_ws();
        if self.bump() == Some(byte) {
            Ok(())
        } else {
            Err(format!("expected '{}' at byte {}", byte as char, self.pos))
        }
    }

    fn parse_value(&mut self) -> ParseResult<Json> {
        self.skip_ws();
        match self.peek() {
            Some(b'{') => self.parse_object(),
            Some(b'[') => self.parse_array(),
            Some(b'"') => Ok(Json::String(self.parse_string()?)),
            Some(b't') => self.parse_literal("true", Json::Bool(true)),
            Some(b'f') => self.parse_literal("false", Json::Bool(false)),
            Some(b'n') => self.parse_literal("null", Json::Null),
            Some(b'-') | Some(b'0'..=b'9') => self.parse_number(),
            other => Err(format!("unexpected byte {:?} at {}", other, self.pos)),
        }
    }

    fn parse_literal(&mut self, lit: &str, value: Json) -> ParseResult<Json> {
        if self.bytes[self.pos..].starts_with(lit.as_bytes()) {
            self.pos += lit.len();
            Ok(value)
        } else {
            Err(format!("invalid literal at byte {}", self.pos))
        }
    }

    fn parse_object(&mut self) -> ParseResult<Json> {
        self.expect(b'{')?;
        let mut fields = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Json::Object(fields));
        }
        loop {
            self.skip_ws();
            let key = self.parse_string()?;
            self.expect(b':')?;
            let value = self.parse_value()?;
            fields.push((key, value));
            self.skip_ws();
            match self.bump() {
                Some(b',') => continue,
                Some(b'}') => break,
                other => {
                    return Err(format!(
                        "expected ',' or '}}' at byte {}, got {:?}",
                        self.pos, other
                    ))
                }
            }
        }
        Ok(Json::Object(fields))
    }

    fn parse_array(&mut self) -> ParseResult<Json> {
        self.expect(b'[')?;
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(Json::Array(items));
        }
        loop {
            let value = self.parse_value()?;
            items.push(value);
            self.skip_ws();
            match self.bump() {
                Some(b',') => continue,
                Some(b']') => break,
                other => {
                    return Err(format!(
                        "expected ',' or ']' at byte {}, got {:?}",
                        self.pos, other
                    ))
                }
            }
        }
        Ok(Json::Array(items))
    }

    fn parse_string(&mut self) -> ParseResult<String> {
        self.skip_ws();
        if self.bump() != Some(b'"') {
            return Err(format!("expected string at byte {}", self.pos));
        }
        let mut out = String::new();
        loop {
            match self.bump() {
                None => return Err("unterminated string".to_string()),
                Some(b'"') => break,
                Some(b'\\') => match self.bump() {
                    Some(b'"') => out.push('"'),
                    Some(b'\\') => out.push('\\'),
                    Some(b'/') => out.push('/'),
                    Some(b'n') => out.push('\n'),
                    Some(b't') => out.push('\t'),
                    Some(b'r') => out.push('\r'),
                    Some(b'b') => out.push('\u{8}'),
                    Some(b'f') => out.push('\u{c}'),
                    Some(b'u') => {
                        let cp = self.parse_hex4()?;
                        out.push(char::from_u32(cp).unwrap_or('\u{fffd}'));
                    }
                    other => return Err(format!("invalid escape {:?}", other)),
                },
                Some(b) => {
                    // Copy this byte plus any continuation bytes of a
                    // multi-byte UTF-8 sequence verbatim; structural JSON
                    // bytes ('"', '\\') are always single-byte ASCII, so
                    // scanning for them at the byte level is safe.
                    let start = self.pos - 1;
                    let len = utf8_len(b);
                    let end = (start + len).min(self.bytes.len());
                    self.pos = end;
                    if let Ok(s) = std::str::from_utf8(&self.bytes[start..end]) {
                        out.push_str(s);
                    }
                }
            }
        }
        Ok(out)
    }

    fn parse_hex4(&mut self) -> ParseResult<u32> {
        let mut value = 0u32;
        for _ in 0..4 {
            let b = self.bump().ok_or("unterminated unicode escape")?;
            let digit = (b as char).to_digit(16).ok_or("invalid unicode escape")?;
            value = value * 16 + digit;
        }
        Ok(value)
    }

    fn parse_number(&mut self) -> ParseResult<Json> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.pos += 1;
        }
        if self.peek() == Some(b'.') {
            self.pos += 1;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        let text = std::str::from_utf8(&self.bytes[start..self.pos]).unwrap_or("0");
        let value: f64 = text
            .parse()
            .map_err(|_| format!("invalid number '{text}'"))?;
        Ok(Json::Number(value as i64))
    }
}

fn utf8_len(first_byte: u8) -> usize {
    if first_byte & 0x80 == 0 {
        1
    } else if first_byte & 0xE0 == 0xC0 {
        2
    } else if first_byte & 0xF0 == 0xE0 {
        3
    } else if first_byte & 0xF8 == 0xF0 {
        4
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_scalars() {
        assert_eq!(Json::Null.render(), "null");
        assert_eq!(Json::Bool(true).render(), "true");
        assert_eq!(Json::Bool(false).render(), "false");
        assert_eq!(Json::Number(-42).render(), "-42");
        assert_eq!(Json::str("hi").render(), "\"hi\"");
    }

    #[test]
    fn escapes_special_characters() {
        let rendered = Json::str("line1\nline2\t\"quoted\"\\").render();
        assert_eq!(rendered, "\"line1\\nline2\\t\\\"quoted\\\"\\\\\"");
    }

    #[test]
    fn escapes_control_characters_as_unicode_escapes() {
        let rendered = Json::str("\u{1}bell").render();
        assert_eq!(rendered, "\"\\u0001bell\"");
    }

    #[test]
    fn renders_arrays_and_objects_flat() {
        let value = Json::Object(vec![
            ("name".to_string(), Json::str("widgets")),
            ("count".to_string(), Json::Number(3)),
            (
                "tags".to_string(),
                Json::Array(vec![Json::str("a"), Json::str("b")]),
            ),
            ("fix".to_string(), Json::Null),
        ]);
        assert_eq!(
            value.render(),
            "{\"name\":\"widgets\",\"count\":3,\"tags\":[\"a\",\"b\"],\"fix\":null}"
        );
    }

    #[test]
    fn empty_array_and_object_render_compactly() {
        assert_eq!(Json::Array(Vec::new()).render(), "[]");
        assert_eq!(Json::Object(Vec::new()).render(), "{}");
    }

    #[test]
    fn render_pretty_indents_nested_structures() {
        let value = Json::Object(vec![
            ("name".to_string(), Json::str("widgets")),
            (
                "tags".to_string(),
                Json::Array(vec![Json::str("a"), Json::str("b")]),
            ),
        ]);
        let rendered = value.render_pretty();
        assert_eq!(
            rendered,
            "{\n  \"name\": \"widgets\",\n  \"tags\": [\n    \"a\",\n    \"b\"\n  ]\n}"
        );
    }

    #[test]
    fn parse_round_trips_every_scalar_shape() {
        assert!(matches!(Json::parse("null").unwrap(), Json::Null));
        assert!(matches!(Json::parse("true").unwrap(), Json::Bool(true)));
        assert!(matches!(Json::parse("false").unwrap(), Json::Bool(false)));
        assert_eq!(Json::parse("-42").unwrap().as_i64(), Some(-42));
        assert_eq!(Json::parse("3.0").unwrap().as_i64(), Some(3));
        assert_eq!(
            Json::parse("\"hi\\nthere\"").unwrap().as_str(),
            Some("hi\nthere")
        );
    }

    #[test]
    fn parse_handles_nested_objects_and_arrays_with_whitespace() {
        let input = r#"
        {
          "traceEvents": [
            {"pid": 1, "tid": 0, "ph": "X", "ts": 0, "dur": 150, "name": "Source",
             "args": {"detail": "foo.h"}}
          ]
        }
        "#;
        let doc = Json::parse(input).unwrap();
        let events = doc.get("traceEvents").and_then(Json::as_array).unwrap();
        assert_eq!(events.len(), 1);
        let event = &events[0];
        assert_eq!(event.get("name").and_then(Json::as_str), Some("Source"));
        assert_eq!(event.get("dur").and_then(Json::as_i64), Some(150));
        assert_eq!(
            event
                .get("args")
                .and_then(|a| a.get("detail"))
                .and_then(Json::as_str),
            Some("foo.h")
        );
    }

    #[test]
    fn parse_rejects_malformed_input_without_panicking() {
        assert!(Json::parse("{").is_err());
        assert!(Json::parse("[1, 2").is_err());
        assert!(Json::parse("not json").is_err());
    }

    #[test]
    fn parse_then_render_is_stable_for_a_representative_document() {
        let original = Json::Object(vec![
            ("a".to_string(), Json::Number(1)),
            (
                "b".to_string(),
                Json::Array(vec![Json::str("x"), Json::Null]),
            ),
        ]);
        let rendered = original.render();
        let reparsed = Json::parse(&rendered).unwrap();
        assert_eq!(reparsed.render(), rendered);
    }
}
