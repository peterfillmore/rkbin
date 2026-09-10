//! Two-pass assembler for Holtek HT-IDE style assembly source.
//!
//! Supported syntax (case-insensitive):
//!
//! ```text
//! ; comment
//! label:      mov   a, 05h          ; immediate
//!             mov   a, [20h]        ; direct data memory
//!             mov   tblp, a         ; memory symbol (SFR names are predefined)
//!             set   pa.3            ; bit operand
//!             sz    z               ; predefined bit symbol (STATUS.2)
//!             jmp   label
//!             ret   a, 10h
//!             clr   wdt
//! counter     equ   [80h]           ; memory symbol
//! ten         equ   10              ; numeric symbol
//! flag        equ   [81h].0         ; bit symbol
//!             org   0F00h
//!             dc    00Ah, 00Bh, "text"   ; program-memory words
//! ```
//!
//! A `.section 'data'` (or `.data`) switches to RAM allocation mode where
//! `name db ?`, `name dw ?`, `name ds n` reserve data memory starting at 80h;
//! `.section 'code'` (or `.code`) switches back.

mod expr;
mod lexer;

use std::collections::HashMap;
use std::fmt;

use crate::device;
use crate::hex::Image;
use crate::isa::{Instruction, Op};

pub use lexer::{Token, TokenKind};

/// An assembler diagnostic with source position.
#[derive(Debug, Clone)]
pub struct AsmError {
    pub file: String,
    pub line: usize,
    pub message: String,
}

impl fmt::Display for AsmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: error: {}", self.file, self.line, self.message)
    }
}

impl std::error::Error for AsmError {}

/// Kind of a symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymKind {
    /// Plain number (also used for program labels).
    Num,
    /// Data memory address.
    Mem,
    /// Bit of a data memory address; the value is the address.
    Bit(u8),
}

#[derive(Debug, Clone, Copy)]
pub struct Symbol {
    pub value: i64,
    pub kind: SymKind,
}

/// One line of the assembled listing.
#[derive(Debug, Clone)]
pub struct ListingLine {
    pub line: usize,
    pub address: Option<u16>,
    pub words: Vec<u16>,
    pub source: String,
}

/// Result of a successful assembly.
pub struct Assembled {
    pub image: Image,
    pub symbols: HashMap<String, Symbol>,
    pub listing: Vec<ListingLine>,
    /// One past the highest RAM address reserved in the data section (>= 0x80).
    pub ram_end: u16,
}

/// Source file provider used for `include` directives.
pub trait IncludeResolver {
    fn read(&self, name: &str) -> Option<String>;
}

/// Resolver that refuses every include.
pub struct NoIncludes;

impl IncludeResolver for NoIncludes {
    fn read(&self, _name: &str) -> Option<String> {
        None
    }
}

/// Resolver that reads includes from the file system relative to a directory.
pub struct FsIncludes {
    pub base: std::path::PathBuf,
}

impl IncludeResolver for FsIncludes {
    fn read(&self, name: &str) -> Option<String> {
        std::fs::read_to_string(self.base.join(name)).ok()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Code,
    Data,
}

/// A parsed operand.
#[derive(Debug, Clone, PartialEq)]
enum Arg {
    /// The accumulator keyword `a`.
    Acc,
    /// `wdt`, `wdt1`, `wdt2`
    Wdt(u8),
    /// `[expr]` or a memory symbol.
    Mem(i64),
    /// Bit operand.
    Bit(i64, i64),
    /// Numeric expression (immediate or program address).
    Num(i64),
    /// Unresolved (pass 1 only).
    Unknown,
}

struct Line<'a> {
    file: &'a str,
    line: usize,
    text: &'a str,
}

pub struct Assembler<'r> {
    symbols: HashMap<String, Symbol>,
    resolver: &'r dyn IncludeResolver,
    section: Section,
    pc: i64,
    ram: i64,
    ram_end: i64,
    pass: u8,
    image: Image,
    listing: Vec<ListingLine>,
    errors: Vec<AsmError>,
    include_depth: usize,
}

