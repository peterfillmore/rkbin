//! Line tokenizer for the assembler.

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    Ident(String),
    Number(i64),
    Str(String),
    Comma,
    Colon,
    Dot,
    LBracket,
    RBracket,
    LParen,
    RParen,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Amp,
    Pipe,
    Caret,
    Tilde,
    Shl,
    Shr,
    Dollar,
    Question,
    Hash,
    Equals,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub col: usize,
}

/// Tokenize one source line.  Comments start with `;` (or `//`).
pub fn tokenize(line: &str) -> Result<Vec<Token>, String> {
    let chars: Vec<char> = line.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let col = i + 1;
        match c {
            ';' => break,
            '/' if chars.get(i + 1) == Some(&'/') => break,
            c if c.is_whitespace() => {
                i += 1;
            }
            '\'' | '"' => {
                let quote = c;
                let mut s = String::new();
                i += 1;
                loop {
                    match chars.get(i) {
                        None => return Err("unterminated string".into()),
                        Some(&q) if q == quote => {
                            i += 1;
                            break;
                        }
                        Some(&'\\') => {
                            i += 1;
                            match chars.get(i) {
                                Some('n') => s.push('\n'),
                                Some('r') => s.push('\r'),
                                Some('t') => s.push('\t'),
                                Some('0') => s.push('\0'),
                                Some(&x) => s.push(x),
                                None => return Err("unterminated escape".into()),
                            }
                            i += 1;
                        }
                        Some(&x) => {
                            s.push(x);
                            i += 1;
                        }
                    }
                }
                // Single-character single-quoted literals are numbers.
                if quote == '\'' && s.chars().count() == 1 {
                    tokens.push(Token {
                        kind: TokenKind::Number(s.chars().next().unwrap() as i64),
                        col,
                    });
                } else {
                    tokens.push(Token {
                        kind: TokenKind::Str(s),
                        col,
                    });
                }
            }
            c if c.is_ascii_digit() => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                let text: String = chars[start..i].iter().filter(|c| **c != '_').collect();
                let value =
                    parse_number(&text).ok_or_else(|| format!("invalid number '{}'", text))?;
                tokens.push(Token {
                    kind: TokenKind::Number(value),
                    col,
                });
            }
            c if c.is_alphabetic() || c == '_' || c == '@' => {
                let start = i;
                while i < chars.len()
                    && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '@')
                {
                    i += 1;
                }
                let text: String = chars[start..i].iter().collect();
                tokens.push(Token {
                    kind: TokenKind::Ident(text),
                    col,
                });
            }
            '.' => {
                tokens.push(Token {
                    kind: TokenKind::Dot,
                    col,
                });
                i += 1;
            }
            '#' => {
                tokens.push(Token {
                    kind: TokenKind::Hash,
                    col,
                });
                i += 1;
            }
            '<' if chars.get(i + 1) == Some(&'<') => {
                tokens.push(Token {
                    kind: TokenKind::Shl,
                    col,
                });
                i += 2;
            }
            '>' if chars.get(i + 1) == Some(&'>') => {
                tokens.push(Token {
                    kind: TokenKind::Shr,
                    col,
                });
                i += 2;
            }
            _ => {
                let kind = match c {
                    ',' => TokenKind::Comma,
                    ':' => TokenKind::Colon,
                    '[' => TokenKind::LBracket,
                    ']' => TokenKind::RBracket,
                    '(' => TokenKind::LParen,
                    ')' => TokenKind::RParen,
                    '+' => TokenKind::Plus,
                    '-' => TokenKind::Minus,
                    '*' => TokenKind::Star,
                    '/' => TokenKind::Slash,
                    '%' => TokenKind::Percent,
                    '&' => TokenKind::Amp,
                    '|' => TokenKind::Pipe,
                    '^' => TokenKind::Caret,
                    '~' => TokenKind::Tilde,
                    '$' => TokenKind::Dollar,
                    '?' => TokenKind::Question,
                    '=' => TokenKind::Equals,
                    other => return Err(format!("unexpected character '{}'", other)),
                };
                tokens.push(Token { kind, col });
                i += 1;
            }
        }
    }
    // `#include` → merge hash + ident into one directive ident.
    if tokens.len() >= 2 && tokens[0].kind == TokenKind::Hash {
        if let TokenKind::Ident(s) = &tokens[1].kind {
            let merged = Token {
                kind: TokenKind::Ident(format!("#{}", s)),
                col: tokens[0].col,
            };
            tokens.splice(0..2, [merged]);
        }
    }
    Ok(tokens)
}

/// Parse `0F00h`, `0xF00`, `1010b`, `77o`, `123d`, `123`.
pub fn parse_number(text: &str) -> Option<i64> {
    let t = text.to_ascii_lowercase();
    if let Some(h) = t.strip_prefix("0x") {
        return i64::from_str_radix(h, 16).ok();
    }
    if let Some(b) = t.strip_prefix("0b") {
        if let Ok(v) = i64::from_str_radix(b, 2) {
            return Some(v);
        }
    }
    if let Some(h) = t.strip_suffix('h') {
        return i64::from_str_radix(h, 16).ok();
    }
    if let Some(b) = t.strip_suffix('b') {
        return i64::from_str_radix(b, 2).ok();
    }
    if let Some(o) = t.strip_suffix('o') {
        return i64::from_str_radix(o, 8).ok();
    }
    if let Some(d) = t.strip_suffix('d') {
        return d.parse().ok();
    }
    t.parse().ok()
}
