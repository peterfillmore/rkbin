//! Tokenizer for the `htc` language.

use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Ident(String),
    Int(i64),
    /// Raw text of an `asm { ... }` block.
    AsmBlock(String),
    Str(String),
    // punctuation
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Semi,
    Comma,
    Dot,
    Colon,
    Question,
    At,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Amp,
    Pipe,
    Caret,
    Tilde,
    Bang,
    Lt,
    Gt,
    Le,
    Ge,
    EqEq,
    Ne,
    AndAnd,
    OrOr,
    Shl,
    Shr,
    Assign,
    PlusAssign,
    MinusAssign,
    StarAssign,
    SlashAssign,
    PercentAssign,
    AmpAssign,
    PipeAssign,
    CaretAssign,
    ShlAssign,
    ShrAssign,
    Inc,
    Dec,
    Eof,
}

impl fmt::Display for Tok {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Tok::Ident(s) => write!(f, "identifier '{}'", s),
            Tok::Int(v) => write!(f, "number {}", v),
            Tok::AsmBlock(_) => write!(f, "asm block"),
            Tok::Str(s) => write!(f, "string \"{}\"", s),
            Tok::Eof => write!(f, "end of file"),
            other => write!(f, "'{}'", punct_text(other)),
        }
    }
}

fn punct_text(t: &Tok) -> &'static str {
    match t {
        Tok::LParen => "(",
        Tok::RParen => ")",
        Tok::LBrace => "{",
        Tok::RBrace => "}",
        Tok::LBracket => "[",
        Tok::RBracket => "]",
        Tok::Semi => ";",
        Tok::Comma => ",",
        Tok::Dot => ".",
        Tok::Colon => ":",
        Tok::Question => "?",
        Tok::At => "@",
        Tok::Plus => "+",
        Tok::Minus => "-",
        Tok::Star => "*",
        Tok::Slash => "/",
        Tok::Percent => "%",
        Tok::Amp => "&",
        Tok::Pipe => "|",
        Tok::Caret => "^",
        Tok::Tilde => "~",
        Tok::Bang => "!",
        Tok::Lt => "<",
        Tok::Gt => ">",
        Tok::Le => "<=",
        Tok::Ge => ">=",
        Tok::EqEq => "==",
        Tok::Ne => "!=",
        Tok::AndAnd => "&&",
        Tok::OrOr => "||",
        Tok::Shl => "<<",
        Tok::Shr => ">>",
        Tok::Assign => "=",
        Tok::PlusAssign => "+=",
        Tok::MinusAssign => "-=",
        Tok::StarAssign => "*=",
        Tok::SlashAssign => "/=",
        Tok::PercentAssign => "%=",
        Tok::AmpAssign => "&=",
        Tok::PipeAssign => "|=",
        Tok::CaretAssign => "^=",
        Tok::ShlAssign => "<<=",
        Tok::ShrAssign => ">>=",
        Tok::Inc => "++",
        Tok::Dec => "--",
        _ => "?",
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub tok: Tok,
    pub line: usize,
    pub col: usize,
}

#[derive(Debug, Clone)]
pub struct LexError {
    pub line: usize,
    pub col: usize,
    pub message: String,
}

