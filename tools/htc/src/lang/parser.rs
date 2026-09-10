//! Recursive-descent parser for the `htc` language.

use super::ast::*;
use super::lexer::{Tok, Token};
use crate::device;

#[derive(Debug, Clone)]
pub struct ParseError {
    pub line: usize,
    pub col: usize,
    pub message: String,
}

pub struct Parser {
    toks: Vec<Token>,
    pos: usize,
}

type PResult<T> = Result<T, ParseError>;

impl Parser {
    pub fn new(toks: Vec<Token>) -> Self {
        Parser { toks, pos: 0 }
    }

    fn peek(&self) -> &Tok {
        &self.toks[self.pos].tok
    }

    fn peek_at(&self, n: usize) -> &Tok {
        &self.toks[(self.pos + n).min(self.toks.len() - 1)].tok
    }

    fn here(&self) -> Pos {
        let t = &self.toks[self.pos];
        Pos {
            line: t.line,
            col: t.col,
        }
    }

    fn bump(&mut self) -> Tok {
        let t = self.toks[self.pos].tok.clone();
        if self.pos < self.toks.len() - 1 {
            self.pos += 1;
        }
        t
    }

    fn error<T>(&self, msg: impl Into<String>) -> PResult<T> {
        let p = self.here();
        Err(ParseError {
            line: p.line,
            col: p.col,
            message: msg.into(),
        })
    }

    fn expect(&mut self, t: Tok) -> PResult<()> {
        if *self.peek() == t {
            self.bump();
            Ok(())
        } else {
            self.error(format!("expected {} but found {}", t, self.peek()))
        }
    }

    fn eat(&mut self, t: &Tok) -> bool {
        if self.peek() == t {
            self.bump();
            true
        } else {
            false
        }
    }

    fn ident(&mut self) -> PResult<String> {
        match self.peek().clone() {
            Tok::Ident(s) => {
                self.bump();
                Ok(s)
            }
            other => self.error(format!("expected identifier but found {}", other)),
        }
    }

    fn is_type_name(&self, t: &Tok) -> bool {
        matches!(t, Tok::Ident(s) if Type::from_name(s).is_some())
    }

    fn parse_type(&mut self) -> PResult<Type> {
        match self.peek().clone() {
            Tok::Ident(s) => match Type::from_name(&s) {
                Some(t) => {
                    self.bump();
                    Ok(t)
                }
                None => self.error(format!("unknown type '{}'", s)),
            },
            other => self.error(format!("expected type but found {}", other)),
        }
    }

    // ------------------------------------------------------------------
    // Top level
    // ------------------------------------------------------------------

    pub fn parse_program(&mut self) -> PResult<Program> {
        let mut prog = Program::default();
        while *self.peek() != Tok::Eof {
            if self.eat(&Tok::Semi) {
                continue;
            }
            let pos = self.here();
            // interrupt handler
            if *self.peek() == Tok::Ident("interrupt".into()) {
                self.bump();
                let vector = self.parse_vector()?;
                let ret = self.parse_type()?;
                let name = self.ident()?;
                let f = self.parse_function_rest(name, ret, Some(vector), pos)?;
                prog.functions.push(f);
                continue;
            }
            let is_const = self.eat(&Tok::Ident("const".into()));
            let _ = self.eat(&Tok::Ident("static".into()));
            let ty = self.parse_type()?;
            let name = self.ident()?;
            if *self.peek() == Tok::LParen {
                if is_const {
                    return self.error("functions cannot be const");
                }
                let f = self.parse_function_rest(name, ty, None, pos)?;
                prog.functions.push(f);
            } else {
                let mut decls = self.parse_var_decl_rest(name, ty, is_const, pos)?;
                prog.globals.append(&mut decls);
            }
        }
        Ok(prog)
    }

    fn parse_vector(&mut self) -> PResult<u16> {
        // `interrupt INT0` or `interrupt(0x04)` or `interrupt 4`
        if self.eat(&Tok::LParen) {
            let v = self.parse_const_int()?;
            self.expect(Tok::RParen)?;
            return Ok(v as u16);
        }
        match self.peek().clone() {
            Tok::Ident(name) if !self.is_type_name(&Tok::Ident(name.clone())) => {
                self.bump();
                match device::vector_by_name(&name) {
                    Some(v) => Ok(v),
                    None => self.error(format!(
                        "unknown interrupt '{}'; known: {}",
                        name,
                        device::VECTORS
                            .iter()
                            .map(|v| v.0)
                            .collect::<Vec<_>>()
                            .join(", ")
                    )),
                }
            }
            Tok::Int(v) => {
                self.bump();
                Ok(v as u16)
            }
            _ => self.error("expected interrupt name or vector address after 'interrupt'"),
        }
    }

