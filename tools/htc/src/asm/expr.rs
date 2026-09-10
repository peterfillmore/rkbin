//! Constant-expression evaluator for assembler operands.

use std::collections::HashMap;

use super::lexer::{Token, TokenKind};
use super::{SymKind, Symbol};

pub enum ExprError {
    Undefined(String),
    Syntax(String),
}

pub struct ExprParser<'a> {
    tokens: &'a [Token],
    pos: usize,
    symbols: &'a HashMap<String, Symbol>,
    pc: i64,
}

impl<'a> ExprParser<'a> {
    pub fn new(tokens: &'a [Token], symbols: &'a HashMap<String, Symbol>, pc: i64) -> Self {
        ExprParser { tokens, pos: 0, symbols, pc }
    }

    pub fn parse(&mut self) -> Result<i64, ExprError> {
        let v = self.or()?;
        if self.pos != self.tokens.len() {
            return Err(ExprError::Syntax(format!("unexpected token {:?}", self.tokens[self.pos].kind)));
        }
        Ok(v)
    }

    fn peek(&self) -> Option<&TokenKind> {
        self.tokens.get(self.pos).map(|t| &t.kind)
    }

    fn bump(&mut self) -> Option<&TokenKind> {
        let t = self.tokens.get(self.pos).map(|t| &t.kind);
        self.pos += 1;
        t
    }

    fn or(&mut self) -> Result<i64, ExprError> {
        let mut v = self.xor()?;
        while self.peek() == Some(&TokenKind::Pipe) {
            self.bump();
            v |= self.xor()?;
        }
        Ok(v)
    }

    fn xor(&mut self) -> Result<i64, ExprError> {
        let mut v = self.and()?;
        while self.peek() == Some(&TokenKind::Caret) {
            self.bump();
            v ^= self.and()?;
        }
        Ok(v)
    }

    fn and(&mut self) -> Result<i64, ExprError> {
        let mut v = self.shift()?;
        while self.peek() == Some(&TokenKind::Amp) {
            self.bump();
            v &= self.shift()?;
        }
        Ok(v)
    }

    fn shift(&mut self) -> Result<i64, ExprError> {
        let mut v = self.additive()?;
        loop {
            match self.peek() {
                Some(TokenKind::Shl) => {
                    self.bump();
                    v = v.wrapping_shl(self.additive()? as u32);
                }
                Some(TokenKind::Shr) => {
                    self.bump();
                    v = v.wrapping_shr(self.additive()? as u32);
                }
                _ => return Ok(v),
            }
        }
    }

    fn additive(&mut self) -> Result<i64, ExprError> {
        let mut v = self.term()?;
        loop {
            match self.peek() {
                Some(TokenKind::Plus) => {
                    self.bump();
                    v = v.wrapping_add(self.term()?);
                }
                Some(TokenKind::Minus) => {
                    self.bump();
                    v = v.wrapping_sub(self.term()?);
                }
                _ => return Ok(v),
            }
        }
    }

    fn term(&mut self) -> Result<i64, ExprError> {
        let mut v = self.unary()?;
        loop {
            match self.peek() {
                Some(TokenKind::Star) => {
                    self.bump();
                    v = v.wrapping_mul(self.unary()?);
                }
                Some(TokenKind::Slash) => {
                    self.bump();
                    let d = self.unary()?;
                    if d == 0 {
                        return Err(ExprError::Syntax("division by zero".into()));
                    }
                    v /= d;
                }
                Some(TokenKind::Percent) => {
                    self.bump();
                    let d = self.unary()?;
                    if d == 0 {
                        return Err(ExprError::Syntax("division by zero".into()));
                    }
                    v %= d;
                }
                _ => return Ok(v),
            }
        }
    }

    fn unary(&mut self) -> Result<i64, ExprError> {
        match self.peek() {
            Some(TokenKind::Minus) => {
                self.bump();
                Ok(self.unary()?.wrapping_neg())
            }
            Some(TokenKind::Plus) => {
                self.bump();
                self.unary()
            }
            Some(TokenKind::Tilde) => {
                self.bump();
                Ok(!self.unary()?)
            }
            _ => self.primary(),
        }
    }

    fn primary(&mut self) -> Result<i64, ExprError> {
        match self.bump().cloned() {
            Some(TokenKind::Number(n)) => Ok(n),
            Some(TokenKind::Dollar) => Ok(self.pc),
            Some(TokenKind::LParen) => {
                let v = self.or()?;
                if self.bump() != Some(&TokenKind::RParen) {
                    return Err(ExprError::Syntax("missing ')'".into()));
                }
                Ok(v)
            }
            Some(TokenKind::LBracket) => {
                // `[expr]` inside an expression evaluates to the address.
                let v = self.or()?;
                if self.bump() != Some(&TokenKind::RBracket) {
                    return Err(ExprError::Syntax("missing ']'".into()));
                }
                Ok(v)
            }
            Some(TokenKind::Ident(name)) => {
                let lname = name.to_ascii_lowercase();
                match lname.as_str() {
                    "low" => Ok(self.unary()? & 0xFF),
                    "high" => Ok((self.unary()? >> 8) & 0xFF),
                    "offset" => self.unary(),
                    _ => match self.symbols.get(&lname) {
                        Some(Symbol { value, kind: SymKind::Num }) | Some(Symbol { value, kind: SymKind::Mem }) => {
                            Ok(*value)
                        }
                        Some(Symbol { kind: SymKind::Bit(_), .. }) => {
                            Err(ExprError::Syntax(format!("bit symbol '{}' used in an expression", name)))
                        }
                        None => Err(ExprError::Undefined(name)),
                    },
                }
            }
            Some(other) => Err(ExprError::Syntax(format!("unexpected token {:?} in expression", other))),
            None => Err(ExprError::Syntax("unexpected end of expression".into())),
        }
    }
}