impl<'r> Assembler<'r> {
    pub fn new(resolver: &'r dyn IncludeResolver) -> Self {
        let mut symbols = HashMap::new();
        for &(name, addr) in device::SFR_TABLE {
            symbols.insert(
                name.to_ascii_lowercase(),
                Symbol {
                    value: addr as i64,
                    kind: SymKind::Mem,
                },
            );
        }
        for &(name, reg, bit) in device::BIT_TABLE {
            symbols.insert(
                name.to_ascii_lowercase(),
                Symbol {
                    value: reg as i64,
                    kind: SymKind::Bit(bit),
                },
            );
        }
        Assembler {
            symbols,
            resolver,
            section: Section::Code,
            pc: 0,
            ram: device::GP_RAM_START as i64,
            ram_end: device::GP_RAM_START as i64,
            pass: 1,
            image: Image::new(),
            listing: Vec::new(),
            errors: Vec::new(),
            include_depth: 0,
        }
    }

    /// Assemble a complete source text.
    pub fn assemble(mut self, file: &str, source: &str) -> Result<Assembled, Vec<AsmError>> {
        for pass in 1..=2 {
            self.pass = pass;
            self.section = Section::Code;
            self.pc = 0;
            self.ram = device::GP_RAM_START as i64;
            self.listing.clear();
            self.errors.clear();
            self.image = Image::new();
            self.assemble_text(file, source);
            if !self.errors.is_empty() && pass == 2 {
                return Err(self.errors);
            }
        }
        if !self.errors.is_empty() {
            return Err(self.errors);
        }
        Ok(Assembled {
            image: self.image,
            symbols: self.symbols,
            listing: self.listing,
            ram_end: self.ram_end as u16,
        })
    }

    fn assemble_text(&mut self, file: &str, source: &str) {
        for (idx, text) in source.lines().enumerate() {
            let line = Line {
                file,
                line: idx + 1,
                text,
            };
            let addr_before = self.pc;
            let words_before = self.image.end();
            let start_listing = self.listing.len();
            self.assemble_line(&line);
            if self.pass == 2 && self.listing.len() == start_listing {
                // Nothing pushed by an include; record this line.
                let mut words = Vec::new();
                if self.section == Section::Code {
                    let end = self.image.end().max(words_before);
                    let from = addr_before as usize;
                    if self.pc as usize > from {
                        for a in from..(self.pc as usize).min(end) {
                            words.push(self.image.get(a as u16));
                        }
                    }
                }
                self.listing.push(ListingLine {
                    line: line.line,
                    address: if self.section == Section::Code {
                        Some(addr_before as u16)
                    } else {
                        None
                    },
                    words,
                    source: text.to_string(),
                });
            }
        }
    }

    fn error(&mut self, line: &Line, msg: impl Into<String>) {
        self.errors.push(AsmError {
            file: line.file.to_string(),
            line: line.line,
            message: msg.into(),
        });
    }

    fn assemble_line(&mut self, line: &Line) {
        let tokens = match lexer::tokenize(line.text) {
            Ok(t) => t,
            Err(e) => {
                self.error(line, e);
                return;
            }
        };
        if tokens.is_empty() {
            return;
        }
        let mut pos = 0;
        // Label?  `name:` or `name <directive>`
        if let TokenKind::Ident(name) = &tokens[0].kind {
            let lname = name.to_ascii_lowercase();
            let next_is_colon = matches!(tokens.get(1).map(|t| &t.kind), Some(TokenKind::Colon));
            let next_is_defining = matches!(
                tokens.get(1).map(|t| &t.kind),
                Some(TokenKind::Ident(d)) if is_defining_directive(d)
            );
            if next_is_colon {
                self.define_label(line, &lname);
                pos = 2;
            } else if next_is_defining {
                let d = if let TokenKind::Ident(d) = &tokens[1].kind {
                    d.to_ascii_lowercase()
                } else {
                    unreachable!()
                };
                self.directive_with_name(line, &lname, &d, &tokens[2..]);
                return;
            }
        }
        if pos >= tokens.len() {
            return;
        }
        let mnemonic = match &tokens[pos].kind {
            TokenKind::Ident(s) => s.to_ascii_lowercase(),
            TokenKind::Dot => {
                // `.section`, `.data`, `.code`, `.list` ...
                if let Some(TokenKind::Ident(s)) = tokens.get(pos + 1).map(|t| &t.kind) {
                    let d = format!(".{}", s.to_ascii_lowercase());
                    self.directive(line, &d, &tokens[pos + 2..]);
                    return;
                }
                self.error(line, "unexpected '.'");
                return;
            }
            other => {
                self.error(line, format!("unexpected token {:?}", other));
                return;
            }
        };
        let rest = &tokens[pos + 1..];
        if is_directive(&mnemonic) {
            self.directive(line, &mnemonic, rest);
        } else if is_mnemonic(&mnemonic) {
            self.instruction(line, &mnemonic, rest);
        } else {
            self.error(
                line,
                format!("unknown mnemonic or directive '{}'", mnemonic),
            );
        }
    }

