//! JSON, spelled the reference's way.
//!
//! The wire's stanzas must diff byte-identical against the Python
//! implementation's, and Python's json module has a particular
//! hand: object keys keep insertion order, the compact separators
//! put no spaces anywhere, and ensure_ascii escapes everything
//! past ASCII as \uXXXX. No crate speaks that dialect off the
//! shelf, so this one is hand-rolled -- the wire's whole need is
//! modest, and the parity sweeps are the point.
//!
//! The one representational departure: a JSON object is a vector
//! of pairs rather than a hash map, which is exactly what
//! insertion order costs.

use crate::errors::VoxamError;

/// One JSON value, integers and floats told apart as Python
/// tells them.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<Value>),
    Object(Object),
}

impl Value {
    /// The string inside, or None for any other shape.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(text) => Some(text),
            _ => None,
        }
    }

    /// The integer inside, or None for any other shape.
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(number) => Some(*number),
            _ => None,
        }
    }

    /// The number inside as a float, integers included.
    pub fn as_float(&self) -> Option<f64> {
        match self {
            Value::Int(number) => Some(*number as f64),
            Value::Float(number) => Some(*number),
            _ => None,
        }
    }

    /// The object inside, or None for any other shape.
    pub fn as_object(&self) -> Option<&Object> {
        match self {
            Value::Object(object) => Some(object),
            _ => None,
        }
    }

    /// The list inside, or None for any other shape.
    pub fn as_list(&self) -> Option<&[Value]> {
        match self {
            Value::List(list) => Some(list),
            _ => None,
        }
    }

    /// Whether this is the JSON true.
    pub fn is_true(&self) -> bool {
        matches!(self, Value::Bool(true))
    }
}

impl From<bool> for Value {
    fn from(told: bool) -> Self {
        Value::Bool(told)
    }
}

impl From<i64> for Value {
    fn from(told: i64) -> Self {
        Value::Int(told)
    }
}

impl From<f64> for Value {
    fn from(told: f64) -> Self {
        Value::Float(told)
    }
}

impl From<&str> for Value {
    fn from(told: &str) -> Self {
        Value::Str(told.to_string())
    }
}

impl From<String> for Value {
    fn from(told: String) -> Self {
        Value::Str(told)
    }
}

impl From<Object> for Value {
    fn from(told: Object) -> Self {
        Value::Object(told)
    }
}

impl From<Vec<Value>> for Value {
    fn from(told: Vec<Value>) -> Self {
        Value::List(told)
    }
}

/// A JSON object with its keys in insertion order, the way
/// Python's dicts keep them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Object(Vec<(String, Value)>);

impl Object {
    pub fn new() -> Self {
        Self(Vec::new())
    }

    /// The value behind a key.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0
            .iter()
            .find(|(held, _)| held == key)
            .map(|(_, value)| value)
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        self.0
            .iter_mut()
            .find(|(held, _)| held == key)
            .map(|(_, value)| value)
    }

    pub fn contains(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    /// Set a key: an existing one keeps its seat, a new one joins
    /// at the end -- dict assignment's own manners.
    pub fn set(&mut self, key: &str, value: impl Into<Value>) {
        let value = value.into();

        match self.0.iter_mut().find(|(held, _)| held == key) {
            Some(seat) => seat.1 = value,
            None => self.0.push((key.to_string(), value)),
        }
    }

    /// Remove a key, telling what it held.
    pub fn remove(&mut self, key: &str) -> Option<Value> {
        let at = self.0.iter().position(|(held, _)| held == key)?;

        Some(self.0.remove(at).1)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.0.iter().map(|(key, value)| (key.as_str(), value))
    }

    /// The object with one key set aside, for comparing.
    pub fn without(&self, key: &str) -> Object {
        Object(
            self.0
                .iter()
                .filter(|(held, _)| held != key)
                .cloned()
                .collect(),
        )
    }
}

impl<const N: usize> From<[(&str, Value); N]> for Object {
    fn from(pairs: [(&str, Value); N]) -> Self {
        Object(
            pairs
                .into_iter()
                .map(|(key, value)| (key.to_string(), value))
                .collect(),
        )
    }
}

/// One value as compact JSON, the reference's exact spelling:
/// insertion-ordered keys, no separator spaces, everything past
/// ASCII escaped.
pub fn dumps(value: &Value) -> String {
    let mut told = String::new();

    write_value(&mut told, value);

    told
}

