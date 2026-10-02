//! Reads the Lua tables KOReader writes (its `dump.lua`): book settings,
//! the Pencil plugin's strokes. A small parser for that subset (`return`,
//! tables with `["key"] =` and `[n] =` keys, strings, numbers, booleans,
//! `nil`, `--` comments); nothing is ever evaluated.

use std::fmt;

/// A parsed value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Nil,
    Bool(bool),
    Number(f64),
    String(String),
    Table(Table),
}

/// A table's entries in file order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Table(pub Vec<(Key, Value)>);

#[derive(Debug, Clone, PartialEq)]
pub enum Key {
    Name(String),
    Index(i64),
}

impl Value {
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.table()?.get(key)
    }

    pub fn str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn num(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(*n),
            _ => None,
        }
    }

    pub fn table(&self) -> Option<&Table> {
        match self {
            Value::Table(t) => Some(t),
            _ => None,
        }
    }

    /// A string field, `None` when missing or not a string.
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(Value::str)
    }

    pub fn get_num(&self, key: &str) -> Option<f64> {
        self.get(key).and_then(Value::num)
    }
}

impl Table {
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0.iter().find_map(|(k, v)| match k {
            Key::Name(n) if n == key => Some(v),
            _ => None,
        })
    }

    /// The array part, `[1]`, `[2]`, … in index order (KOReader writes
    /// them in any order).
    pub fn array(&self) -> Vec<&Value> {
        let mut items: Vec<(i64, &Value)> = self
            .0
            .iter()
            .filter_map(|(k, v)| match k {
                Key::Index(i) => Some((*i, v)),
                Key::Name(_) => None,
            })
            .collect();
        items.sort_by_key(|(i, _)| *i);
        items.into_iter().map(|(_, v)| v).collect()
    }
}

#[derive(Debug)]
pub struct ParseError {
    pub at: usize,
    pub what: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at byte {}", self.what, self.at)
    }
}

impl std::error::Error for ParseError {}