    fn parse_const_int(&mut self) -> PResult<i64> {
        let e = self.parse_expr()?;
        match fold_const(&e) {
            Some(v) => Ok(v),
            None => Err(ParseError {
                line: e.pos.line,
                col: e.pos.col,
                message: "expected constant expression".into(),
            }),
        }
    }

    fn parse_function_rest(
        &mut self,
        name: String,
        ret: Type,
        interrupt: Option<u16>,
        pos: Pos,
    ) -> PResult<FnDecl> {
        self.expect(Tok::LParen)?;
        let mut params = Vec::new();
        if !self.eat(&Tok::RParen) {
            if *self.peek() == Tok::Ident("void".into()) && *self.peek_at(1) == Tok::RParen {
                self.bump();
            } else {
                loop {
                    let ty = self.parse_type()?;
                    if ty == Type::Void {
                        return self.error("parameters cannot be void");
                    }
                    let pname = self.ident()?;
                    params.push(Param { name: pname, ty });
                    if !self.eat(&Tok::Comma) {
                        break;
                    }
                }
            }
            self.expect(Tok::RParen)?;
        }
        if interrupt.is_some() && (!params.is_empty() || ret != Type::Void) {
            return self.error("interrupt handlers must be 'void name()'");
        }
        let body = self.parse_block()?;
        Ok(FnDecl {
            name,
            ret,
            params,
            body,
            interrupt,
            pos,
        })
    }

    /// Parses the remainder of `type name ...;` after the first name.
    fn parse_var_decl_rest(
        &mut self,
        first: String,
        ty: Type,
        is_const: bool,
        pos: Pos,
    ) -> PResult<Vec<VarDecl>> {
        if ty == Type::Void {
            return self.error("variables cannot be void");
        }
        let mut decls = Vec::new();
        let mut name = first;
        loop {
            let mut array_len = None;
            let mut array_len_expr = None;
            let mut at = None;
            if self.eat(&Tok::LBracket) {
                if *self.peek() == Tok::RBracket {
                    self.bump();
                    array_len = Some(0); // length from initializer
                } else {
                    let e = self.parse_expr()?;
                    self.expect(Tok::RBracket)?;
                    match fold_const(&e) {
                        Some(n) => {
                            if !(1..=256).contains(&n) {
                                return self.error("array length must be 1..256");
                            }
                            array_len = Some(n as usize);
                        }
                        None => {
                            array_len = Some(0);
                            array_len_expr = Some(e);
                        }
                    }
                }
            }
            if self.eat(&Tok::At) {
                let a = self.parse_const_int()?;
                if !(0..=0xFF).contains(&a) {
                    return self.error("absolute address must be 00h..FFh");
                }
                at = Some(a as u8);
            }
            let init = if self.eat(&Tok::Assign) {
                if self.eat(&Tok::LBrace) {
                    let mut items = Vec::new();
                    while *self.peek() != Tok::RBrace {
                        items.push(self.parse_assignment()?);
                        if !self.eat(&Tok::Comma) {
                            break;
                        }
                    }
                    self.expect(Tok::RBrace)?;
                    Some(Initializer::List(items))
                } else if let Tok::Str(s) = self.peek().clone() {
                    self.bump();
                    let items = s
                        .bytes()
                        .chain(std::iter::once(0))
                        .map(|b| Expr {
                            kind: ExprKind::Int(b as i64),
                            pos,
                        })
                        .collect();
                    Some(Initializer::List(items))
                } else {
                    Some(Initializer::Scalar(self.parse_assignment()?))
                }
            } else {
                None
            };
            if let (Some(0), None) = (array_len, &array_len_expr) {
                match &init {
                    Some(Initializer::List(items)) if !items.is_empty() => {
                        array_len = Some(items.len())
                    }
                    _ => return self.error("array without length needs an initializer list"),
                }
            }
            if let (Some(Initializer::List(items)), Some(n), None) =
                (&init, array_len, &array_len_expr)
            {
                if items.len() > n {
                    return self.error("too many initializers for array");
                }
            }
            if let (Some(Initializer::List(_)), None) = (&init, array_len) {
                return self.error("initializer list for a scalar variable");
            }
            decls.push(VarDecl {
                name,
                ty,
                array_len,
                array_len_expr,
                is_const,
                at,
                init,
                pos,
            });
            if self.eat(&Tok::Comma) {
                name = self.ident()?;
                continue;
            }
            break;
        }
        self.expect(Tok::Semi)?;
        Ok(decls)
    }