fn write_value(told: &mut String, value: &Value) {
    match value {
        Value::Null => told.push_str("null"),
        Value::Bool(true) => told.push_str("true"),
        Value::Bool(false) => told.push_str("false"),
        Value::Int(number) => told.push_str(&number.to_string()),
        Value::Float(number) => write_float(told, *number),
        Value::Str(text) => write_string(told, text),
        Value::List(list) => {
            told.push('[');

            for (at, item) in list.iter().enumerate() {
                if at > 0 {
                    told.push(',');
                }

                write_value(told, item);
            }

            told.push(']');
        }
        Value::Object(object) => {
            told.push('{');

            for (at, (key, item)) in object.0.iter().enumerate() {
                if at > 0 {
                    told.push(',');
                }

                write_string(told, key);
                told.push(':');
                write_value(told, item);
            }

            told.push('}');
        }
    }
}

/// A float in Python repr's shortest spelling; the exponent forms
/// gain Python's plus sign and two-digit floor.
fn write_float(told: &mut String, number: f64) {
    if number.is_nan() || number.is_infinite() {
        // Python would write Infinity/NaN; the wire never carries
        // either, so refuse quietly with null.
        told.push_str("null");

        return;
    }

    let shortest = format!("{number:?}");

    if let Some(at) = shortest.find(['e', 'E']) {
        let (mantissa, exponent) = shortest.split_at(at);
        let exponent = &exponent[1..];
        let (sign, digits) = match exponent.strip_prefix('-') {
            Some(digits) => ("-", digits),
            None => ("+", exponent),
        };

        told.push_str(mantissa);
        told.push('e');
        told.push_str(sign);

        if digits.len() < 2 {
            told.push('0');
        }

        told.push_str(digits);

        return;
    }

    told.push_str(&shortest);
}

fn write_string(told: &mut String, text: &str) {
    told.push('"');

    for piece in text.chars() {
        match piece {
            '"' => told.push_str("\\\""),
            '\\' => told.push_str("\\\\"),
            '\n' => told.push_str("\\n"),
            '\r' => told.push_str("\\r"),
            '\t' => told.push_str("\\t"),
            '\u{8}' => told.push_str("\\b"),
            '\u{c}' => told.push_str("\\f"),
            piece if (piece as u32) < 0x20 => {
                told.push_str(&format!("\\u{:04x}", piece as u32));
            }
            piece if piece.is_ascii() => told.push(piece),
            piece => {
                // ensure_ascii: astral characters travel as their
                // surrogate pair, the rest as one \uXXXX.
                let mut units = [0u16; 2];

                for unit in piece.encode_utf16(&mut units) {
                    told.push_str(&format!("\\u{unit:04x}"));
                }
            }
        }
    }

    told.push('"');
}

fn json_error(message: String) -> VoxamError {
    VoxamError::GlkOte(message)
}

/// Parse one JSON document; trailing content is refused.
pub fn loads(text: &str) -> Result<Value, VoxamError> {
    let mut parser = Parser {
        bytes: text.as_bytes(),
        at: 0,
    };

    parser.skip_whitespace();

    let value = parser.value()?;

    parser.skip_whitespace();

    if parser.at != parser.bytes.len() {
        return Err(json_error("trailing content after the document".into()));
    }

    Ok(value)
}