/// Parses a file of the form `return <value>`.
pub fn parse(source: &str) -> Result<Value, ParseError> {
    let mut p = Parser {
        s: source.as_bytes(),
        i: 0,
    };
    p.skip();
    if !p.eat_word(b"return") {
        return Err(p.error("expected `return`"));
    }
    let value = p.value(0)?;
    p.skip();
    if p.i < p.s.len() {
        return Err(p.error("unexpected text after the value"));
    }
    Ok(value)
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

/// Deeper than any KOReader file; stops a hostile one blowing the stack.
const MAX_DEPTH: usize = 64;

impl Parser<'_> {
    fn error(&self, what: &str) -> ParseError {
        ParseError {
            at: self.i,
            what: what.to_owned(),
        }
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    /// Skips whitespace and `--` comments.
    fn skip(&mut self) {
        loop {
            while self.peek().is_some_and(|c| c.is_ascii_whitespace()) {
                self.i += 1;
            }
            if self.s[self.i..].starts_with(b"--") {
                while self.peek().is_some_and(|c| c != b'\n') {
                    self.i += 1;
                }
            } else {
                return;
            }
        }
    }

    fn eat(&mut self, c: u8) -> bool {
        self.skip();
        if self.peek() == Some(c) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn eat_word(&mut self, word: &[u8]) -> bool {
        let rest = &self.s[self.i..];
        let ends = rest
            .get(word.len())
            .is_none_or(|c| !c.is_ascii_alphanumeric() && *c != b'_');
        if rest.starts_with(word) && ends {
            self.i += word.len();
            true
        } else {
            false
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value, ParseError> {
        self.skip();
        match self.peek() {
            Some(b'{') => self.table(depth),
            Some(b'"' | b'\'') => self.string().map(Value::String),
            Some(c) if c == b'-' || c == b'.' || c.is_ascii_digit() => self.number(),
            _ if self.eat_word(b"true") => Ok(Value::Bool(true)),
            _ if self.eat_word(b"false") => Ok(Value::Bool(false)),
            _ if self.eat_word(b"nil") => Ok(Value::Nil),
            _ => Err(self.error("expected a value")),
        }
    }

    fn table(&mut self, depth: usize) -> Result<Value, ParseError> {
        if depth >= MAX_DEPTH {
            return Err(self.error("tables nested too deeply"));
        }
        self.i += 1; // {
        let mut entries = Vec::new();
        let mut next_index = 1;
        loop {
            if self.eat(b'}') {
                return Ok(Value::Table(Table(entries)));
            }
            let key = if self.eat(b'[') {
                let key = match self.value(depth + 1)? {
                    Value::String(s) => Key::Name(s),
                    Value::Number(n) if n.fract() == 0.0 => Key::Index(n as i64),
                    _ => return Err(self.error("unsupported key")),
                };
                if !self.eat(b']') || !self.eat(b'=') {
                    return Err(self.error("expected `] =`"));
                }
                key
            } else if let Some(name) = self.name_key() {
                Key::Name(name)
            } else {
                let k = Key::Index(next_index);
                next_index += 1;
                k
            };
            let value = self.value(depth + 1)?;
            entries.push((key, value));
            if !(self.eat(b',') || self.eat(b';')) {
                if self.eat(b'}') {
                    return Ok(Value::Table(Table(entries)));
                }
                return Err(self.error("expected `,` or `}`"));
            }
        }
    }

    /// A bare `name =` key, if one comes next (`true`, a value, isn't one).
    fn name_key(&mut self) -> Option<String> {
        let start = self.i;
        while self
            .peek()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_')
        {
            self.i += 1;
        }
        let name = &self.s[start..self.i];
        if !name.is_empty() && !name[0].is_ascii_digit() && self.eat(b'=') {
            return Some(String::from_utf8_lossy(name).into_owned());
        }
        self.i = start;
        None
    }

    fn number(&mut self) -> Result<Value, ParseError> {
        let start = self.i;
        if self.peek() == Some(b'-') {
            self.i += 1;
        }
        while self
            .peek()
            .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'+' | b'-'))
        {
            // A sign only after an exponent.
            if matches!(self.peek(), Some(b'+' | b'-'))
                && !matches!(self.s[self.i - 1], b'e' | b'E')
            {
                break;
            }
            self.i += 1;
        }
        let text = std::str::from_utf8(&self.s[start..self.i]).unwrap_or("");
        let parsed = match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
            Some(hex) => i64::from_str_radix(hex, 16).ok().map(|n| n as f64),
            None => text.parse::<f64>().ok(),
        };
        parsed
            .map(Value::Number)
            .ok_or_else(|| self.error("bad number"))
    }

    /// A quoted string with Lua's escapes (`%q` writes a newline as a
    /// backslash before a real one, and other control bytes as `\ddd`).
    fn string(&mut self) -> Result<String, ParseError> {
        let quote = self.s[self.i];
        self.i += 1;
        let mut out = Vec::new();
        loop {
            let Some(c) = self.peek() else {
                return Err(self.error("unterminated string"));
            };
            self.i += 1;
            if c == quote {
                break;
            }
            if c != b'\\' {
                out.push(c);
                continue;
            }
            let Some(e) = self.peek() else {
                return Err(self.error("unterminated string"));
            };
            self.i += 1;
            match e {
                b'n' | b'\n' => out.push(b'\n'),
                b'r' => out.push(b'\r'),
                b't' => out.push(b'\t'),
                b'a' => out.push(7),
                b'b' => out.push(8),
                b'f' => out.push(12),
                b'v' => out.push(11),
                b'\r' => {
                    out.push(b'\n');
                    if self.peek() == Some(b'\n') {
                        self.i += 1;
                    }
                }
                b'x' => {
                    let hex = self.s.get(self.i..self.i + 2).unwrap_or_default();
                    let byte = std::str::from_utf8(hex)
                        .ok()
                        .and_then(|h| u8::from_str_radix(h, 16).ok())
                        .ok_or_else(|| self.error("bad \\x escape"))?;
                    self.i += 2;
                    out.push(byte);
                }
                b'z' => {
                    while self.peek().is_some_and(|c| c.is_ascii_whitespace()) {
                        self.i += 1;
                    }
                }
                b'u' => {
                    let close = self.s[self.i..]
                        .iter()
                        .position(|&c| c == b'}')
                        .ok_or_else(|| self.error("bad \\u escape"))?;
                    let code = std::str::from_utf8(&self.s[self.i + 1..self.i + close])
                        .ok()
                        .and_then(|h| u32::from_str_radix(h, 16).ok())
                        .and_then(char::from_u32)
                        .ok_or_else(|| self.error("bad \\u escape"))?;
                    self.i += close + 1;
                    let mut buf = [0; 4];
                    out.extend_from_slice(code.encode_utf8(&mut buf).as_bytes());
                }
                d if d.is_ascii_digit() => {
                    let mut n = u32::from(d - b'0');
                    for _ in 0..2 {
                        match self.peek() {
                            Some(d) if d.is_ascii_digit() => {
                                n = n * 10 + u32::from(d - b'0');
                                self.i += 1;
                            }
                            _ => break,
                        }
                    }
                    out.push(u8::try_from(n).map_err(|_| self.error("bad \\ddd escape"))?);
                }
                other => out.push(other), // \\ \" \'
            }
        }
        Ok(String::from_utf8_lossy(&out).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_koreader_settings_file() {
        let src = "-- /mnt/onboard/b.kepub.sdr/metadata.epub.lua\nreturn {\n    [\"annotations\"] = {\n        [2] = {\n            [\"text\"] = \"second\",\n        },\n        [1] = {\n            [\"note\"] = \"woah\\\n\",\n            [\"pageno\"] = 25,\n            [\"text\"] = \"White \\\"Christ\\\"\",\n        },\n    },\n    [\"percent_finished\"] = 0.050656660412758,\n    [\"hyphenation\"] = true,\n    [\"font_family_fonts\"] = {},\n}\n";
        let v = parse(src).unwrap();
        let notes = v.get("annotations").unwrap().table().unwrap().array();
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].get_str("note"), Some("woah\n"));
        assert_eq!(notes[0].get_str("text"), Some("White \"Christ\""));
        assert_eq!(notes[0].get_num("pageno"), Some(25.0));
        assert_eq!(notes[1].get_str("text"), Some("second"));
        assert_eq!(v.get_num("percent_finished"), Some(0.050656660412758));
        assert_eq!(v.get("hyphenation"), Some(&Value::Bool(true)));
        assert!(
            v.get("font_family_fonts")
                .unwrap()
                .table()
                .unwrap()
                .0
                .is_empty()
        );
    }

    #[test]
    fn reads_escapes_and_plain_lua() {
        let v = parse(
            r#"return { "a\116b\xe2\x80\x99", 'q\u{e9}', name = -1.5e-3, true, [5] = nil; }"#,
        )
        .unwrap();
        let t = v.table().unwrap();
        assert_eq!(t.array()[0].str(), Some("atb\u{2019}"));
        assert_eq!(t.array()[1].str(), Some("qé"));
        assert_eq!(t.array()[2], &Value::Bool(true));
        assert_eq!(v.get_num("name"), Some(-0.0015));
    }

    #[test]
    fn refuses_what_isnt_data() {
        assert!(parse("os.execute('rm -rf /')").is_err());
        assert!(parse("return { a = f() }").is_err());
        assert!(parse("return \"open").is_err());
        assert!(parse(&format!("return {}", "{".repeat(100))).is_err());
    }
}