    // ------------------------------------------------------------------
    // Statements
    // ------------------------------------------------------------------

    fn parse_block(&mut self) -> PResult<Vec<Stmt>> {
        self.expect(Tok::LBrace)?;
        let mut stmts = Vec::new();
        while !self.eat(&Tok::RBrace) {
            if *self.peek() == Tok::Eof {
                return self.error("unexpected end of file inside block");
            }
            stmts.push(self.parse_stmt()?);
        }
        Ok(stmts)
    }

    fn starts_declaration(&self) -> bool {
        match self.peek() {
            Tok::Ident(s) if s == "const" || s == "static" => true,
            t if self.is_type_name(t) => matches!(self.peek_at(1), Tok::Ident(_)),
            _ => false,
        }
    }

    fn parse_declaration_stmts(&mut self) -> PResult<Vec<Stmt>> {
        let pos = self.here();
        let is_const = self.eat(&Tok::Ident("const".into()));
        let _ = self.eat(&Tok::Ident("static".into()));
        let ty = self.parse_type()?;
        let name = self.ident()?;
        let decls = self.parse_var_decl_rest(name, ty, is_const, pos)?;
        Ok(decls
            .into_iter()
            .map(|d| Stmt {
                pos: d.pos,
                kind: StmtKind::Decl(d),
            })
            .collect())
    }