struct Parser<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Parser<'_> {
    fn skip_whitespace(&mut self) {
        while self.at < self.bytes.len()
            && matches!(self.bytes[self.at], b' ' | b'\t' | b'\n' | b'\r')
        {
            self.at += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn expect(&mut self, wanted: u8) -> Result<(), VoxamError> {
        if self.peek() == Some(wanted) {
            self.at += 1;

            Ok(())
        } else {
            Err(json_error(format!(
                "expected {:?} at byte {}",
                wanted as char, self.at
            )))
        }
    }

    fn literal(&mut self, word: &str, value: Value) -> Result<Value, VoxamError> {
        if self.bytes[self.at..].starts_with(word.as_bytes()) {
            self.at += word.len();

            Ok(value)
        } else {
            Err(json_error(format!("expected {word} at byte {}", self.at)))
        }
    }

    fn value(&mut self) -> Result<Value, VoxamError> {
        match self.peek() {
            Some(b'{') => self.object(),
            Some(b'[') => self.list(),
            Some(b'"') => Ok(Value::Str(self.string()?)),
            Some(b't') => self.literal("true", Value::Bool(true)),
            Some(b'f') => self.literal("false", Value::Bool(false)),
            Some(b'n') => self.literal("null", Value::Null),
            Some(byte) if byte == b'-' || byte.is_ascii_digit() => self.number(),
            _ => Err(json_error(format!("expected a value at byte {}", self.at))),
        }
    }

    fn object(&mut self) -> Result<Value, VoxamError> {
        self.expect(b'{')?;
        self.skip_whitespace();

        let mut object = Object::new();

        if self.peek() == Some(b'}') {
            self.at += 1;

            return Ok(Value::Object(object));
        }

        loop {
            self.skip_whitespace();

            let key = self.string()?;

            self.skip_whitespace();
            self.expect(b':')?;
            self.skip_whitespace();

            let value = self.value()?;

            // Later spellings of a key overwrite, as Python's
            // parser keeps only the last.
            object.set(&key, value);
            self.skip_whitespace();

            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b'}') => {
                    self.at += 1;

                    return Ok(Value::Object(object));
                }
                _ => {
                    return Err(json_error(format!(
                        "expected ',' or '}}' at byte {}",
                        self.at
                    )));
                }
            }
        }
    }

    fn list(&mut self) -> Result<Value, VoxamError> {
        self.expect(b'[')?;
        self.skip_whitespace();

        let mut list = Vec::new();

        if self.peek() == Some(b']') {
            self.at += 1;

            return Ok(Value::List(list));
        }

        loop {
            self.skip_whitespace();
            list.push(self.value()?);
            self.skip_whitespace();

            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b']') => {
                    self.at += 1;

                    return Ok(Value::List(list));
                }
                _ => {
                    return Err(json_error(format!(
                        "expected ',' or ']' at byte {}",
                        self.at
                    )));
                }
            }
        }
    }

    fn string(&mut self) -> Result<String, VoxamError> {
        self.expect(b'"')?;

        let mut told = String::new();
        let mut pending: Option<u16> = None;

        loop {
            let Some(byte) = self.peek() else {
                return Err(json_error("unterminated string".into()));
            };

            if byte == b'"' {
                self.at += 1;

                if let Some(lone) = pending.take() {
                    told.push_str(&lone_surrogate(lone));
                }

                return Ok(told);
            }

            if byte == b'\\' {
                self.at += 1;

                let Some(escape) = self.peek() else {
                    return Err(json_error("unterminated escape".into()));
                };

                self.at += 1;

                if escape == b'u' {
                    let unit = self.unit()?;

                    match pending.take() {
                        Some(high) if (0xDC00..0xE000).contains(&unit) => {
                            let paired = 0x10000
                                + ((u32::from(high) - 0xD800) << 10)
                                + (u32::from(unit) - 0xDC00);

                            told.push(char::from_u32(paired).expect("a paired surrogate"));
                        }
                        held => {
                            if let Some(lone) = held {
                                told.push_str(&lone_surrogate(lone));
                            }

                            if (0xD800..0xDC00).contains(&unit) {
                                pending = Some(unit);
                            } else {
                                told.push_str(&lone_surrogate(unit));
                            }
                        }
                    }

                    continue;
                }

                if let Some(lone) = pending.take() {
                    told.push_str(&lone_surrogate(lone));
                }

                match escape {
                    b'"' => told.push('"'),
                    b'\\' => told.push('\\'),
                    b'/' => told.push('/'),
                    b'b' => told.push('\u{8}'),
                    b'f' => told.push('\u{c}'),
                    b'n' => told.push('\n'),
                    b'r' => told.push('\r'),
                    b't' => told.push('\t'),
                    _ => {
                        return Err(json_error(format!(
                            "unknown escape at byte {}",
                            self.at - 1
                        )));
                    }
                }

                continue;
            }

            if let Some(lone) = pending.take() {
                told.push_str(&lone_surrogate(lone));
            }

            // A multi-byte character rides through whole; the
            // input is a &str, so the bytes are valid UTF-8.
            let rest = std::str::from_utf8(&self.bytes[self.at..])
                .map_err(|_| json_error("invalid UTF-8".into()))?;
            let piece = rest.chars().next().expect("checked non-empty");

            told.push(piece);
            self.at += piece.len_utf8();
        }
    }

    fn unit(&mut self) -> Result<u16, VoxamError> {
        if self.at + 4 > self.bytes.len() {
            return Err(json_error("truncated \\u escape".into()));
        }

        let hex = std::str::from_utf8(&self.bytes[self.at..self.at + 4])
            .map_err(|_| json_error("invalid \\u escape".into()))?;
        let unit = u16::from_str_radix(hex, 16)
            .map_err(|_| json_error(format!("invalid \\u escape at byte {}", self.at)))?;

        self.at += 4;

        Ok(unit)
    }

    fn number(&mut self) -> Result<Value, VoxamError> {
        let start = self.at;

        if self.peek() == Some(b'-') {
            self.at += 1;
        }

        while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
            self.at += 1;
        }

        let mut floated = false;

        if self.peek() == Some(b'.') {
            floated = true;
            self.at += 1;

            while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
                self.at += 1;
            }
        }

        if matches!(self.peek(), Some(b'e' | b'E')) {
            floated = true;
            self.at += 1;

            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.at += 1;
            }

            while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
                self.at += 1;
            }
        }

        let text = std::str::from_utf8(&self.bytes[start..self.at]).expect("digits are ASCII");

        if floated {
            text.parse::<f64>()
                .map(Value::Float)
                .map_err(|_| json_error(format!("invalid number at byte {start}")))
        } else {
            match text.parse::<i64>() {
                Ok(number) => Ok(Value::Int(number)),
                // Past i64, Python would keep an exact big int;
                // the wire never carries one, so a float stands in.
                Err(_) => text
                    .parse::<f64>()
                    .map(Value::Float)
                    .map_err(|_| json_error(format!("invalid number at byte {start}"))),
            }
        }
    }
}