    fn define_label(&mut self, line: &Line, name: &str) {
        let (value, kind) = match self.section {
            Section::Code => (self.pc, SymKind::Num),
            Section::Data => (self.ram, SymKind::Mem),
        };
        self.define(line, name, Symbol { value, kind });
    }

    fn define(&mut self, line: &Line, name: &str, sym: Symbol) {
        if self.pass == 1 {
            if let Some(existing) = self.symbols.get(name) {
                if existing.value != sym.value || existing.kind != sym.kind {
                    // Redefinition with a different value is an error, except
                    // for predefined device names being shadowed identically.
                    self.error(line, format!("symbol '{}' already defined", name));
                    return;
                }
            }
        }
        self.symbols.insert(name.to_string(), sym);
    }

    fn split_args<'t>(&self, tokens: &'t [Token]) -> Vec<&'t [Token]> {
        let mut args = Vec::new();
        let mut start = 0;
        let mut depth = 0i32;
        for (i, t) in tokens.iter().enumerate() {
            match t.kind {
                TokenKind::LParen | TokenKind::LBracket => depth += 1,
                TokenKind::RParen | TokenKind::RBracket => depth -= 1,
                TokenKind::Comma if depth == 0 => {
                    args.push(&tokens[start..i]);
                    start = i + 1;
                }
                _ => {}
            }
        }
        if start < tokens.len() || !tokens.is_empty() {
            args.push(&tokens[start..]);
        }
        args
    }

    fn directive_with_name(&mut self, line: &Line, name: &str, directive: &str, rest: &[Token]) {
        match directive {
            "equ" | "=" => {
                let args = self.split_args(rest);
                if args.len() != 1 || args[0].is_empty() {
                    self.error(line, "equ needs exactly one value");
                    return;
                }
                match self.parse_arg(line, args[0]) {
                    Arg::Mem(a) => self.define(
                        line,
                        name,
                        Symbol {
                            value: a,
                            kind: SymKind::Mem,
                        },
                    ),
                    Arg::Bit(a, b) => {
                        if !(0..8).contains(&b) {
                            self.error(line, "bit number must be 0..7");
                            return;
                        }
                        self.define(
                            line,
                            name,
                            Symbol {
                                value: a,
                                kind: SymKind::Bit(b as u8),
                            },
                        )
                    }
                    Arg::Num(v) => self.define(
                        line,
                        name,
                        Symbol {
                            value: v,
                            kind: SymKind::Num,
                        },
                    ),
                    Arg::Unknown => {
                        if self.pass == 2 {
                            self.error(line, "cannot resolve equ value");
                        }
                    }
                    _ => self.error(line, "invalid equ value"),
                }
            }
            "db" | "dw" | "ds" => {
                if self.section == Section::Data {
                    let size = match directive {
                        "db" => 1,
                        "dw" => 2,
                        _ => {
                            let args = self.split_args(rest);
                            match args.first().map(|a| self.parse_arg(line, a)) {
                                Some(Arg::Num(n)) if n >= 0 => n,
                                _ => {
                                    self.error(line, "ds needs a size");
                                    return;
                                }
                            }
                        }
                    };
                    let addr = self.ram;
                    self.define(
                        line,
                        name,
                        Symbol {
                            value: addr,
                            kind: SymKind::Mem,
                        },
                    );
                    self.reserve(line, size);
                } else {
                    self.define_label(line, name);
                    self.directive(line, directive, rest);
                }
            }
            "dc" => {
                self.define_label(line, name);
                self.directive(line, directive, rest);
            }
            _ => {
                // `.section` style with a leading name: `code .section at 0 'code'`
                self.directive(line, directive, rest);
            }
        }
    }

    fn reserve(&mut self, line: &Line, size: i64) {
        let count = size.max(0);
        if self.ram + count > 0x100 {
            self.error(
                line,
                format!("data memory overflow: {} bytes at {:02x}h", count, self.ram),
            );
        }
        self.ram += count;
        if self.ram > self.ram_end {
            self.ram_end = self.ram;
        }
    }

    fn directive(&mut self, line: &Line, directive: &str, rest: &[Token]) {
        match directive {
            "org" => {
                let args = self.split_args(rest);
                let v = match args.first().map(|a| self.parse_arg(line, a)) {
                    Some(Arg::Num(v)) | Some(Arg::Mem(v)) => v,
                    _ => {
                        if self.pass == 2 {
                            self.error(line, "org needs a resolvable address");
                        }
                        return;
                    }
                };
                match self.section {
                    Section::Code => {
                        if !(0..=device::PROGRAM_LAST as i64).contains(&v) {
                            self.error(
                                line,
                                format!("org address {:x}h outside program memory", v),
                            );
                            return;
                        }
                        self.pc = v;
                    }
                    Section::Data => {
                        if !(0..=0xFF).contains(&v) {
                            self.error(line, format!("org address {:x}h outside data memory", v));
                            return;
                        }
                        self.ram = v;
                    }
                }
            }
            "dc" | "dw" | "db" => {
                if self.section == Section::Data {
                    let size = if directive == "dw" { 2 } else { 1 };
                    let n = self.split_args(rest).len().max(1) as i64;
                    self.reserve(line, size * n);
                    return;
                }
                let args = self.split_args(rest);
                for a in args {
                    if a.len() == 1 {
                        if let TokenKind::Str(s) = &a[0].kind {
                            for b in s.bytes() {
                                self.emit_word(line, b as u16);
                            }
                            continue;
                        }
                    }
                    match self.parse_arg(line, a) {
                        Arg::Num(v) | Arg::Mem(v) => {
                            if !(-0x8000..=0xFFFF).contains(&v) {
                                self.error(line, format!("value {} does not fit in 16 bits", v));
                            }
                            self.emit_word(line, v as u16);
                        }
                        Arg::Unknown => self.emit_word(line, 0),
                        _ => self.error(line, "invalid data value"),
                    }
                }
            }
            "ds" => {
                let args = self.split_args(rest);
                let n = match args.first().map(|a| self.parse_arg(line, a)) {
                    Some(Arg::Num(n)) => n,
                    _ => {
                        self.error(line, "ds needs a size");
                        return;
                    }
                };
                match self.section {
                    Section::Data => self.reserve(line, n),
                    Section::Code => {
                        for _ in 0..n {
                            self.emit_word(line, 0);
                        }
                    }
                }
            }
            ".section" | "section" => {
                // `.section 'data'` / `.section at 0 'code'`
                let is_data = rest.iter().any(
                    |t| matches!(&t.kind, TokenKind::Str(s) if s.eq_ignore_ascii_case("data")),
                );
                self.section = if is_data {
                    Section::Data
                } else {
                    Section::Code
                };
            }
            ".data" => self.section = Section::Data,
            ".code" | ".text" => self.section = Section::Code,
            "include" | ".include" | "#include" => {
                let name = match rest.first().map(|t| &t.kind) {
                    Some(TokenKind::Str(s)) => s.clone(),
                    Some(TokenKind::Ident(s)) => {
                        // include file.inc (unquoted)
                        let mut s = s.clone();
                        for t in &rest[1..] {
                            match &t.kind {
                                TokenKind::Dot => s.push('.'),
                                TokenKind::Ident(x) => s.push_str(x),
                                _ => {}
                            }
                        }
                        s
                    }
                    _ => {
                        self.error(line, "include needs a file name");
                        return;
                    }
                };
                if self.include_depth > 16 {
                    self.error(line, "include nesting too deep");
                    return;
                }
                match self.resolver.read(&name) {
                    Some(text) => {
                        self.include_depth += 1;
                        self.assemble_text(&name, &text);
                        self.include_depth -= 1;
                    }
                    None => self.error(line, format!("cannot read include file '{}'", name)),
                }
            }
            "end" | "public" | "extern" | ".list" | ".nolist" | "list" | "nolist" | "rom"
            | "ram" | "device" | "chip" => {}
            _ => self.error(line, format!("unknown directive '{}'", directive)),
        }
    }

    fn emit_word(&mut self, line: &Line, word: u16) {
        if self.pc > device::PROGRAM_LAST as i64 {
            if self.pass == 2 {
                self.error(line, "program memory overflow (4096 words)");
            }
            self.pc += 1;
            return;
        }
        if self.pass == 2 {
            if self.image.is_used(self.pc as u16) {
                self.error(
                    line,
                    format!("program address {:03x}h assembled twice", self.pc),
                );
            }
            self.image.set(self.pc as u16, word);
        }
        self.pc += 1;
    }

    /// Parse one operand.
    fn parse_arg(&mut self, line: &Line, tokens: &[Token]) -> Arg {
        if tokens.is_empty() {
            self.error(line, "missing operand");
            return Arg::Unknown;
        }
        // Register keywords.
        if tokens.len() == 1 {
            if let TokenKind::Ident(s) = &tokens[0].kind {
                match s.to_ascii_lowercase().as_str() {
                    "a" => return Arg::Acc,
                    "wdt" => return Arg::Wdt(0),
                    "wdt1" => return Arg::Wdt(1),
                    "wdt2" => return Arg::Wdt(2),
                    _ => {}
                }
            }
        }
        // `[expr]` or `[expr].bit`
        if tokens[0].kind == TokenKind::LBracket {
            let close = match matching_bracket(tokens, 0) {
                Some(c) => c,
                None => {
                    self.error(line, "missing ']'");
                    return Arg::Unknown;
                }
            };
            let addr = match self.eval(line, &tokens[1..close]) {
                Some(v) => v,
                None => return Arg::Unknown,
            };
            let rest = &tokens[close + 1..];
            if rest.is_empty() {
                return Arg::Mem(addr);
            }
            if rest[0].kind == TokenKind::Dot {
                return match self.eval(line, &rest[1..]) {
                    Some(b) => Arg::Bit(addr, b),
                    None => Arg::Unknown,
                };
            }
            self.error(line, "unexpected tokens after ']'");
            return Arg::Unknown;
        }
        // `symbol.bit` where symbol is a memory symbol.
        if tokens.len() >= 3 && tokens[1].kind == TokenKind::Dot {
            if let TokenKind::Ident(name) = &tokens[0].kind {
                if let Some(sym) = self.symbols.get(&name.to_ascii_lowercase()).copied() {
                    if sym.kind == SymKind::Mem {
                        return match self.eval(line, &tokens[2..]) {
                            Some(b) => Arg::Bit(sym.value, b),
                            None => Arg::Unknown,
                        };
                    }
                }
            }
        }
        // A single memory or bit symbol.
        if tokens.len() == 1 {
            if let TokenKind::Ident(name) = &tokens[0].kind {
                match self.symbols.get(&name.to_ascii_lowercase()).copied() {
                    Some(Symbol {
                        value,
                        kind: SymKind::Mem,
                    }) => return Arg::Mem(value),
                    Some(Symbol {
                        value,
                        kind: SymKind::Bit(b),
                    }) => return Arg::Bit(value, b as i64),
                    _ => {}
                }
            }
        }
        match self.eval(line, tokens) {
            Some(v) => Arg::Num(v),
            None => Arg::Unknown,
        }
    }

    /// Evaluate a numeric expression; `None` if unresolved (only tolerated in pass 1).
    fn eval(&mut self, line: &Line, tokens: &[Token]) -> Option<i64> {
        let pc = self.pc;
        let mut parser = expr::ExprParser::new(tokens, &self.symbols, pc);
        match parser.parse() {
            Ok(v) => Some(v),
            Err(expr::ExprError::Undefined(name)) => {
                if self.pass == 2 {
                    self.error(line, format!("undefined symbol '{}'", name));
                }
                None
            }
            Err(expr::ExprError::Syntax(msg)) => {
                self.error(line, msg);
                None
            }
        }
    }

    fn instruction(&mut self, line: &Line, mnemonic: &str, rest: &[Token]) {
        let arg_tokens = self.split_args(rest);
        let mut args = Vec::new();
        for a in &arg_tokens {
            if a.is_empty() {
                self.error(line, "empty operand");
                self.emit_word(line, 0);
                return;
            }
            args.push(self.parse_arg(line, a));
        }
        if args.contains(&Arg::Unknown) {
            // Unresolved in pass 1: reserve the word.
            self.emit_word(line, 0);
            return;
        }
        match self.select(mnemonic, &args) {
            Ok(ins) => match ins.encode() {
                Ok(w) => self.emit_word(line, w),
                Err(e) => {
                    self.error(line, e.to_string());
                    self.emit_word(line, 0);
                }
            },
            Err(msg) => {
                self.error(line, msg);
                self.emit_word(line, 0);
            }
        }
    }

    fn select(&self, mnemonic: &str, args: &[Arg]) -> Result<Instruction, String> {
        let mem = |op: Op, v: i64| -> Result<Instruction, String> {
            if !(0..=0xFF).contains(&v) {
                return Err(format!("data address {:x}h out of range 00h..FFh", v));
            }
            Ok(Instruction::mem(op, v as u8))
        };
        let imm = |op: Op, v: i64| -> Result<Instruction, String> {
            if !(-128..=0xFF).contains(&v) {
                return Err(format!("immediate {} does not fit in 8 bits", v));
            }
            Ok(Instruction::imm(op, (v & 0xFF) as u8))
        };
        let bit = |op: Op, m: i64, b: i64| -> Result<Instruction, String> {
            if !(0..=0xFF).contains(&m) {
                return Err(format!("data address {:x}h out of range 00h..FFh", m));
            }
            if !(0..8).contains(&b) {
                return Err(format!("bit number {} out of range 0..7", b));
            }
            Ok(Instruction::bit(op, m as u8, b as u8))
        };
        let addr = |op: Op, v: i64| -> Result<Instruction, String> {
            if !(0..=device::PROGRAM_LAST as i64).contains(&v) {
                return Err(format!("program address {:x}h out of range", v));
            }
            Ok(Instruction::addr(op, v as u16))
        };
        let bad = || Err(format!("invalid operands for '{}'", mnemonic));

        match (mnemonic, args) {
            ("nop", []) => Ok(Instruction::simple(Op::Nop)),
            ("halt", []) => Ok(Instruction::simple(Op::Halt)),
            ("ret", []) => Ok(Instruction::simple(Op::Ret)),
            ("ret", [Arg::Acc, Arg::Num(x)]) => imm(Op::RetAI, *x),
            ("reti", []) => Ok(Instruction::simple(Op::Reti)),
            ("clr", [Arg::Wdt(0)]) => Ok(Instruction::simple(Op::ClrWdt)),
            ("clr", [Arg::Wdt(1)]) => Ok(Instruction::simple(Op::ClrWdt1)),
            ("clr", [Arg::Wdt(2)]) => Ok(Instruction::simple(Op::ClrWdt2)),
            ("clr", [Arg::Mem(m)]) => mem(Op::Clr, *m),
            ("clr", [Arg::Bit(m, b)]) => bit(Op::ClrBit, *m, *b),
            ("set", [Arg::Mem(m)]) => mem(Op::Set, *m),
            ("set", [Arg::Bit(m, b)]) => bit(Op::SetBit, *m, *b),
            ("sz", [Arg::Mem(m)]) => mem(Op::Sz, *m),
            ("sz", [Arg::Bit(m, b)]) => bit(Op::SzBit, *m, *b),
            ("snz", [Arg::Bit(m, b)]) => bit(Op::SnzBit, *m, *b),
            ("mov", [Arg::Acc, Arg::Mem(m)]) => mem(Op::MovAM, *m),
            ("mov", [Arg::Acc, Arg::Num(x)]) => imm(Op::MovAI, *x),
            ("mov", [Arg::Mem(m), Arg::Acc]) => mem(Op::MovMA, *m),
            ("add", [Arg::Acc, Arg::Mem(m)]) => mem(Op::Add, *m),
            ("add", [Arg::Acc, Arg::Num(x)]) => imm(Op::AddI, *x),
            ("sub", [Arg::Acc, Arg::Mem(m)]) => mem(Op::Sub, *m),
            ("sub", [Arg::Acc, Arg::Num(x)]) => imm(Op::SubI, *x),
            ("and", [Arg::Acc, Arg::Mem(m)]) => mem(Op::And, *m),
            ("and", [Arg::Acc, Arg::Num(x)]) => imm(Op::AndI, *x),
            ("or", [Arg::Acc, Arg::Mem(m)]) => mem(Op::Or, *m),
            ("or", [Arg::Acc, Arg::Num(x)]) => imm(Op::OrI, *x),
            ("xor", [Arg::Acc, Arg::Mem(m)]) => mem(Op::Xor, *m),
            ("xor", [Arg::Acc, Arg::Num(x)]) => imm(Op::XorI, *x),
            ("addm", [Arg::Acc, Arg::Mem(m)]) => mem(Op::Addm, *m),
            ("subm", [Arg::Acc, Arg::Mem(m)]) => mem(Op::Subm, *m),
            ("andm", [Arg::Acc, Arg::Mem(m)]) => mem(Op::Andm, *m),
            ("orm", [Arg::Acc, Arg::Mem(m)]) => mem(Op::Orm, *m),
            ("xorm", [Arg::Acc, Arg::Mem(m)]) => mem(Op::Xorm, *m),
            ("adc", [Arg::Acc, Arg::Mem(m)]) => mem(Op::Adc, *m),
            ("adcm", [Arg::Acc, Arg::Mem(m)]) => mem(Op::Adcm, *m),
            ("sbc", [Arg::Acc, Arg::Mem(m)]) => mem(Op::Sbc, *m),
            ("sbcm", [Arg::Acc, Arg::Mem(m)]) => mem(Op::Sbcm, *m),
            ("jmp", [Arg::Num(a)]) => addr(Op::Jmp, *a),
            ("call", [Arg::Num(a)]) => addr(Op::Call, *a),
            (_, [Arg::Mem(m)]) | (_, [Arg::Num(m)]) => {
                let op = match mnemonic {
                    "cpla" => Op::Cpla,
                    "cpl" => Op::Cpl,
                    "sza" => Op::Sza,
                    "swapa" => Op::Swapa,
                    "swap" => Op::Swap,
                    "inca" => Op::Inca,
                    "inc" => Op::Inc,
                    "deca" => Op::Deca,
                    "dec" => Op::Dec,
                    "siza" => Op::Siza,
                    "siz" => Op::Siz,
                    "sdza" => Op::Sdza,
                    "sdz" => Op::Sdz,
                    "rla" => Op::Rla,
                    "rl" => Op::Rl,
                    "rra" => Op::Rra,
                    "rr" => Op::Rr,
                    "rlca" => Op::Rlca,
                    "rlc" => Op::Rlc,
                    "rrca" => Op::Rrca,
                    "rrc" => Op::Rrc,
                    "tabrd" => Op::Tabrd,
                    "tabrdc" => Op::Tabrdc,
                    "tabrdl" => Op::Tabrdl,
                    "daa" => Op::Daa,
                    _ => return bad(),
                };
                mem(op, *m)
            }
            _ => bad(),
        }
    }
}

