//! The JSON of the language server protocol: a value, a parser and a writer. The parser also
//! reads JSON with comments, as project configuration files are written.

use std::fmt::Write as _;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Json {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Json>),
    /// In source order.
    Object(Vec<(String, Json)>),
}

/// Deeper input is rejected rather than parsed with unbounded recursion.
const MAX_DEPTH: usize = 512;

impl Json {
    pub(crate) fn object<const N: usize>(entries: [(&str, Json); N]) -> Json {
        Json::Object(entries.into_iter().map(|(key, value)| (key.to_owned(), value)).collect())
    }

    pub(crate) fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(entries) => entries.iter().find(|(name, _)| name == key).map(|(_, value)| value),
            _ => None,
        }
    }

    pub(crate) fn path(&self, keys: &[&str]) -> Option<&Json> {
        keys.iter().try_fold(self, |value, key| value.get(key))
    }

    pub(crate) fn as_str(&self) -> Option<&str> {
        match self {
            Json::String(text) => Some(text),
            _ => None,
        }
    }

    /// A non-negative integer.
    pub(crate) fn as_u64(&self) -> Option<u64> {
        match *self {
            Json::Number(number) if number >= 0.0 && number.fract() == 0.0 && number < 9_007_199_254_740_992.0 => Some(number as u64),
            _ => None,
        }
    }

    pub(crate) fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Array(items) => Some(items),
            _ => None,
        }
    }

    pub(crate) fn parse(text: &str) -> Result<Json, String> {
        Self::parse_with(text, false)
    }

    /// JSON with comments (`//` and `/* */`) and trailing commas, as VS Code's settings and
    /// LuaLS's `.luarc.json` are written.
    pub(crate) fn parse_with_comments(text: &str) -> Result<Json, String> {
        Self::parse_with(text, true)
    }

    fn parse_with(text: &str, comments: bool) -> Result<Json, String> {
        let mut parser = Parser { bytes: text.as_bytes(), at: 0, comments };
        let value = parser.value(0)?;
        parser.whitespace();
        if parser.at != parser.bytes.len() {
            return Err(format!("unexpected data at byte {}", parser.at));
        }
        Ok(value)
    }

    pub(crate) fn write(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
            Json::Number(number) => {
                if !number.is_finite() {
                    out.push_str("null");
                } else if number.fract() == 0.0 && number.abs() < 9_007_199_254_740_992.0 {
                    let _ = write!(out, "{}", *number as i64);
                } else {
                    let _ = write!(out, "{number}");
                }
            }
            Json::String(text) => write_string(text, out),
            Json::Array(items) => {
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 { out.push(','); }
                    item.write(out);
                }
                out.push(']');
            }
            Json::Object(entries) => {
                out.push('{');
                for (index, (key, value)) in entries.iter().enumerate() {
                    if index > 0 { out.push(','); }
                    write_string(key, out);
                    out.push(':');
                    value.write(out);
                }
                out.push('}');
            }
        }
    }
}

impl std::fmt::Display for Json {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut out = String::new();
        self.write(&mut out);
        formatter.write_str(&out)
    }
}

impl From<&str> for Json {
    fn from(text: &str) -> Json { Json::String(text.to_owned()) }
}
impl From<String> for Json {
    fn from(text: String) -> Json { Json::String(text) }
}
impl From<bool> for Json {
    fn from(value: bool) -> Json { Json::Bool(value) }
}
impl From<usize> for Json {
    fn from(value: usize) -> Json { Json::Number(value as f64) }
}
impl From<i64> for Json {
    fn from(value: i64) -> Json { Json::Number(value as f64) }
}

fn write_string(text: &str, out: &mut String) {
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            character if (character as u32) < 0x20 => { let _ = write!(out, "\\u{:04x}", character as u32); }
            character => out.push(character),
        }
    }
    out.push('"');
}

struct Parser<'a> {
    bytes: &'a [u8],
    at: usize,
    /// Comments are whitespace, and a comma may end an array or object.
    comments: bool,
}

