use crate::{Comment, CommentKind, Diagnostic, Span};

#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TokenKind {
    Identifier(String),
    Keyword(String),
    Number(String),
    String(Vec<u8>),
    Symbol(String),
    Comment(Comment),
    Newline,
    Eof,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Lexed {
    pub tokens: Vec<Token>,
    pub diagnostics: Vec<Diagnostic>,
}

pub fn lex(source: &str) -> Lexed {
    let mut lexer = Lexer {
        source,
        pos: 0,
        tokens: Vec::new(),
        diagnostics: Vec::new(),
    };
    lexer.run();
    Lexed {
        tokens: lexer.tokens,
        diagnostics: lexer.diagnostics,
    }
}

struct Lexer<'a> {
    source: &'a str,
    pos: usize,
    tokens: Vec<Token>,
    diagnostics: Vec<Diagnostic>,
}

impl Lexer<'_> {
    fn run(&mut self) {
        while self.pos < self.source.len() {
            let start = self.pos;
            let ch = self.char();
            if ch == '\r' || ch == '\n' {
                self.newline();
                self.push(start, TokenKind::Newline);
            } else if ch.is_ascii_whitespace() {
                self.advance();
            } else if self.at("--") {
                self.comment(start);
            } else if start == 0 && self.at("#!") {
                self.line_end();
                self.push_comment(start, CommentKind::Shebang);
            } else if ch == '\'' || ch == '"' {
                let value = self.short_string(ch, start);
                self.push(start, TokenKind::String(value));
            } else if let Some((equals, body)) = self.long_open(start) {
                let value = self.long_content(equals, body, start, false);
                self.push(start, TokenKind::String(value));
            } else if ch.is_ascii_digit()
                || (ch == '.' && self.peek_char(1).is_some_and(|c| c.is_ascii_digit()))
            {
                self.number(start);
            } else if identifier_start(ch) {
                self.advance();
                while self.pos < self.source.len() && identifier_continue(self.char()) {
                    self.advance();
                }
                let spelling = self.source[start..self.pos].to_owned();
                let kind = if keyword(&spelling) {
                    TokenKind::Keyword(spelling)
                } else {
                    TokenKind::Identifier(spelling)
                };
                self.push(start, kind);
            } else {
                let symbol = ["...", "..", "==", "~=", "<=", ">=", "<<", ">>", "//", "::"]
                    .into_iter()
                    .find(|symbol| self.at(symbol));
                if let Some(symbol) = symbol {
                    self.pos += symbol.len();
                    self.push(start, TokenKind::Symbol(symbol.to_owned()));
                } else if "=(){}[],;:.+-*/%^#<>|&~?@".contains(ch) {
                    self.advance();
                    self.push(start, TokenKind::Symbol(ch.to_string()));
                } else {
                    self.advance();
                    self.error(start, format!("unexpected character {ch:?}"));
                    self.push(start, TokenKind::Symbol(ch.to_string()));
                }
            }
        }
        self.tokens.push(Token {
            kind: TokenKind::Eof,
            span: Span::new(self.pos, self.pos),
        });
    }

    fn char(&self) -> char {
        self.source[self.pos..].chars().next().unwrap()
    }
    fn peek_char(&self, offset: usize) -> Option<char> {
        self.source[self.pos..].chars().nth(offset)
    }
    fn at(&self, value: &str) -> bool {
        self.source[self.pos..].starts_with(value)
    }
    fn advance(&mut self) {
        self.pos += self.char().len_utf8();
    }
    fn newline(&mut self) {
        if self.at("\r\n") {
            self.pos += 2;
        } else {
            self.pos += 1;
        }
    }
    fn line_end(&mut self) {
        while self.pos < self.source.len() && !matches!(self.char(), '\r' | '\n') {
            self.advance();
        }
    }
    fn push(&mut self, start: usize, kind: TokenKind) {
        self.tokens.push(Token {
            kind,
            span: Span::new(start, self.pos),
        });
    }
    fn error(&mut self, start: usize, message: String) {
        self.diagnostics.push(Diagnostic {
            span: Span::new(start, self.pos),
            message,
        });
    }
    fn push_comment(&mut self, start: usize, kind: CommentKind) {
        self.push(
            start,
            TokenKind::Comment(Comment {
                span: Span::new(start, self.pos),
                text: self.source[start..self.pos].to_owned(),
                kind,
            }),
        );
    }

    fn comment(&mut self, start: usize) {
        self.pos += 2;
        if let Some((equals, body)) = self.long_open(self.pos) {
            let doc = self.source[body..].starts_with("---");
            self.long_content(equals, body, start, true);
            self.push_comment(
                start,
                if doc {
                    CommentKind::Documentation
                } else {
                    CommentKind::Long
                },
            );
        } else {
            self.line_end();
            let doc = self.source[start..].starts_with("---");
            self.push_comment(
                start,
                if doc {
                    CommentKind::Documentation
                } else {
                    CommentKind::Line
                },
            );
        }
    }

    fn long_open(&self, start: usize) -> Option<(usize, usize)> {
        let bytes = self.source.as_bytes();
        if bytes.get(start) != Some(&b'[') {
            return None;
        }
        let mut end = start + 1;
        while bytes.get(end) == Some(&b'=') {
            end += 1;
        }
        (bytes.get(end) == Some(&b'[')).then_some((end - start - 1, end + 1))
    }

    fn long_content(&mut self, equals: usize, body: usize, start: usize, comment: bool) -> Vec<u8> {
        let close = format!("]{}]", "=".repeat(equals));
        if let Some(relative) = self.source[body..].find(&close) {
            let end = body + relative;
            self.pos = end + close.len();
            let mut content = &self.source[body..end];
            if content.starts_with("\r\n") {
                content = &content[2..];
            } else if content.starts_with(['\r', '\n']) {
                content = &content[1..];
            }
            content
                .replace("\r\n", "\n")
                .replace('\r', "\n")
                .into_bytes()
        } else {
            self.pos = self.source.len();
            self.error(
                start,
                format!(
                    "unterminated long {}",
                    if comment { "comment" } else { "string" }
                ),
            );
            self.source.as_bytes()[body..].to_vec()
        }
    }

    fn short_string(&mut self, quote: char, start: usize) -> Vec<u8> {
        self.advance();
        let mut value = Vec::new();
        while self.pos < self.source.len() {
            let ch = self.char();
            if ch == quote {
                self.advance();
                return value;
            }
            if matches!(ch, '\r' | '\n') {
                self.error(start, "unterminated string before newline".to_owned());
                return value;
            }
            if ch != '\\' {
                let end = self.pos + ch.len_utf8();
                value.extend_from_slice(&self.source.as_bytes()[self.pos..end]);
                self.pos = end;
                continue;
            }
            let escape_start = self.pos;
            self.advance();
            if self.pos == self.source.len() {
                break;
            }
            let escaped = self.char();
            self.advance();
            match escaped {
                'a' => value.push(7),
                'b' => value.push(8),
                'f' => value.push(12),
                'n' => value.push(b'\n'),
                'r' => value.push(b'\r'),
                't' => value.push(b'\t'),
                'v' => value.push(11),
                '\\' => value.push(b'\\'),
                '\'' => value.push(b'\''),
                '"' => value.push(b'"'),
                '\n' => value.push(b'\n'),
                '\r' => {
                    if self.at("\n") {
                        self.pos += 1;
                    }
                    value.push(b'\n');
                }
                'z' => {
                    while self.pos < self.source.len() && self.char().is_ascii_whitespace() {
                        self.advance();
                    }
                }
                'x' => {
                    let hex_start = self.pos;
                    for _ in 0..2 {
                        if self.pos < self.source.len() && self.char().is_ascii_hexdigit() {
                            self.advance();
                        }
                    }
                    if self.pos - hex_start == 2 {
                        value.push(
                            u8::from_str_radix(&self.source[hex_start..self.pos], 16).unwrap(),
                        );
                    } else {
                        self.error(escape_start, "hex escape requires two digits".to_owned());
                    }
                }
                'u' => {
                    if !self.at("{") {
                        self.error(escape_start, "Unicode escape requires braces".to_owned());
                        continue;
                    }
                    self.pos += 1;
                    let hex_start = self.pos;
                    while self.pos < self.source.len() && self.char().is_ascii_hexdigit() {
                        self.advance();
                    }
                    let scalar = u32::from_str_radix(&self.source[hex_start..self.pos], 16)
                        .ok()
                        .and_then(char::from_u32);
                    if self.at("}") {
                        self.pos += 1;
                    } else {
                        self.error(escape_start, "unclosed Unicode escape".to_owned());
                    }
                    if let Some(scalar) = scalar {
                        let mut buffer = [0; 4];
                        value.extend_from_slice(scalar.encode_utf8(&mut buffer).as_bytes());
                    } else {
                        self.error(escape_start, "invalid Unicode scalar escape".to_owned());
                    }
                }
                digit if digit.is_ascii_digit() => {
                    let digits_start = self.pos - 1;
                    for _ in 0..2 {
                        if self.pos < self.source.len() && self.char().is_ascii_digit() {
                            self.advance();
                        }
                    }
                    let number = self.source[digits_start..self.pos].parse::<u16>().unwrap();
                    if let Ok(byte) = u8::try_from(number) {
                        value.push(byte);
                    } else {
                        self.error(escape_start, "decimal escape exceeds 255".to_owned());
                    }
                }
                _ => self.error(
                    escape_start,
                    format!("unsupported string escape \\{escaped}"),
                ),
            }
        }
        self.error(start, "unterminated string".to_owned());
        value
    }

    fn number(&mut self, start: usize) {
        let hex = self.at("0x") || self.at("0X");
        if hex {
            self.pos += 2;
        }
        let mut digits = self.digits(hex);
        if self.at(".") && !self.at("..") {
            self.pos += 1;
            digits += self.digits(hex);
        }
        let exponent = if hex {
            self.at("p") || self.at("P")
        } else {
            self.at("e") || self.at("E")
        };
        let mut valid = digits > 0;
        if exponent {
            self.pos += 1;
            if self.at("+") || self.at("-") {
                self.pos += 1;
            }
            valid &= self.digits(false) > 0;
        }
        // Lua scans adjoining dots as part of the numeral before validating it.
        // `1..2` is malformed; concatenation needs whitespace after the number.
        if self.at(".") {
            valid = false;
            while self.pos < self.source.len()
                && (self.char() == '.' || identifier_continue(self.char())) { self.advance(); }
        }
        if self.pos < self.source.len() && identifier_continue(self.char()) {
            valid = false;
            while self.pos < self.source.len() && identifier_continue(self.char()) {
                self.advance();
            }
        }
        if !valid {
            self.error(start, "invalid or unsupported numeric literal".to_owned());
        }
        self.push(
            start,
            TokenKind::Number(self.source[start..self.pos].to_owned()),
        );
    }

    fn digits(&mut self, hex: bool) -> usize {
        let start = self.pos;
        while self.pos < self.source.len()
            && if hex {
                self.char().is_ascii_hexdigit()
            } else {
                self.char().is_ascii_digit()
            }
        {
            self.pos += 1;
        }
        self.pos - start
    }
}

fn identifier_start(ch: char) -> bool {
    ch == '_' || ch.is_alphabetic() || ch >= '\u{00ff}'
}
fn identifier_continue(ch: char) -> bool {
    identifier_start(ch) || ch.is_ascii_digit()
}
fn keyword(value: &str) -> bool {
    matches!(
        value,
        "and"
            | "break"
            | "do"
            | "else"
            | "elseif"
            | "end"
            | "false"
            | "for"
            | "function"
            | "goto"
            | "if"
            | "in"
            | "local"
            | "nil"
            | "not"
            | "or"
            | "repeat"
            | "return"
            | "then"
            | "true"
            | "until"
            | "while"
    )
}