fn matching_bracket(tokens: &[Token], open: usize) -> Option<usize> {
    let mut depth = 0;
    for (i, t) in tokens.iter().enumerate().skip(open) {
        match t.kind {
            TokenKind::LBracket => depth += 1,
            TokenKind::RBracket => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

const MNEMONICS: &[&str] = &[
    "nop", "halt", "ret", "reti", "clr", "set", "sz", "snz", "mov", "add", "sub", "and", "or",
    "xor", "addm", "subm", "andm", "orm", "xorm", "adc", "adcm", "sbc", "sbcm", "jmp", "call",
    "cpla", "cpl", "sza", "swapa", "swap", "inca", "inc", "deca", "dec", "siza", "siz", "sdza",
    "sdz", "rla", "rl", "rra", "rr", "rlca", "rlc", "rrca", "rrc", "tabrd", "tabrdc", "tabrdl",
    "daa",
];

const DIRECTIVES: &[&str] = &[
    "org", "dc", "dw", "db", "ds", "equ", "end", "include", "#include", ".include", "public",
    "extern", ".section", "section", ".data", ".code", ".text", ".list", ".nolist", "list",
    "nolist", "rom", "ram", "device", "chip",
];

fn is_mnemonic(s: &str) -> bool {
    MNEMONICS.contains(&s)
}

fn is_directive(s: &str) -> bool {
    DIRECTIVES.contains(&s)
}

fn is_defining_directive(s: &str) -> bool {
    let s = s.to_ascii_lowercase();
    matches!(s.as_str(), "equ" | "db" | "dw" | "ds" | "dc")
}

/// Convenience: assemble a source string without include support.
pub fn assemble_str(file: &str, source: &str) -> Result<Assembled, Vec<AsmError>> {
    Assembler::new(&NoIncludes).assemble(file, source)
}

/// Render a listing as text.
pub fn format_listing(listing: &[ListingLine]) -> String {
    let mut out = String::new();
    for l in listing {
        let addr = l
            .address
            .map(|a| format!("{:04X}", a))
            .unwrap_or_else(|| "    ".into());
        let words: Vec<String> = l.words.iter().map(|w| format!("{:04X}", w)).collect();
        out.push_str(&format!(
            "{:5} {} {:<9} {}\n",
            l.line,
            addr,
            words.join(" "),
            l.source
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asm(src: &str) -> Assembled {
        match assemble_str("test.asm", src) {
            Ok(a) => a,
            Err(errs) => panic!(
                "{}",
                errs.iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
        }
    }

    #[test]
    fn listing_file_encodings() {
        let a = asm("
            mov a,low (offset startup_value_1)
            mov tblp,a
            mov a,high (offset startup_value_1)
            mov tbhp,a
          next_table:
            inc tblp
            sz z
            inc tbhp
            tabrd mp0
            sz mp0
            jmp read_data
            jmp next_table
          read_data:
            mov a,tblh
            clr wdt2
            orm a,[32]
            sdz acc
            org 0f06h
          startup_value_1:
            dc 1234h
        ");
        let w: Vec<u16> = (0..13).map(|i| a.image.get(i)).collect();
        assert_eq!(
            w,
            vec![
                0x0F06, 0x0087, 0x0F0F, 0x0089, 0x1487, 0x3D0A, 0x1489, 0x1D01, 0x1081, 0x280B,
                0x2804, 0x0708, 0x0005
            ]
        );
        assert_eq!(a.image.get(13), 0x05A0);
        assert_eq!(a.image.get(14), 0x1785);
        assert_eq!(a.image.get(0xF06), 0x1234);
    }

    #[test]
    fn data_section_and_symbols() {
        let a = asm("
            .section 'data'
            counter db ?
            total   dw ?
            buf     ds 4
            .section 'code'
            flag equ counter.3
            ten  equ 10
            mov a, ten
            mov counter, a
            set flag
            clr [buf+2]
            mov a, [total+1]
            add a, ten*2+1
            ret a, 'A'
            snz [0f0h].7
            jmp $
        ");
        assert_eq!(a.image.get(0), 0x0F0A);
        assert_eq!(a.image.get(1), 0x4080); // mov [80h],a  (bit 14 set)
        assert_eq!(a.image.get(2), 0x7180); // set [80h].3
        assert_eq!(a.image.get(3), 0x5F05); // clr [85h]
        assert_eq!(a.image.get(4), 0x4702); // mov a,[82h]
        assert_eq!(a.image.get(5), 0x0B15);
        assert_eq!(a.image.get(6), 0x0941);
        assert_eq!(a.image.get(7), 0x7BF0); // snz [f0h].7
        assert_eq!(a.image.get(8), 0x2808);
        assert_eq!(a.ram_end, 0x87);
    }

    #[test]
    fn errors_are_reported() {
        let e = match assemble_str("t.asm", "mov a, nosuch\n bogus\n mov [300h], a\n") {
            Err(e) => e,
            Ok(_) => panic!("expected errors"),
        };
        assert_eq!(e.len(), 3);
        assert!(e[0].message.contains("undefined symbol"));
        assert!(e[1].message.contains("unknown mnemonic"));
        assert!(e[2].message.contains("out of range"));
    }
}