    fn parse_stmt(&mut self) -> PResult<Stmt> {
        let pos = self.here();
        if self.starts_declaration() {
            let mut stmts = self.parse_declaration_stmts()?;
            return Ok(if stmts.len() == 1 {
                stmts.remove(0)
            } else {
                Stmt {
                    kind: StmtKind::Block(stmts),
                    pos,
                }
            });
        }
        let kind = match self.peek().clone() {
            Tok::LBrace => StmtKind::Block(self.parse_block()?),
            Tok::Semi => {
                self.bump();
                StmtKind::Empty
            }
            Tok::AsmBlock(text) => {
                self.bump();
                let _ = self.eat(&Tok::Semi);
                StmtKind::Asm(text)
            }
            Tok::Ident(kw) => match kw.as_str() {
                "if" => {
                    self.bump();
                    self.expect(Tok::LParen)?;
                    let cond = self.parse_expr()?;
                    self.expect(Tok::RParen)?;
                    let then = Box::new(self.parse_stmt()?);
                    let els = if self.eat(&Tok::Ident("else".into())) {
                        Some(Box::new(self.parse_stmt()?))
                    } else {
                        None
                    };
                    StmtKind::If(cond, then, els)
                }
                "while" => {
                    self.bump();
                    self.expect(Tok::LParen)?;
                    let cond = self.parse_expr()?;
                    self.expect(Tok::RParen)?;
                    StmtKind::While(cond, Box::new(self.parse_stmt()?))
                }
                "do" => {
                    self.bump();
                    let body = Box::new(self.parse_stmt()?);
                    self.expect(Tok::Ident("while".into()))?;
                    self.expect(Tok::LParen)?;
                    let cond = self.parse_expr()?;
                    self.expect(Tok::RParen)?;
                    self.expect(Tok::Semi)?;
                    StmtKind::DoWhile(body, cond)
                }
                "for" => {
                    self.bump();
                    self.expect(Tok::LParen)?;
                    let init = if self.eat(&Tok::Semi) {
                        None
                    } else if self.starts_declaration() {
                        let stmts = self.parse_declaration_stmts()?;
                        Some(Box::new(Stmt {
                            kind: StmtKind::Block(stmts),
                            pos,
                        }))
                    } else {
                        let e = self.parse_expr()?;
                        self.expect(Tok::Semi)?;
                        Some(Box::new(Stmt {
                            kind: StmtKind::Expr(e),
                            pos,
                        }))
                    };
                    let cond = if *self.peek() == Tok::Semi {
                        None
                    } else {
                        Some(self.parse_expr()?)
                    };
                    self.expect(Tok::Semi)?;
                    let step = if *self.peek() == Tok::RParen {
                        None
                    } else {
                        Some(self.parse_expr()?)
                    };
                    self.expect(Tok::RParen)?;
                    let body = Box::new(self.parse_stmt()?);
                    StmtKind::For(init, cond, step, body)
                }
                "switch" => {
                    self.bump();
                    self.expect(Tok::LParen)?;
                    let subject = self.parse_expr()?;
                    self.expect(Tok::RParen)?;
                    self.expect(Tok::LBrace)?;
                    let mut cases = Vec::new();
                    while !self.eat(&Tok::RBrace) {
                        let mut values = Vec::new();
                        let mut is_default = false;
                        loop {
                            if self.eat(&Tok::Ident("case".into())) {
                                values.push(self.parse_ternary()?);
                                self.expect(Tok::Colon)?;
                            } else if self.eat(&Tok::Ident("default".into())) {
                                is_default = true;
                                self.expect(Tok::Colon)?;
                            } else {
                                break;
                            }
                        }
                        if values.is_empty() && !is_default {
                            return self.error("expected 'case' or 'default' in switch");
                        }
                        let mut body = Vec::new();
                        while !matches!(self.peek(), Tok::RBrace)
                            && !matches!(self.peek(), Tok::Ident(s) if s == "case" || s == "default")
                        {
                            body.push(self.parse_stmt()?);
                        }
                        cases.push(SwitchCase {
                            values,
                            is_default,
                            body,
                        });
                    }
                    StmtKind::Switch(subject, cases)
                }
                "break" => {
                    self.bump();
                    self.expect(Tok::Semi)?;
                    StmtKind::Break
                }
                "continue" => {
                    self.bump();
                    self.expect(Tok::Semi)?;
                    StmtKind::Continue
                }
                "return" => {
                    self.bump();
                    let e = if *self.peek() == Tok::Semi {
                        None
                    } else {
                        Some(self.parse_expr()?)
                    };
                    self.expect(Tok::Semi)?;
                    StmtKind::Return(e)
                }
                _ => {
                    let e = self.parse_expr()?;
                    self.expect(Tok::Semi)?;
                    StmtKind::Expr(e)
                }
            },
            _ => {
                let e = self.parse_expr()?;
                self.expect(Tok::Semi)?;
                StmtKind::Expr(e)
            }
        };
        Ok(Stmt { kind, pos })
    }

    // ------------------------------------------------------------------
    // Expressions (C precedence)
    // ------------------------------------------------------------------

    pub fn parse_expr(&mut self) -> PResult<Expr> {
        self.parse_assignment()
    }

    fn parse_assignment(&mut self) -> PResult<Expr> {
        let lhs = self.parse_ternary()?;
        let pos = lhs.pos;
        let op = match self.peek() {
            Tok::Assign => None,
            Tok::PlusAssign => Some(BinOp::Add),
            Tok::MinusAssign => Some(BinOp::Sub),
            Tok::StarAssign => Some(BinOp::Mul),
            Tok::SlashAssign => Some(BinOp::Div),
            Tok::PercentAssign => Some(BinOp::Rem),
            Tok::AmpAssign => Some(BinOp::And),
            Tok::PipeAssign => Some(BinOp::Or),
            Tok::CaretAssign => Some(BinOp::Xor),
            Tok::ShlAssign => Some(BinOp::Shl),
            Tok::ShrAssign => Some(BinOp::Shr),
            _ => return Ok(lhs),
        };
        self.bump();
        let rhs = self.parse_assignment()?;
        if !is_lvalue(&lhs) {
            return Err(ParseError {
                line: pos.line,
                col: pos.col,
                message: "left side of assignment is not assignable".into(),
            });
        }
        let kind = match op {
            None => ExprKind::Assign(Box::new(lhs), Box::new(rhs)),
            Some(op) => ExprKind::CompoundAssign(op, Box::new(lhs), Box::new(rhs)),
        };
        Ok(Expr { kind, pos })
    }