impl Parser<'_> {
    fn whitespace(&mut self) {
        loop {
            while matches!(self.bytes.get(self.at), Some(b' ' | b'\t' | b'\n' | b'\r')) {
                self.at += 1;
            }
            if !self.comments || self.bytes.get(self.at) != Some(&b'/') {
                return;
            }
            match self.bytes.get(self.at + 1) {
                Some(b'/') => {
                    while self.bytes.get(self.at).is_some_and(|byte| *byte != b'\n') {
                        self.at += 1;
                    }
                }
                Some(b'*') => {
                    let end = self.bytes[self.at + 2..].windows(2).position(|pair| pair == b"*/");
                    self.at = end.map_or(self.bytes.len(), |end| self.at + 2 + end + 2);
                }
                _ => return,
            }
        }
    }

    /// Whether a trailing comma's array or object ends here.
    fn closes(&mut self, close: u8) -> bool {
        self.whitespace();
        if self.comments && self.bytes.get(self.at) == Some(&close) {
            self.at += 1;
            return true;
        }
        false
    }

    fn error<T>(&self, expected: &str) -> Result<T, String> {
        Err(format!("expected {expected} at byte {}", self.at))
    }

    fn literal(&mut self, text: &str, value: Json) -> Result<Json, String> {
        if self.bytes[self.at..].starts_with(text.as_bytes()) {
            self.at += text.len();
            Ok(value)
        } else {
            self.error(text)
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, String> {
        if depth > MAX_DEPTH {
            return Err("JSON nesting is too deep".into());
        }
        self.whitespace();
        match self.bytes.get(self.at) {
            Some(b'n') => self.literal("null", Json::Null),
            Some(b't') => self.literal("true", Json::Bool(true)),
            Some(b'f') => self.literal("false", Json::Bool(false)),
            Some(b'"') => self.string().map(Json::String),
            Some(b'[') => {
                self.at += 1;
                let mut items = Vec::new();
                self.whitespace();
                if self.bytes.get(self.at) == Some(&b']') {
                    self.at += 1;
                    return Ok(Json::Array(items));
                }
                loop {
                    items.push(self.value(depth + 1)?);
                    self.whitespace();
                    match self.bytes.get(self.at) {
                        Some(b',') => {
                            self.at += 1;
                            if self.closes(b']') { return Ok(Json::Array(items)); }
                        }
                        Some(b']') => { self.at += 1; return Ok(Json::Array(items)); }
                        _ => return self.error("',' or ']'"),
                    }
                }
            }
            Some(b'{') => {
                self.at += 1;
                let mut entries = Vec::new();
                self.whitespace();
                if self.bytes.get(self.at) == Some(&b'}') {
                    self.at += 1;
                    return Ok(Json::Object(entries));
                }
                loop {
                    self.whitespace();
                    if self.bytes.get(self.at) != Some(&b'"') {
                        return self.error("an object key");
                    }
                    let key = self.string()?;
                    self.whitespace();
                    if self.bytes.get(self.at) != Some(&b':') {
                        return self.error("':'");
                    }
                    self.at += 1;
                    entries.push((key, self.value(depth + 1)?));
                    self.whitespace();
                    match self.bytes.get(self.at) {
                        Some(b',') => {
                            self.at += 1;
                            if self.closes(b'}') { return Ok(Json::Object(entries)); }
                        }
                        Some(b'}') => { self.at += 1; return Ok(Json::Object(entries)); }
                        _ => return self.error("',' or '}'"),
                    }
                }
            }
            Some(b'-' | b'0'..=b'9') => self.number(),
            _ => self.error("a JSON value"),
        }
    }

    fn number(&mut self) -> Result<Json, String> {
        let start = self.at;
        while matches!(self.bytes.get(self.at), Some(b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')) {
            self.at += 1;
        }
        std::str::from_utf8(&self.bytes[start..self.at])
            .ok()
            .and_then(|text| text.parse::<f64>().ok())
            .map(Json::Number)
            .map_or_else(|| self.error("a number"), Ok)
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let digits = self.bytes.get(self.at..self.at + 4).ok_or_else(|| "truncated \\u escape".to_string())?;
        let text = std::str::from_utf8(digits).map_err(|_| "invalid \\u escape".to_string())?;
        let value = u32::from_str_radix(text, 16).map_err(|_| "invalid \\u escape".to_string())?;
        self.at += 4;
        Ok(value)
    }

    fn string(&mut self) -> Result<String, String> {
        self.at += 1;
        let mut text = Vec::new();
        loop {
            match self.bytes.get(self.at) {
                None => return self.error("'\"'"),
                Some(b'"') => {
                    self.at += 1;
                    return String::from_utf8(text).map_err(|_| "invalid UTF-8 in a string".into());
                }
                Some(b'\\') => {
                    self.at += 1;
                    let escape = *self.bytes.get(self.at).ok_or_else(|| "truncated escape".to_string())?;
                    self.at += 1;
                    match escape {
                        b'"' => text.push(b'"'),
                        b'\\' => text.push(b'\\'),
                        b'/' => text.push(b'/'),
                        b'b' => text.push(0x08),
                        b'f' => text.push(0x0c),
                        b'n' => text.push(b'\n'),
                        b'r' => text.push(b'\r'),
                        b't' => text.push(b'\t'),
                        b'u' => {
                            let mut code = self.hex4()?;
                            if (0xd800..0xdc00).contains(&code) && self.bytes[self.at..].starts_with(b"\\u") {
                                self.at += 2;
                                let low = self.hex4()?;
                                if (0xdc00..0xe000).contains(&low) {
                                    code = 0x10000 + ((code - 0xd800) << 10) + (low - 0xdc00);
                                }
                            }
                            let character = char::from_u32(code).unwrap_or('\u{fffd}');
                            let mut buffer = [0; 4];
                            text.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
                        }
                        _ => return self.error("an escape"),
                    }
                }
                Some(&byte) => {
                    text.push(byte);
                    self.at += 1;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Json;

    #[test]
    fn values_round_trip_with_escapes_and_nesting() {
        let text = r#"{"a":[1,2.5,-3,true,false,null],"b":"line\n\"quoted\" é 😀","c":{}}"#;
        let value = Json::parse(text).unwrap();
        assert_eq!(value.path(&["a"]).and_then(Json::as_array).map(<[Json]>::len), Some(6));
        assert_eq!(value.get("b").and_then(Json::as_str), Some("line\n\"quoted\" é 😀"));
        assert_eq!(Json::parse(&value.to_string()).unwrap(), value);
    }

    #[test]
    fn malformed_and_too_deep_input_is_rejected() {
        assert!(Json::parse("{\"a\":}").is_err());
        assert!(Json::parse("[1,]").is_err());
        assert!(Json::parse("[1 // one\n]").is_err());
        assert_eq!(
            Json::parse_with_comments("// settings\n{ /* a */ \"a\": [1, 2,], // b\n \"b\": \"//\", }").unwrap(),
            Json::object([("a", Json::Array(vec![Json::Number(1.0), Json::Number(2.0)])), ("b", Json::from("//"))]),
        );
        assert!(Json::parse_with_comments("[,]").is_err());
        assert!(Json::parse_with_comments("{\"a\": 1 /* open").is_err());
        assert!(Json::parse(&"[".repeat(10_000)).is_err());
    }
}