/// A lone UTF-16 surrogate cannot live in a Rust string; the
/// replacement character stands in, where Python would keep the
/// unpaired unit. The wire never carries one.
fn lone_surrogate(unit: u16) -> String {
    char::from_u32(u32::from(unit))
        .map(|piece| piece.to_string())
        .unwrap_or_else(|| '\u{fffd}'.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // The compact spelling matches Python's json.dumps with the
    // wire's separators: insertion-ordered keys, no spaces, and
    // ensure_ascii escapes.
    #[test]
    fn dumps_speaks_pythons_compact_dialect() {
        let mut inner = Object::new();

        inner.set("b", 2i64);
        inner.set("a", 1i64);

        let mut object = Object::new();

        object.set("type", "update");
        object.set("gen", 1i64);
        object.set("held", Value::List(vec![Value::Null, Value::Bool(true)]));
        object.set("nested", inner);
        object.set("text", "å → \"quoted\"\n");
        object.set("astral", "\u{1f600}");
        object.set("ratio", 0.5f64);

        assert_eq!(
            dumps(&Value::Object(object)),
            "{\"type\":\"update\",\"gen\":1,\"held\":[null,true],\
             \"nested\":{\"b\":2,\"a\":1},\
             \"text\":\"\\u00e5 \\u2192 \\\"quoted\\\"\\n\",\
             \"astral\":\"\\ud83d\\ude00\",\"ratio\":0.5}"
        );
    }

    // Floats keep Python repr's shortest spelling, exponent signs
    // and two-digit floors included.
    #[test]
    fn floats_wear_pythons_repr() {
        assert_eq!(dumps(&Value::Float(640.0)), "640.0");
        assert_eq!(dumps(&Value::Float(0.1)), "0.1");
        assert_eq!(dumps(&Value::Float(1e16)), "1e+16");
        assert_eq!(dumps(&Value::Float(1e-5)), "1e-05");
        assert_eq!(dumps(&Value::Float(-2.5e-7)), "-2.5e-07");
    }

    // The parser reads back what dumps writes, integers and
    // floats told apart, escapes and surrogate pairs unwound.
    #[test]
    fn loads_round_trips() {
        let text = "{\"gen\":3,\"box\":[0,30,640.5,400],\
                    \"text\":\"\\u00e5 \\ud83d\\ude00 \\\"q\\\"\",\
                    \"deep\":{\"on\":true,\"off\":null}}";
        let value = loads(text).unwrap();
        let object = value.as_object().unwrap();

        assert_eq!(object.get("gen"), Some(&Value::Int(3)));
        assert_eq!(
            object.get("box").unwrap().as_list().unwrap()[2],
            Value::Float(640.5)
        );
        assert_eq!(
            object.get("text").unwrap().as_str(),
            Some("\u{e5} \u{1f600} \"q\"")
        );
        assert_eq!(dumps(&loads(&dumps(&value)).unwrap()), dumps(&value));
    }

    // What is not JSON is refused: bare words, trailing content,
    // and torn strings.
    #[test]
    fn the_parser_refuses_what_is_not_json() {
        assert!(loads("porthole").is_err());
        assert!(loads("{}extra").is_err());
        assert!(loads("\"unterminated").is_err());
        assert!(loads("{\"a\":}").is_err());
        assert!(loads("[1,]").is_err());
    }

    // Object keys keep insertion order, setting an existing key
    // keeps its seat, and without() sets one aside for comparing.
    #[test]
    fn objects_keep_their_order() {
        let mut object = Object::new();

        object.set("z", 1i64);
        object.set("a", 2i64);
        object.set("z", 3i64);

        let keys: Vec<&str> = object.iter().map(|(key, _)| key).collect();

        assert_eq!(keys, vec!["z", "a"]);
        assert_eq!(object.get("z"), Some(&Value::Int(3)));
        assert_eq!(object.without("z").get("z"), None);
        assert_eq!(object.without("z").get("a"), Some(&Value::Int(2)));
    }
}