    fn parse_ternary(&mut self) -> PResult<Expr> {
        let cond = self.parse_binary(0)?;
        if self.eat(&Tok::Question) {
            let pos = cond.pos;
            let a = self.parse_assignment()?;
            self.expect(Tok::Colon)?;
            let b = self.parse_assignment()?;
            return Ok(Expr {
                kind: ExprKind::Ternary(Box::new(cond), Box::new(a), Box::new(b)),
                pos,
            });
        }
        Ok(cond)
    }

    fn binop_of(&self, t: &Tok) -> Option<(BinOp, u8)> {
        Some(match t {
            Tok::OrOr => (BinOp::LOr, 1),
            Tok::AndAnd => (BinOp::LAnd, 2),
            Tok::Pipe => (BinOp::Or, 3),
            Tok::Caret => (BinOp::Xor, 4),
            Tok::Amp => (BinOp::And, 5),
            Tok::EqEq => (BinOp::Eq, 6),
            Tok::Ne => (BinOp::Ne, 6),
            Tok::Lt => (BinOp::Lt, 7),
            Tok::Le => (BinOp::Le, 7),
            Tok::Gt => (BinOp::Gt, 7),
            Tok::Ge => (BinOp::Ge, 7),
            Tok::Shl => (BinOp::Shl, 8),
            Tok::Shr => (BinOp::Shr, 8),
            Tok::Plus => (BinOp::Add, 9),
            Tok::Minus => (BinOp::Sub, 9),
            Tok::Star => (BinOp::Mul, 10),
            Tok::Slash => (BinOp::Div, 10),
            Tok::Percent => (BinOp::Rem, 10),
            _ => return None,
        })
    }

    fn parse_binary(&mut self, min_prec: u8) -> PResult<Expr> {
        let mut lhs = self.parse_unary()?;
        loop {
            let (op, prec) = match self.binop_of(self.peek()) {
                Some(x) if x.1 >= min_prec => x,
                _ => return Ok(lhs),
            };
            self.bump();
            let rhs = self.parse_binary(prec + 1)?;
            let pos = lhs.pos;
            lhs = Expr {
                kind: ExprKind::Binary(op, Box::new(lhs), Box::new(rhs)),
                pos,
            };
        }
    }

    fn parse_unary(&mut self) -> PResult<Expr> {
        let pos = self.here();
        match self.peek().clone() {
            Tok::Minus => {
                self.bump();
                let e = self.parse_unary()?;
                Ok(Expr {
                    kind: ExprKind::Unary(UnOp::Neg, Box::new(e)),
                    pos,
                })
            }
            Tok::Plus => {
                self.bump();
                self.parse_unary()
            }
            Tok::Tilde => {
                self.bump();
                let e = self.parse_unary()?;
                Ok(Expr {
                    kind: ExprKind::Unary(UnOp::Not, Box::new(e)),
                    pos,
                })
            }
            Tok::Bang => {
                self.bump();
                let e = self.parse_unary()?;
                Ok(Expr {
                    kind: ExprKind::Unary(UnOp::LNot, Box::new(e)),
                    pos,
                })
            }
            Tok::Inc | Tok::Dec => {
                let delta = if self.bump() == Tok::Inc { 1 } else { -1 };
                let e = self.parse_unary()?;
                if !is_lvalue(&e) {
                    return self.error("operand of ++/-- is not assignable");
                }
                Ok(Expr {
                    kind: ExprKind::IncDec {
                        target: Box::new(e),
                        delta,
                        prefix: true,
                    },
                    pos,
                })
            }
            Tok::LParen
                if self.is_type_name(self.peek_at(1)) && *self.peek_at(2) == Tok::RParen =>
            {
                self.bump();
                let ty = self.parse_type()?;
                self.expect(Tok::RParen)?;
                let e = self.parse_unary()?;
                Ok(Expr {
                    kind: ExprKind::Cast(ty, Box::new(e)),
                    pos,
                })
            }
            _ => self.parse_postfix(),
        }
    }

    fn parse_postfix(&mut self) -> PResult<Expr> {
        let mut e = self.parse_primary()?;
        loop {
            let pos = e.pos;
            match self.peek().clone() {
                Tok::Dot => {
                    self.bump();
                    let bit = match self.bump() {
                        Tok::Int(b) if (0..8).contains(&b) => b as u8,
                        _ => return self.error("expected bit number 0..7 after '.'"),
                    };
                    if !is_lvalue(&e) {
                        return self.error("bit access requires a variable or array element");
                    }
                    e = Expr {
                        kind: ExprKind::Bit(Box::new(e), bit),
                        pos,
                    };
                }
                Tok::Inc | Tok::Dec => {
                    let delta = if self.bump() == Tok::Inc { 1 } else { -1 };
                    if !is_lvalue(&e) {
                        return self.error("operand of ++/-- is not assignable");
                    }
                    e = Expr {
                        kind: ExprKind::IncDec {
                            target: Box::new(e),
                            delta,
                            prefix: false,
                        },
                        pos,
                    };
                }
                _ => return Ok(e),
            }
        }
    }