pub fn tokenize(src: &str) -> Result<Vec<Token>, LexError> {
    let chars: Vec<char> = src.chars().collect();
    let mut toks = Vec::new();
    let mut i = 0;
    let mut line = 1;
    let mut line_start = 0;
    let err = |line: usize, col: usize, m: &str| LexError {
        line,
        col,
        message: m.to_string(),
    };

    while i < chars.len() {
        let c = chars[i];
        let col = i - line_start + 1;
        if c == '\n' {
            line += 1;
            i += 1;
            line_start = i;
            continue;
        }
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        // comments
        if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            loop {
                if i >= chars.len() {
                    return Err(err(line, col, "unterminated block comment"));
                }
                if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                    i += 2;
                    break;
                }
                if chars[i] == '\n' {
                    line += 1;
                    line_start = i + 1;
                }
                i += 1;
            }
            continue;
        }
        // identifiers / keywords / asm blocks
        if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            if word == "asm" {
                // asm { ... }  — raw text up to the matching '}'
                let mut j = i;
                while j < chars.len() && chars[j].is_whitespace() {
                    if chars[j] == '\n' {
                        line += 1;
                        line_start = j + 1;
                    }
                    j += 1;
                }
                if chars.get(j) == Some(&'{') {
                    j += 1;
                    let body_start = j;
                    let mut depth = 1;
                    while j < chars.len() {
                        match chars[j] {
                            '{' => depth += 1,
                            '}' => {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                            '\n' => {
                                line += 1;
                                line_start = j + 1;
                            }
                            _ => {}
                        }
                        j += 1;
                    }
                    if j >= chars.len() {
                        return Err(err(line, col, "unterminated asm block"));
                    }
                    let body: String = chars[body_start..j].iter().collect();
                    toks.push(Token {
                        tok: Tok::AsmBlock(body),
                        line,
                        col,
                    });
                    i = j + 1;
                    continue;
                }
            }
            toks.push(Token {
                tok: Tok::Ident(word),
                line,
                col,
            });
            continue;
        }
        // numbers
        if c.is_ascii_digit() {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let text: String = chars[start..i].iter().filter(|c| **c != '_').collect();
            let lower = text.to_ascii_lowercase();
            let value = if let Some(h) = lower.strip_prefix("0x") {
                i64::from_str_radix(h, 16).ok()
            } else if let Some(b) = lower.strip_prefix("0b") {
                i64::from_str_radix(b, 2).ok()
            } else if let Some(h) = lower.strip_suffix('h') {
                i64::from_str_radix(h, 16).ok()
            } else if lower.starts_with('0')
                && lower.len() > 1
                && lower.chars().all(|c| c.is_ascii_digit())
            {
                i64::from_str_radix(&lower[1..], 8).ok()
            } else {
                lower.parse().ok()
            };
            match value {
                Some(v) => toks.push(Token {
                    tok: Tok::Int(v),
                    line,
                    col,
                }),
                None => return Err(err(line, col, &format!("invalid number '{}'", text))),
            }
            continue;
        }
        // char literal
        if c == '\'' {
            i += 1;
            let ch = match chars.get(i) {
                Some('\\') => {
                    i += 1;
                    let e = match chars.get(i) {
                        Some('n') => '\n',
                        Some('r') => '\r',
                        Some('t') => '\t',
                        Some('0') => '\0',
                        Some('\\') => '\\',
                        Some('\'') => '\'',
                        Some(&x) => x,
                        None => return Err(err(line, col, "unterminated char literal")),
                    };
                    e
                }
                Some(&x) => x,
                None => return Err(err(line, col, "unterminated char literal")),
            };
            i += 1;
            if chars.get(i) != Some(&'\'') {
                return Err(err(line, col, "unterminated char literal"));
            }
            i += 1;
            toks.push(Token {
                tok: Tok::Int(ch as i64),
                line,
                col,
            });
            continue;
        }
        if c == '"' {
            i += 1;
            let mut s = String::new();
            loop {
                match chars.get(i) {
                    None | Some('\n') => return Err(err(line, col, "unterminated string")),
                    Some('"') => {
                        i += 1;
                        break;
                    }
                    Some('\\') => {
                        i += 1;
                        match chars.get(i) {
                            Some('n') => s.push('\n'),
                            Some('t') => s.push('\t'),
                            Some('0') => s.push('\0'),
                            Some(&x) => s.push(x),
                            None => return Err(err(line, col, "unterminated string")),
                        }
                        i += 1;
                    }
                    Some(&x) => {
                        s.push(x);
                        i += 1;
                    }
                }
            }
            toks.push(Token {
                tok: Tok::Str(s),
                line,
                col,
            });
            continue;
        }
        // operators, longest match first
        let three: String = chars[i..(i + 3).min(chars.len())].iter().collect();
        let two: String = chars[i..(i + 2).min(chars.len())].iter().collect();
        let (tok, len) = match three.as_str() {
            "<<=" => (Tok::ShlAssign, 3),
            ">>=" => (Tok::ShrAssign, 3),
            _ => match two.as_str() {
                "<=" => (Tok::Le, 2),
                ">=" => (Tok::Ge, 2),
                "==" => (Tok::EqEq, 2),
                "!=" => (Tok::Ne, 2),
                "&&" => (Tok::AndAnd, 2),
                "||" => (Tok::OrOr, 2),
                "<<" => (Tok::Shl, 2),
                ">>" => (Tok::Shr, 2),
                "+=" => (Tok::PlusAssign, 2),
                "-=" => (Tok::MinusAssign, 2),
                "*=" => (Tok::StarAssign, 2),
                "/=" => (Tok::SlashAssign, 2),
                "%=" => (Tok::PercentAssign, 2),
                "&=" => (Tok::AmpAssign, 2),
                "|=" => (Tok::PipeAssign, 2),
                "^=" => (Tok::CaretAssign, 2),
                "++" => (Tok::Inc, 2),
                "--" => (Tok::Dec, 2),
                _ => match c {
                    '(' => (Tok::LParen, 1),
                    ')' => (Tok::RParen, 1),
                    '{' => (Tok::LBrace, 1),
                    '}' => (Tok::RBrace, 1),
                    '[' => (Tok::LBracket, 1),
                    ']' => (Tok::RBracket, 1),
                    ';' => (Tok::Semi, 1),
                    ',' => (Tok::Comma, 1),
                    '.' => (Tok::Dot, 1),
                    ':' => (Tok::Colon, 1),
                    '?' => (Tok::Question, 1),
                    '@' => (Tok::At, 1),
                    '+' => (Tok::Plus, 1),
                    '-' => (Tok::Minus, 1),
                    '*' => (Tok::Star, 1),
                    '/' => (Tok::Slash, 1),
                    '%' => (Tok::Percent, 1),
                    '&' => (Tok::Amp, 1),
                    '|' => (Tok::Pipe, 1),
                    '^' => (Tok::Caret, 1),
                    '~' => (Tok::Tilde, 1),
                    '!' => (Tok::Bang, 1),
                    '<' => (Tok::Lt, 1),
                    '>' => (Tok::Gt, 1),
                    '=' => (Tok::Assign, 1),
                    other => {
                        return Err(err(line, col, &format!("unexpected character '{}'", other)))
                    }
                },
            },
        };
        toks.push(Token { tok, line, col });
        i += len;
    }
    toks.push(Token {
        tok: Tok::Eof,
        line,
        col: 1,
    });
    Ok(toks)
}