    fn parse_primary(&mut self) -> PResult<Expr> {
        let pos = self.here();
        match self.bump() {
            Tok::Int(v) => Ok(Expr {
                kind: ExprKind::Int(v),
                pos,
            }),
            Tok::LParen => {
                let e = self.parse_expr()?;
                self.expect(Tok::RParen)?;
                Ok(e)
            }
            Tok::Ident(name) => match name.as_str() {
                "true" => Ok(Expr {
                    kind: ExprKind::Bool(true),
                    pos,
                }),
                "false" => Ok(Expr {
                    kind: ExprKind::Bool(false),
                    pos,
                }),
                _ => {
                    if self.eat(&Tok::LParen) {
                        let mut args = Vec::new();
                        if !self.eat(&Tok::RParen) {
                            loop {
                                args.push(self.parse_assignment()?);
                                if !self.eat(&Tok::Comma) {
                                    break;
                                }
                            }
                            self.expect(Tok::RParen)?;
                        }
                        return Ok(Expr {
                            kind: ExprKind::Call(name, args),
                            pos,
                        });
                    }
                    if self.eat(&Tok::LBracket) {
                        let idx = self.parse_expr()?;
                        self.expect(Tok::RBracket)?;
                        return Ok(Expr {
                            kind: ExprKind::Index(name, Box::new(idx)),
                            pos,
                        });
                    }
                    Ok(Expr {
                        kind: ExprKind::Var(name),
                        pos,
                    })
                }
            },
            other => Err(ParseError {
                line: pos.line,
                col: pos.col,
                message: format!("unexpected {} in expression", other),
            }),
        }
    }
}

pub fn is_lvalue(e: &Expr) -> bool {
    matches!(
        e.kind,
        ExprKind::Var(_) | ExprKind::Index(..) | ExprKind::Bit(..)
    )
}

/// Fold an expression made only of literals and operators.
pub fn fold_const(e: &Expr) -> Option<i64> {
    Some(match &e.kind {
        ExprKind::Int(v) => *v,
        ExprKind::Bool(b) => *b as i64,
        ExprKind::Unary(op, x) => {
            let v = fold_const(x)?;
            match op {
                UnOp::Neg => v.wrapping_neg(),
                UnOp::Not => !v,
                UnOp::LNot => (v == 0) as i64,
            }
        }
        ExprKind::Binary(op, a, b) => {
            let x = fold_const(a)?;
            let y = fold_const(b)?;
            match op {
                BinOp::Add => x.wrapping_add(y),
                BinOp::Sub => x.wrapping_sub(y),
                BinOp::Mul => x.wrapping_mul(y),
                BinOp::Div => {
                    if y == 0 {
                        return None;
                    }
                    x / y
                }
                BinOp::Rem => {
                    if y == 0 {
                        return None;
                    }
                    x % y
                }
                BinOp::And => x & y,
                BinOp::Or => x | y,
                BinOp::Xor => x ^ y,
                BinOp::Shl => x.wrapping_shl(y as u32),
                BinOp::Shr => x.wrapping_shr(y as u32),
                BinOp::Eq => (x == y) as i64,
                BinOp::Ne => (x != y) as i64,
                BinOp::Lt => (x < y) as i64,
                BinOp::Le => (x <= y) as i64,
                BinOp::Gt => (x > y) as i64,
                BinOp::Ge => (x >= y) as i64,
                BinOp::LAnd => (x != 0 && y != 0) as i64,
                BinOp::LOr => (x != 0 || y != 0) as i64,
            }
        }
        ExprKind::Cast(t, x) => t.wrap(fold_const(x)?),
        ExprKind::Ternary(c, a, b) => {
            if fold_const(c)? != 0 {
                fold_const(a)?
            } else {
                fold_const(b)?
            }
        }
        _ => return None,
    })
}
