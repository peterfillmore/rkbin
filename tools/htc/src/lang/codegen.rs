//! Code generation: `htc` AST → Holtek assembly text.
//!
//! Memory model
//! ------------
//! * Globals live in bank-0 general purpose RAM (80h..FFh).
//! * Every function owns a statically allocated *frame* holding its
//!   parameters, locals and expression temporaries.  Frames are laid out
//!   with the classic overlay algorithm: a callee's frame starts after the
//!   frames of all of its callers, so no live data is ever shared.  As a
//!   consequence recursion is not allowed (it is diagnosed).
//! * Interrupt handlers get frames beyond everything reachable from `main`.
//! * 8-bit values are computed in ACC; 16-bit values live in memory pairs
//!   (little endian) and are combined byte-wise with ADC/SBC.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::ast::*;
use super::parser::fold_const;
use super::runtime;
use crate::device;

#[derive(Debug, Clone)]
pub struct CompileError {
    pub line: usize,
    pub col: usize,
    pub message: String,
}

type R<T> = Result<T, CompileError>;

fn err<T>(pos: Pos, msg: impl Into<String>) -> R<T> {
    Err(CompileError {
        line: pos.line,
        col: pos.col,
        message: msg.into(),
    })
}

/// Result of a successful compilation.
pub struct Output {
    pub asm: String,
    pub warnings: Vec<String>,
    /// Bytes of bank-0 RAM used (globals, runtime scratch and frames).
    pub ram_used: usize,
    /// Per-function frame sizes and bases for the memory report.
    pub frames: Vec<(String, u8, usize)>,
}

// ---------------------------------------------------------------------------
// Values
// ---------------------------------------------------------------------------

/// Where a value currently is.
#[derive(Debug, Clone, PartialEq)]
enum Loc {
    /// Compile-time constant.
    Imm(i64),
    /// Memory at an assembler address expression (`_x`, `F1+2`, `pa`).
    /// 16-bit values occupy `addr` and `addr+1`.
    Mem(String),
    /// 8-bit value in the accumulator.
    Acc,
    /// Expression temporary in the current frame: (offset, size).
    Temp(usize, usize),
    /// 8-bit value at IAR0 (MP0 already loaded).  Transient.
    Ind,
    /// A bit of a memory byte.
    Bit(String, u8),
    /// A bit of the byte at IAR0.
    BitInd(u8),
}

#[derive(Debug, Clone)]
struct Val {
    loc: Loc,
    ty: Type,
}

impl Val {
    fn new(loc: Loc, ty: Type) -> Self {
        Val { loc, ty }
    }
}

// ---------------------------------------------------------------------------
// Symbols
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum GlobalKind {
    /// Allocated in RAM (or at a fixed address).
    Ram,
    /// `const` scalar: compile-time constant.
    ConstScalar(i64),
    /// `const` array in program memory.
    ConstTable(Vec<i64>),
}

#[derive(Debug, Clone)]
struct Global {
    ty: Type,
    len: Option<usize>,
    sym: String,
    kind: GlobalKind,
    fixed: Option<u8>,
    pos: Pos,
}

#[derive(Debug, Clone)]
struct Local {
    ty: Type,
    len: Option<usize>,
    /// Frame offset, or `None` for a compile-time constant.
    off: Option<usize>,
    constant: Option<i64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Uses {
    mp0: bool,
    table: bool,
    runtime: bool,
}

impl Uses {
    fn union(self, o: Uses) -> Uses {
        Uses {
            mp0: self.mp0 || o.mp0,
            table: self.table || o.table,
            runtime: self.runtime || o.runtime,
        }
    }
}

#[derive(Debug, Clone)]
struct FnInfo {
    id: usize,
    ret: Type,
    params: Vec<Param>,
    isr: Option<u16>,
    calls: BTreeSet<String>,
    frame_size: usize,
    uses: Uses,
    lines: Vec<String>,
    generated: bool,
}

/// Per-function generation state.
struct Cur {
    id: usize,
    ret: Type,
    isr: bool,
    scopes: Vec<HashMap<String, Local>>,
    locals_size: usize,
    locals_max: usize,
    temp_sp: usize,
    temp_max: usize,
    loops: Vec<(String, String)>,
    lines: Vec<String>,
    calls: BTreeSet<String>,
    uses: Uses,
    exit_label: String,
}

pub struct Gen {
    globals: BTreeMap<String, Global>,
    global_order: Vec<String>,
    fns: BTreeMap<String, FnInfo>,
    fn_order: Vec<String>,
    cur: Option<Cur>,
    label_counter: usize,
    runtime_used: BTreeSet<String>,
    scratch_bytes: usize,
    needs_ret16: bool,
    warnings: Vec<String>,
    startup: Vec<String>,
}

fn literal_type(v: i64) -> Type {
    if (0..=255).contains(&v) {
        Type::U8
    } else if (-128..0).contains(&v) {
        Type::I8
    } else if (256..=65535).contains(&v) {
        Type::U16
    } else {
        Type::I16
    }
}

fn contains_call(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Int(_) | ExprKind::Bool(_) | ExprKind::Var(_) => false,
        ExprKind::Index(_, i) => contains_call(i),
        ExprKind::Bit(b, _) => contains_call(b),
        ExprKind::Call(..) => true,
        ExprKind::Unary(_, x) | ExprKind::Cast(_, x) => contains_call(x),
        ExprKind::Binary(_, a, b) | ExprKind::Assign(a, b) | ExprKind::CompoundAssign(_, a, b) => {
            contains_call(a) || contains_call(b)
        }
        ExprKind::IncDec { target, .. } => contains_call(target),
        ExprKind::Ternary(a, b, c) => contains_call(a) || contains_call(b) || contains_call(c),
    }
}

impl Gen {
    pub fn new() -> Self {
        Gen {
            globals: BTreeMap::new(),
            global_order: Vec::new(),
            fns: BTreeMap::new(),
            fn_order: Vec::new(),
            cur: None,
            label_counter: 0,
            runtime_used: BTreeSet::new(),
            scratch_bytes: 0,
            needs_ret16: false,
            warnings: Vec::new(),
            startup: Vec::new(),
        }
    }

    // -----------------------------------------------------------------
    // Small helpers
    // -----------------------------------------------------------------

    fn cur(&mut self) -> &mut Cur {
        self.cur.as_mut().expect("inside a function")
    }

    fn emit(&mut self, line: impl Into<String>) {
        let s = line.into();
        self.cur().lines.push(format!("        {}", s));
    }

    fn emit_label(&mut self, label: &str) {
        self.cur().lines.push(format!("{}:", label));
    }

    fn new_label(&mut self) -> String {
        self.label_counter += 1;
        format!("L{}", self.label_counter)
    }

    fn alloc_temp(&mut self, size: usize) -> Loc {
        let c = self.cur();
        let off = c.temp_sp;
        c.temp_sp += size;
        if c.temp_sp > c.temp_max {
            c.temp_max = c.temp_sp;
        }
        Loc::Temp(off, size)
    }

    /// Free a temporary.  Temporaries are released in LIFO order; freeing a
    /// non-temporary location is a no-op.
    fn free(&mut self, loc: &Loc) {
        if let Loc::Temp(off, _) = loc {
            let c = self.cur();
            if *off < c.temp_sp {
                c.temp_sp = *off;
            }
        }
    }

    /// Address expression text of byte `byte` of a memory location.
    fn addr(&self, loc: &Loc, byte: usize) -> String {
        match loc {
            Loc::Mem(s) => {
                if byte == 0 {
                    format!("[{}]", s)
                } else {
                    format!("[{}+{}]", s, byte)
                }
            }
            Loc::Temp(off, _) => {
                let id = self.cur.as_ref().map(|c| c.id).unwrap_or(0);
                format!("[T{}+{}]", id, off + byte)
            }
            Loc::Ind => {
                assert_eq!(byte, 0, "indirect 16-bit access must be copied");
                "iar0".to_string()
            }
            other => panic!("addr() of non-memory location {:?}", other),
        }
    }

    fn use_runtime(&mut self, name: &str) {
        self.runtime_used.insert(name.to_string());
        self.scratch_bytes = self.scratch_bytes.max(runtime::scratch_bytes(name));
        self.cur().uses.runtime = true;
        self.cur().calls.insert(name.to_string());
    }

    // -----------------------------------------------------------------
    // Symbol lookup
    // -----------------------------------------------------------------

    fn lookup_local(&self, name: &str) -> Option<Local> {
        let c = self.cur.as_ref()?;
        for scope in c.scopes.iter().rev() {
            if let Some(l) = scope.get(name) {
                return Some(l.clone());
            }
        }
        None
    }

    fn frame_addr(&self, off: usize) -> String {
        let id = self.cur.as_ref().map(|c| c.id).unwrap_or(0);
        format!("F{}+{}", id, off)
    }

    /// Resolve a scalar variable name to a location.
    fn resolve_var(&mut self, name: &str, pos: Pos) -> R<Val> {
        if let Some(l) = self.lookup_local(name) {
            if l.len.is_some() {
                return err(pos, format!("array '{}' used without an index", name));
            }
            if let Some(c) = l.constant {
                return Ok(Val::new(Loc::Imm(c), l.ty));
            }
            return Ok(Val::new(Loc::Mem(self.frame_addr(l.off.unwrap())), l.ty));
        }
        if let Some(g) = self.globals.get(name).cloned() {
            if g.len.is_some() {
                return err(pos, format!("array '{}' used without an index", name));
            }
            return match g.kind {
                GlobalKind::ConstScalar(v) => Ok(Val::new(Loc::Imm(v), g.ty)),
                _ => Ok(Val::new(Loc::Mem(g.sym.clone()), g.ty)),
            };
        }
        if let Some(addr) = device::sfr_by_name(name) {
            if name.chars().all(|c| !c.is_ascii_lowercase()) {
                let _ = addr;
                return Ok(Val::new(Loc::Mem(name.to_ascii_lowercase()), Type::U8));
            }
        }
        if let Some((reg, bit)) = device::bit_by_name(name) {
            if name.chars().all(|c| !c.is_ascii_lowercase()) {
                let reg_name = device::sfr_name(reg).unwrap().to_ascii_lowercase();
                return Ok(Val::new(Loc::Bit(reg_name, bit), Type::Bool));
            }
        }
        err(pos, format!("undefined variable '{}'", name))
    }

    // -----------------------------------------------------------------
    // Constant folding (aware of named constants)
    // -----------------------------------------------------------------

    /// Value of a named compile-time constant, if `name` is one.
    fn const_value(&self, name: &str) -> Option<i64> {
        if let Some(l) = self.lookup_local(name) {
            return l.constant;
        }
        match self.globals.get(name) {
            Some(Global {
                kind: GlobalKind::ConstScalar(v),
                ..
            }) => Some(*v),
            _ => None,
        }
    }

    /// Fold a constant expression; like `parser::fold_const` but named
    /// constants (`const u8 N = 8;`) and const-table elements with constant
    /// indices are folded too.
    fn fold(&self, e: &Expr) -> Option<i64> {
        if let Some(v) = fold_const(e) {
            return Some(v);
        }
        Some(match &e.kind {
            ExprKind::Var(name) => self.const_value(name)?,
            ExprKind::Index(name, idx) => {
                let i = self.fold(idx)?;
                match self.globals.get(name) {
                    Some(Global {
                        kind: GlobalKind::ConstTable(vals),
                        ..
                    }) => *vals.get(usize::try_from(i).ok()?)?,
                    _ => return None,
                }
            }
            ExprKind::Unary(op, x) => {
                let v = self.fold(x)?;
                match op {
                    UnOp::Neg => v.wrapping_neg(),
                    UnOp::Not => !v,
                    UnOp::LNot => (v == 0) as i64,
                }
            }
            ExprKind::Binary(op, a, b) => {
                let x = self.fold(a)?;
                let y = self.fold(b)?;
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
            ExprKind::Cast(t, x) => t.wrap(self.fold(x)?),
            ExprKind::Ternary(c, a, b) => {
                if self.fold(c)? != 0 {
                    self.fold(a)?
                } else {
                    self.fold(b)?
                }
            }
            _ => return None,
        })
    }

    /// Resolve the declared length of an array (None for scalars).
    fn array_len_of(&self, d: &VarDecl) -> R<Option<usize>> {
        match &d.array_len_expr {
            None => Ok(d.array_len),
            Some(e) => match self.fold(e) {
                Some(n) if (1..=256).contains(&n) => {
                    if let Some(Initializer::List(items)) = &d.init {
                        if items.len() > n as usize {
                            return err(d.pos, "too many initializers for array");
                        }
                    }
                    Ok(Some(n as usize))
                }
                Some(_) => err(e.pos, "array length must be 1..256"),
                None => err(e.pos, "array length must be a constant expression"),
            },
        }
    }

    // -----------------------------------------------------------------
    // Static type inference (no code emitted)
    // -----------------------------------------------------------------

    /// Type of a constant-foldable expression with value `v`.
    fn const_type(e: &Expr, v: i64) -> Type {
        match &e.kind {
            ExprKind::Bool(_) => Type::Bool,
            ExprKind::Cast(t, _) => *t,
            ExprKind::Binary(op, ..) if op.is_comparison() || op.is_logical() => Type::Bool,
            ExprKind::Unary(UnOp::LNot, _) => Type::Bool,
            _ => literal_type(v),
        }
    }

    fn type_of(&self, e: &Expr) -> R<Type> {
        if let Some(v) = self.fold(e) {
            return Ok(Self::const_type(e, v));
        }
        Ok(match &e.kind {
            ExprKind::Int(v) => literal_type(*v),
            ExprKind::Bool(_) => Type::Bool,
            ExprKind::Var(name) | ExprKind::Index(name, _) => {
                if let Some(l) = self.lookup_local(name) {
                    l.ty
                } else if let Some(g) = self.globals.get(name) {
                    g.ty
                } else if device::sfr_by_name(name).is_some() {
                    Type::U8
                } else if device::bit_by_name(name).is_some() {
                    Type::Bool
                } else {
                    return err(e.pos, format!("undefined variable '{}'", name));
                }
            }
            ExprKind::Bit(..) => Type::Bool,
            ExprKind::Call(name, _) => match name.as_str() {
                "halt" | "nop" | "clr_wdt" | "ei" | "di" | "enable_interrupts"
                | "disable_interrupts" => Type::Void,
                _ => match self.fns.get(name) {
                    Some(f) => f.ret,
                    None => return err(e.pos, format!("undefined function '{}'", name)),
                },
            },
            ExprKind::Unary(UnOp::LNot, _) => Type::Bool,
            ExprKind::Unary(_, x) => {
                let t = self.type_of(x)?;
                if t == Type::Bool {
                    Type::U8
                } else {
                    t
                }
            }
            ExprKind::Binary(op, a, b) => {
                if op.is_comparison() || op.is_logical() {
                    Type::Bool
                } else if matches!(op, BinOp::Shl | BinOp::Shr) {
                    let t = self.type_of(a)?;
                    if t == Type::Bool {
                        Type::U8
                    } else {
                        t
                    }
                } else {
                    Type::common(self.type_of(a)?, self.type_of(b)?)
                }
            }
            ExprKind::Assign(a, _) | ExprKind::CompoundAssign(_, a, _) => self.type_of(a)?,
            ExprKind::IncDec { target, .. } => self.type_of(target)?,
            ExprKind::Cast(t, _) => *t,
            ExprKind::Ternary(_, a, b) => Type::common(self.type_of(a)?, self.type_of(b)?),
        })
    }

    // -----------------------------------------------------------------
    // Loading values
    // -----------------------------------------------------------------

    /// Load the low byte of a value into ACC (bits become 0/1).
    fn load_acc(&mut self, v: &Val) {
        match &v.loc {
            Loc::Imm(x) => self.emit(format!("mov a,{}", x & 0xFF)),
            Loc::Mem(_) | Loc::Temp(..) => {
                let a = self.addr(&v.loc, 0);
                self.emit(format!("mov a,{}", a));
            }
            Loc::Acc => {}
            Loc::Ind => self.emit("mov a,iar0"),
            Loc::Bit(m, b) => {
                self.emit("mov a,0");
                self.emit(format!("sz [{}].{}", m, b));
                self.emit("mov a,1");
            }
            Loc::BitInd(b) => {
                self.emit("mov a,0");
                self.emit(format!("sz iar0.{}", b));
                self.emit("mov a,1");
            }
        }
    }

    /// Byte `byte` (0 or 1) of a 16-bit memory/immediate value into ACC.
    fn byte_to_acc(&mut self, loc: &Loc, byte: usize) {
        match loc {
            Loc::Imm(x) => self.emit(format!("mov a,{}", (x >> (8 * byte)) & 0xFF)),
            _ => {
                let a = self.addr(loc, byte);
                self.emit(format!("mov a,{}", a));
            }
        }
    }

    /// Store ACC into an 8-bit temporary.
    fn acc_to_temp(&mut self) -> Loc {
        let t = self.alloc_temp(1);
        let a = self.addr(&t, 0);
        self.emit(format!("mov {},a", a));
        t
    }

    /// Convert a value to `to`, returning a new value of that type.
    fn convert(&mut self, v: Val, to: Type) -> Val {
        if v.ty == to {
            return v;
        }
        if let Loc::Imm(x) = v.loc {
            return Val::new(Loc::Imm(to.wrap(x)), to);
        }
        match to.size() {
            0 => v,
            1 => {
                if to == Type::Bool {
                    // Normalise to 0/1.
                    if v.ty == Type::Bool {
                        return Val::new(v.loc, to);
                    }
                    let lf = self.new_label();
                    let le = self.new_label();
                    self.val_cond(&v, &lf, false);
                    self.emit("mov a,1");
                    self.emit(format!("jmp {}", le));
                    self.emit_label(&lf);
                    self.emit("mov a,0");
                    self.emit_label(&le);
                    return Val::new(Loc::Acc, to);
                }
                match v.loc {
                    Loc::Temp(off, _) => Val::new(Loc::Temp(off, 1), to),
                    Loc::Bit(..) | Loc::BitInd(_) => {
                        self.load_acc(&v);
                        Val::new(Loc::Acc, to)
                    }
                    other => Val::new(other, to),
                }
            }
            _ => {
                if v.ty.size() == 2 {
                    return Val::new(v.loc, to);
                }
                // Extend 8 → 16.
                let t = self.alloc_temp(2);
                self.load_acc(&v);
                let a0 = self.addr(&t, 0);
                let a1 = self.addr(&t, 1);
                self.emit(format!("mov {},a", a0));
                if v.ty.is_signed() {
                    self.emit("mov a,0");
                    self.emit(format!("sz {}.7", a0));
                    self.emit("mov a,255");
                    self.emit(format!("mov {},a", a1));
                } else {
                    self.emit(format!("clr {}", a1));
                }
                Val::new(t, to)
            }
        }
    }

    /// Evaluate to a 16-bit memory/immediate location of the natural
    /// (sign-preserving) 16-bit type.
    fn eval16(&mut self, e: &Expr) -> R<Val> {
        let v = self.eval(e)?;
        let to = if v.ty.is_signed() {
            Type::I16
        } else {
            Type::U16
        };
        let v = self.convert(v, to);
        Ok(self.materialize16(v))
    }

    /// Ensure a 16-bit value is in memory (Mem/Temp) or is an immediate.
    fn materialize16(&mut self, v: Val) -> Val {
        match v.loc {
            Loc::Mem(_) | Loc::Temp(..) | Loc::Imm(_) => v,
            Loc::Ind => {
                // Copy element pair through IAR0.
                let t = self.alloc_temp(2);
                let a0 = self.addr(&t, 0);
                let a1 = self.addr(&t, 1);
                self.emit("mov a,iar0");
                self.emit(format!("mov {},a", a0));
                self.emit("inc mp0");
                self.emit("mov a,iar0");
                self.emit(format!("mov {},a", a1));
                Val::new(t, v.ty)
            }
            _ => panic!("16-bit value in {:?}", v.loc),
        }
    }

    /// Copy a 16-bit value into a fresh temporary (or return the existing one).
    fn copy16_to_temp(&mut self, v: &Val) -> Loc {
        if let Loc::Temp(_, 2) = v.loc {
            return v.loc.clone();
        }
        let t = self.alloc_temp(2);
        for b in 0..2 {
            self.byte_to_acc(&v.loc, b);
            let a = self.addr(&t, b);
            self.emit(format!("mov {},a", a));
        }
        t
    }

    /// True if evaluating the expression produces no code and does not touch
    /// ACC/MP0: constants and directly addressed scalars/elements.
    fn simple(&mut self, e: &Expr) -> R<Option<Val>> {
        if let Some(v) = self.fold(e) {
            let ty = Self::const_type(e, v);
            return Ok(Some(Val::new(Loc::Imm(ty.wrap(v)), ty)));
        }
        match &e.kind {
            ExprKind::Var(name) => {
                let v = self.resolve_var(name, e.pos)?;
                Ok(match v.loc {
                    Loc::Mem(_) | Loc::Imm(_) => Some(v),
                    _ => None,
                })
            }
            ExprKind::Index(name, idx) => {
                if let Some(i) = self.fold(idx) {
                    if let Some(v) = self.const_index(name, i, e.pos)? {
                        return Ok(Some(v));
                    }
                }
                Ok(None)
            }
            ExprKind::Cast(t, inner) if t.size() == 1 => {
                // (u8)x where x is simple 8-bit: still simple.
                match self.simple(inner)? {
                    Some(v) if v.ty.size() == 1 && v.ty != Type::Bool && *t != Type::Bool => {
                        Ok(Some(Val::new(v.loc, *t)))
                    }
                    Some(v) if v.ty.size() == 2 && *t != Type::Bool => match v.loc {
                        Loc::Imm(x) => Ok(Some(Val::new(Loc::Imm(t.wrap(x)), *t))),
                        Loc::Mem(_) => Ok(Some(Val::new(v.loc, *t))),
                        _ => Ok(None),
                    },
                    _ => Ok(None),
                }
            }
            _ => Ok(None),
        }
    }

    /// Element `i` of a RAM array with a constant index (None for const tables).
    fn const_index(&mut self, name: &str, i: i64, pos: Pos) -> R<Option<Val>> {
        if let Some(l) = self.lookup_local(name) {
            let len = match l.len {
                Some(n) => n,
                None => return err(pos, format!("'{}' is not an array", name)),
            };
            if i < 0 || i as usize >= len {
                return err(
                    pos,
                    format!("index {} out of bounds for '{}[{}]'", i, name, len),
                );
            }
            let off = l.off.unwrap() + i as usize * l.ty.size();
            return Ok(Some(Val::new(Loc::Mem(self.frame_addr(off)), l.ty)));
        }
        if let Some(g) = self.globals.get(name).cloned() {
            let len = match g.len {
                Some(n) => n,
                None => return err(pos, format!("'{}' is not an array", name)),
            };
            if i < 0 || i as usize >= len {
                return err(
                    pos,
                    format!("index {} out of bounds for '{}[{}]'", i, name, len),
                );
            }
            return Ok(match g.kind {
                GlobalKind::Ram => {
                    let off = i as usize * g.ty.size();
                    if off == 0 {
                        Some(Val::new(Loc::Mem(g.sym.clone()), g.ty))
                    } else {
                        Some(Val::new(Loc::Mem(format!("{}+{}", g.sym, off)), g.ty))
                    }
                }
                GlobalKind::ConstTable(ref vals) => {
                    // Constant-fold the table element.
                    let v = vals.get(i as usize).copied().unwrap_or(0);
                    Some(Val::new(Loc::Imm(v), g.ty))
                }
                GlobalKind::ConstScalar(_) => unreachable!(),
            });
        }
        err(pos, format!("undefined array '{}'", name))
    }

    /// Load MP0 with the address of `name[idx]` (RAM arrays).  Returns the
    /// element type.  Clobbers ACC.
    fn load_mp0(&mut self, name: &str, idx: &Expr, pos: Pos) -> R<Type> {
        let (base, ty, len) = if let Some(l) = self.lookup_local(name) {
            match l.len {
                Some(n) => (self.frame_addr(l.off.unwrap()), l.ty, n),
                None => return err(pos, format!("'{}' is not an array", name)),
            }
        } else if let Some(g) = self.globals.get(name).cloned() {
            match (g.len, &g.kind) {
                (Some(n), GlobalKind::Ram) => (g.sym.clone(), g.ty, n),
                _ => return err(pos, format!("'{}' is not a RAM array", name)),
            }
        } else {
            return err(pos, format!("undefined array '{}'", name));
        };
        let _ = len;
        let iv = self.eval(idx)?;
        self.load_acc(&iv);
        self.free(&iv.loc);
        if ty.size() == 2 {
            self.emit("add a,acc");
        }
        self.emit(format!("add a,low({})", base));
        self.emit("mov mp0,a");
        self.cur().uses.mp0 = true;
        Ok(ty)
    }

    /// Read `name[idx]` from a const table in program memory.
    fn table_read(&mut self, name: &str, idx: &Expr, g: &Global, pos: Pos) -> R<Val> {
        let _ = pos;
        self.cur().uses.table = true;
        if let Some(i) = self.fold(idx) {
            let len = g.len.unwrap();
            if i < 0 || i as usize >= len {
                return err(
                    idx.pos,
                    format!("index {} out of bounds for '{}[{}]'", i, name, len),
                );
            }
            self.emit(format!("mov a,low({}+{})", g.sym, i));
            self.emit("mov tblp,a");
            self.emit(format!("mov a,high({}+{})", g.sym, i));
            self.emit("mov tbhp,a");
        } else {
            let iv = self.eval(idx)?;
            self.load_acc(&iv);
            self.free(&iv.loc);
            self.emit(format!("add a,low({})", g.sym));
            self.emit("mov tblp,a");
            self.emit(format!("mov a,high({})", g.sym));
            self.emit("sz c");
            self.emit("add a,1");
            self.emit("mov tbhp,a");
        }
        if g.ty.size() == 1 {
            self.emit("tabrd acc");
            Ok(Val::new(Loc::Acc, g.ty))
        } else {
            let t = self.alloc_temp(2);
            let a0 = self.addr(&t, 0);
            let a1 = self.addr(&t, 1);
            self.emit(format!("tabrd {}", a0));
            self.emit("mov a,tblh");
            self.emit(format!("mov {},a", a1));
            Ok(Val::new(t, g.ty))
        }
    }

    // -----------------------------------------------------------------
    // Expressions
    // -----------------------------------------------------------------

    fn eval(&mut self, e: &Expr) -> R<Val> {
        if let Some(v) = self.fold(e) {
            let ty = Self::const_type(e, v);
            return Ok(Val::new(Loc::Imm(ty.wrap(v)), ty));
        }
        match &e.kind {
            ExprKind::Int(v) => Ok(Val::new(Loc::Imm(*v), literal_type(*v))),
            ExprKind::Bool(b) => Ok(Val::new(Loc::Imm(*b as i64), Type::Bool)),
            ExprKind::Var(name) => self.resolve_var(name, e.pos),
            ExprKind::Index(name, idx) => {
                if let Some(g) = self.globals.get(name).cloned() {
                    if let GlobalKind::ConstTable(_) = g.kind {
                        return self.table_read(name, idx, &g, e.pos);
                    }
                }
                if let Some(i) = self.fold(idx) {
                    return Ok(self.const_index(name, i, e.pos)?.unwrap());
                }
                let ty = self.load_mp0(name, idx, e.pos)?;
                Ok(Val::new(Loc::Ind, ty))
            }
            ExprKind::Bit(base, bit) => {
                let b = self.eval(base)?;
                if b.ty.size() != 1 {
                    return err(e.pos, "bit access requires an 8-bit variable");
                }
                Ok(match b.loc {
                    Loc::Mem(m) => Val::new(Loc::Bit(m, *bit), Type::Bool),
                    Loc::Ind => Val::new(Loc::BitInd(*bit), Type::Bool),
                    _ => return err(e.pos, "bit access requires a variable or array element"),
                })
            }
            ExprKind::Call(name, args) => self.eval_call(name, args, e.pos),
            ExprKind::Unary(op, x) => self.eval_unary(*op, x, e.pos),
            ExprKind::Binary(op, l, r) => self.eval_binary(*op, l, r, e),
            ExprKind::Assign(l, r) => {
                let v = self.eval(r)?;
                self.store(l, v)
            }
            ExprKind::CompoundAssign(op, l, r) => self.eval_compound(*op, l, r, e),
            ExprKind::IncDec {
                target,
                delta,
                prefix,
            } => self.eval_incdec(target, *delta, *prefix, false),
            ExprKind::Cast(t, x) => {
                let v = self.eval(x)?;
                if *t == Type::Void {
                    return err(e.pos, "cannot cast to void");
                }
                Ok(self.convert(v, *t))
            }
            ExprKind::Ternary(c, a, b) => {
                let ty = Type::common(self.type_of(a)?, self.type_of(b)?);
                let lf = self.new_label();
                let le = self.new_label();
                self.cond(c, &lf, false)?;
                if ty.size() == 1 {
                    let va = self.eval(a)?;
                    let va = self.convert(va, ty);
                    self.load_acc(&va);
                    self.free(&va.loc);
                    self.emit(format!("jmp {}", le));
                    self.emit_label(&lf);
                    let vb = self.eval(b)?;
                    let vb = self.convert(vb, ty);
                    self.load_acc(&vb);
                    self.free(&vb.loc);
                    self.emit_label(&le);
                    Ok(Val::new(Loc::Acc, ty))
                } else {
                    let t = self.alloc_temp(2);
                    let va = self.eval(a)?;
                    let va = self.convert(va, ty);
                    let va = self.materialize16(va);
                    self.copy16_into(&va.loc, &t);
                    self.free(&va.loc);
                    self.emit(format!("jmp {}", le));
                    self.emit_label(&lf);
                    let vb = self.eval(b)?;
                    let vb = self.convert(vb, ty);
                    let vb = self.materialize16(vb);
                    self.copy16_into(&vb.loc, &t);
                    self.free(&vb.loc);
                    self.emit_label(&le);
                    Ok(Val::new(t, ty))
                }
            }
        }
    }

    fn copy16_into(&mut self, from: &Loc, to: &Loc) {
        if from == to {
            return;
        }
        for b in 0..2 {
            self.byte_to_acc(from, b);
            let a = self.addr(to, b);
            self.emit(format!("mov {},a", a));
        }
    }

    fn eval_unary(&mut self, op: UnOp, x: &Expr, pos: Pos) -> R<Val> {
        let _ = pos;
        match op {
            UnOp::LNot => {
                let lf = self.new_label();
                let le = self.new_label();
                self.cond(x, &lf, true)?;
                self.emit("mov a,1");
                self.emit(format!("jmp {}", le));
                self.emit_label(&lf);
                self.emit("mov a,0");
                self.emit_label(&le);
                Ok(Val::new(Loc::Acc, Type::Bool))
            }
            UnOp::Neg | UnOp::Not => {
                let v = self.eval(x)?;
                let ty = if v.ty == Type::Bool { Type::U8 } else { v.ty };
                if ty.size() == 1 {
                    self.load_acc(&v);
                    self.free(&v.loc);
                    self.emit("cpla acc");
                    if op == UnOp::Neg {
                        self.emit("add a,1");
                    }
                    Ok(Val::new(Loc::Acc, ty))
                } else {
                    let v = self.materialize16(v);
                    let t = self.copy16_to_temp(&v);
                    if t != v.loc {
                        self.free(&v.loc);
                    }
                    let a0 = self.addr(&t, 0);
                    let a1 = self.addr(&t, 1);
                    self.emit(format!("cpl {}", a0));
                    self.emit(format!("cpl {}", a1));
                    if op == UnOp::Neg {
                        self.emit(format!("inc {}", a0));
                        self.emit("sz z");
                        self.emit(format!("inc {}", a1));
                    }
                    Ok(Val::new(t, ty))
                }
            }
        }
    }

    fn eval_binary(&mut self, op: BinOp, l: &Expr, r: &Expr, e: &Expr) -> R<Val> {
        if op.is_comparison() || op.is_logical() {
            let lf = self.new_label();
            let le = self.new_label();
            self.cond(e, &lf, false)?;
            self.emit("mov a,1");
            self.emit(format!("jmp {}", le));
            self.emit_label(&lf);
            self.emit("mov a,0");
            self.emit_label(&le);
            return Ok(Val::new(Loc::Acc, Type::Bool));
        }
        let tl = self.type_of(l)?;
        let tr = self.type_of(r)?;
        let ty = match op {
            BinOp::Shl | BinOp::Shr => {
                if tl == Type::Bool {
                    Type::U8
                } else {
                    tl
                }
            }
            _ => Type::common(tl, tr),
        };
        match op {
            BinOp::Shl | BinOp::Shr => self.eval_shift(op, l, r, ty),
            BinOp::Mul | BinOp::Div | BinOp::Rem => self.eval_muldiv(op, l, r, ty, e.pos),
            _ => {
                if ty.size() == 1 {
                    self.bin8(op, l, r, ty)
                } else {
                    self.bin16(op, l, r, ty)
                }
            }
        }
    }

    fn op_mnemonic(op: BinOp) -> &'static str {
        match op {
            BinOp::Add => "add",
            BinOp::Sub => "sub",
            BinOp::And => "and",
            BinOp::Or => "or",
            BinOp::Xor => "xor",
            _ => unreachable!(),
        }
    }

    /// `op a,<operand>` for a simple 8-bit operand.
    fn emit_op_acc(&mut self, mn: &str, v: &Val) {
        match &v.loc {
            Loc::Imm(x) => self.emit(format!("{} a,{}", mn, x & 0xFF)),
            Loc::Mem(_) | Loc::Temp(..) => {
                let a = self.addr(&v.loc, 0);
                self.emit(format!("{} a,{}", mn, a));
            }
            Loc::Ind => self.emit(format!("{} a,iar0", mn)),
            other => panic!("emit_op_acc with {:?}", other),
        }
    }

    fn bin8(&mut self, op: BinOp, l: &Expr, r: &Expr, ty: Type) -> R<Val> {
        let mn = Self::op_mnemonic(op);
        let commutative = op != BinOp::Sub;
        if let Some(rv) = self.simple(r)? {
            let lv = self.eval(l)?;
            self.load_acc(&lv);
            self.free(&lv.loc);
            self.emit_op_acc(mn, &rv);
            return Ok(Val::new(Loc::Acc, ty));
        }
        if commutative {
            if let Some(lv) = self.simple(l)? {
                let rv = self.eval(r)?;
                self.load_acc(&rv);
                self.free(&rv.loc);
                self.emit_op_acc(mn, &lv);
                return Ok(Val::new(Loc::Acc, ty));
            }
        }
        let rv = self.eval(r)?;
        self.load_acc(&rv);
        self.free(&rv.loc);
        let t = self.acc_to_temp();
        let lv = self.eval(l)?;
        self.load_acc(&lv);
        self.free(&lv.loc);
        self.emit_op_acc(mn, &Val::new(t.clone(), ty));
        self.free(&t);
        Ok(Val::new(Loc::Acc, ty))
    }

    fn bin16(&mut self, op: BinOp, l: &Expr, r: &Expr, ty: Type) -> R<Val> {
        let lv = self.eval16(l)?;
        let rv = self.eval16(r)?;
        let rv = if op == BinOp::Sub {
            if let Loc::Imm(_) = rv.loc {
                let t = self.copy16_to_temp(&rv);
                Val::new(t, rv.ty)
            } else {
                rv
            }
        } else {
            rv
        };
        // Pick the result temporary: reuse L's temp if any, else R's, else new.
        let (t, free_r) = match (&lv.loc, &rv.loc) {
            (Loc::Temp(..), Loc::Temp(..)) => (lv.loc.clone(), true),
            (Loc::Temp(..), _) => (lv.loc.clone(), false),
            (_, Loc::Temp(..)) => (rv.loc.clone(), false),
            _ => (self.alloc_temp(2), false),
        };
        let (lo_mn, hi_mn) = match op {
            BinOp::Add => ("add", "adc"),
            BinOp::Sub => ("sub", "sbc"),
            BinOp::And => ("and", "and"),
            BinOp::Or => ("or", "or"),
            BinOp::Xor => ("xor", "xor"),
            _ => unreachable!(),
        };
        // low byte
        self.byte_to_acc(&lv.loc, 0);
        match &rv.loc {
            Loc::Imm(x) => self.emit(format!("{} a,{}", lo_mn, x & 0xFF)),
            _ => {
                let a = self.addr(&rv.loc, 0);
                self.emit(format!("{} a,{}", lo_mn, a));
            }
        }
        let t0 = self.addr(&t, 0);
        self.emit(format!("mov {},a", t0));
        // high byte
        match &rv.loc {
            Loc::Imm(x) => {
                // commutative ops only (sub immediates were materialised)
                self.emit(format!("mov a,{}", (x >> 8) & 0xFF));
                let a = self.addr(&lv.loc, 1);
                self.emit(format!("{} a,{}", hi_mn, a));
            }
            _ => {
                self.byte_to_acc(&lv.loc, 1);
                let a = self.addr(&rv.loc, 1);
                self.emit(format!("{} a,{}", hi_mn, a));
            }
        }
        let t1 = self.addr(&t, 1);
        self.emit(format!("mov {},a", t1));
        if free_r {
            self.free(&rv.loc);
        }
        Ok(Val::new(t, ty))
    }

    fn eval_shift(&mut self, op: BinOp, l: &Expr, r: &Expr, ty: Type) -> R<Val> {
        let count = self.fold(r);
        if ty.size() == 1 {
            let lv = self.eval(l)?;
            let lv = self.convert(lv, ty);
            self.load_acc(&lv);
            self.free(&lv.loc);
            let signed = ty.is_signed() && op == BinOp::Shr;
            if let Some(n) = count {
                let n = (n & 0xFF).min(8) as usize;
                if n == 0 {
                    return Ok(Val::new(Loc::Acc, ty));
                }
                if op == BinOp::Shl {
                    for _ in 0..n {
                        self.emit("add a,acc");
                    }
                } else if !signed {
                    for _ in 0..n {
                        self.emit("clr c");
                        self.emit("rrca acc");
                    }
                } else {
                    let t = self.acc_to_temp();
                    let a = self.addr(&t, 0);
                    for i in 0..n {
                        self.emit(format!("rlca {}", a)); // C = sign
                        self.emit(format!("rrca {}", a));
                        if i + 1 < n {
                            self.emit(format!("mov {},a", a));
                        }
                    }
                    self.free(&t);
                }
                return Ok(Val::new(Loc::Acc, ty));
            }
            // variable count: value in a temp, count in a temp
            let tv = self.acc_to_temp();
            let rv = self.eval(r)?;
            self.load_acc(&rv);
            self.free(&rv.loc);
            let tc = self.acc_to_temp();
            let av = self.addr(&tv, 0);
            let ac = self.addr(&tc, 0);
            let lloop = self.new_label();
            let lend = self.new_label();
            self.emit(format!("sz {}", ac));
            self.emit(format!("jmp {}", lloop));
            self.emit(format!("jmp {}", lend));
            self.emit_label(&lloop);
            if op == BinOp::Shl {
                self.emit("clr c");
                self.emit(format!("rlc {}", av));
            } else if signed {
                self.emit(format!("rlca {}", av));
                self.emit(format!("rrc {}", av));
            } else {
                self.emit("clr c");
                self.emit(format!("rrc {}", av));
            }
            self.emit(format!("sdz {}", ac));
            self.emit(format!("jmp {}", lloop));
            self.emit_label(&lend);
            self.emit(format!("mov a,{}", av));
            self.free(&tc);
            self.free(&tv);
            return Ok(Val::new(Loc::Acc, ty));
        }
        // 16-bit
        let lv = self.eval16(l)?;
        let t = self.copy16_to_temp(&lv);
        if t != lv.loc {
            self.free(&lv.loc);
        }
        let a0 = self.addr(&t, 0);
        let a1 = self.addr(&t, 1);
        let signed = ty.is_signed() && op == BinOp::Shr;
        let shift_once = |g: &mut Gen| {
            if op == BinOp::Shl {
                g.emit("clr c");
                g.emit(format!("rlc {}", a0));
                g.emit(format!("rlc {}", a1));
            } else {
                if signed {
                    g.emit(format!("rlca {}", a1));
                } else {
                    g.emit("clr c");
                }
                g.emit(format!("rrc {}", a1));
                g.emit(format!("rrc {}", a0));
            }
        };
        if let Some(n) = count {
            let n = (n & 0xFF).min(16) as usize;
            if n >= 8 && !signed {
                // Byte move first.
                if op == BinOp::Shl {
                    self.emit(format!("mov a,{}", a0));
                    self.emit(format!("mov {},a", a1));
                    self.emit(format!("clr {}", a0));
                } else {
                    self.emit(format!("mov a,{}", a1));
                    self.emit(format!("mov {},a", a0));
                    self.emit(format!("clr {}", a1));
                }
                for _ in 8..n {
                    shift_once(self);
                }
            } else {
                for _ in 0..n {
                    shift_once(self);
                }
            }
            return Ok(Val::new(t, ty));
        }
        let rv = self.eval(r)?;
        self.load_acc(&rv);
        self.free(&rv.loc);
        let tc = self.acc_to_temp();
        let ac = self.addr(&tc, 0);
        let lloop = self.new_label();
        let lend = self.new_label();
        self.emit(format!("sz {}", ac));
        self.emit(format!("jmp {}", lloop));
        self.emit(format!("jmp {}", lend));
        self.emit_label(&lloop);
        shift_once(self);
        self.emit(format!("sdz {}", ac));
        self.emit(format!("jmp {}", lloop));
        self.emit_label(&lend);
        self.free(&tc);
        Ok(Val::new(t, ty))
    }

    fn eval_muldiv(&mut self, op: BinOp, l: &Expr, r: &Expr, ty: Type, pos: Pos) -> R<Val> {
        if let Some(0) = self.fold(r) {
            if op != BinOp::Mul {
                return err(pos, "division by zero");
            }
        }
        let signed = ty.is_signed();
        if ty.size() == 1 {
            let routine = match op {
                BinOp::Mul => "__mul8",
                _ => {
                    if signed {
                        "__divs8"
                    } else {
                        "__divu8"
                    }
                }
            };
            // r first (into a temp unless simple), then l into __r0.
            let rv = match self.simple(r)? {
                Some(v) => v,
                None => {
                    let v = self.eval(r)?;
                    self.load_acc(&v);
                    self.free(&v.loc);
                    Val::new(self.acc_to_temp(), ty)
                }
            };
            let lv = self.eval(l)?;
            self.load_acc(&lv);
            self.free(&lv.loc);
            self.emit("mov [__r0],a");
            self.load_acc(&rv);
            self.free(&rv.loc);
            self.emit("mov [__r1],a");
            self.use_runtime(routine);
            self.emit(format!("call {}", routine));
            if op == BinOp::Rem {
                self.emit("mov a,[__r2]");
            }
            return Ok(Val::new(Loc::Acc, ty));
        }
        let routine = match op {
            BinOp::Mul => "__mul16",
            _ => {
                if signed {
                    "__divs16"
                } else {
                    "__divu16"
                }
            }
        };
        let lv = self.eval16(l)?;
        let rv = self.eval16(r)?;
        for b in 0..2 {
            self.byte_to_acc(&lv.loc, b);
            self.emit(format!("mov [__r{}],a", b));
        }
        for b in 0..2 {
            self.byte_to_acc(&rv.loc, b);
            self.emit(format!("mov [__r{}],a", 2 + b));
        }
        // Result temp: reuse a temp operand if possible (LIFO: free r, keep l).
        let t = match (&lv.loc, &rv.loc) {
            (Loc::Temp(..), Loc::Temp(..)) => {
                self.free(&rv.loc);
                lv.loc.clone()
            }
            (Loc::Temp(..), _) => lv.loc.clone(),
            (_, Loc::Temp(..)) => rv.loc.clone(),
            _ => self.alloc_temp(2),
        };
        self.use_runtime(routine);
        self.emit(format!("call {}", routine));
        let (r0, r1) = if op == BinOp::Rem {
            ("__r6", "__r7")
        } else {
            ("__r4", "__r5")
        };
        let a0 = self.addr(&t, 0);
        let a1 = self.addr(&t, 1);
        self.emit(format!("mov a,[{}]", r0));
        self.emit(format!("mov {},a", a0));
        self.emit(format!("mov a,[{}]", r1));
        self.emit(format!("mov {},a", a1));
        Ok(Val::new(t, ty))
    }

    fn eval_compound(&mut self, op: BinOp, l: &Expr, r: &Expr, e: &Expr) -> R<Val> {
        let tl = self.type_of(l)?;
        // Fast paths for directly addressed 8-bit targets.
        if let ExprKind::Var(_) | ExprKind::Index(..) = &l.kind {
            if let Some(target) = self.simple(l)? {
                if let Loc::Mem(_) = target.loc {
                    if tl.size() == 1
                        && tl != Type::Bool
                        && matches!(op, BinOp::Add | BinOp::And | BinOp::Or | BinOp::Xor)
                    {
                        let rv = self.eval(r)?;
                        self.load_acc(&rv);
                        self.free(&rv.loc);
                        let mn = format!("{}m", Self::op_mnemonic(op));
                        let a = self.addr(&target.loc, 0);
                        self.emit(format!("{} a,{}", mn, a));
                        // value is now in memory; reload for the expression value
                        self.emit(format!("mov a,{}", a));
                        return Ok(Val::new(Loc::Acc, tl));
                    }
                    if tl.size() == 1 && tl != Type::Bool && op == BinOp::Sub {
                        if let Some(rv) = self.simple(r)? {
                            let a = self.addr(&target.loc, 0);
                            self.emit(format!("mov a,{}", a));
                            self.emit_op_acc("sub", &rv);
                            self.emit(format!("mov {},a", a));
                            return Ok(Val::new(Loc::Acc, tl));
                        }
                    }
                    if tl.size() == 2 && matches!(op, BinOp::Add | BinOp::Sub) {
                        let rv = self.eval16(r)?;
                        let rv = if let Loc::Imm(_) = rv.loc {
                            if op == BinOp::Sub {
                                Val::new(self.copy16_to_temp(&rv), rv.ty)
                            } else {
                                rv
                            }
                        } else {
                            rv
                        };
                        let a0 = self.addr(&target.loc, 0);
                        let a1 = self.addr(&target.loc, 1);
                        if op == BinOp::Add {
                            self.byte_to_acc(&rv.loc, 0);
                            self.emit(format!("addm a,{}", a0));
                            self.byte_to_acc(&rv.loc, 1);
                            self.emit(format!("adcm a,{}", a1));
                        } else {
                            let r0 = self.addr(&rv.loc, 0);
                            let r1 = self.addr(&rv.loc, 1);
                            self.emit(format!("mov a,{}", a0));
                            self.emit(format!("sub a,{}", r0));
                            self.emit(format!("mov {},a", a0));
                            self.emit(format!("mov a,{}", a1));
                            self.emit(format!("sbc a,{}", r1));
                            self.emit(format!("mov {},a", a1));
                        }
                        self.free(&rv.loc);
                        return Ok(Val::new(target.loc, tl));
                    }
                }
            }
        }
        // Generic: l = l op r
        let bin = Expr {
            kind: ExprKind::Binary(op, Box::new(l.clone()), Box::new(r.clone())),
            pos: e.pos,
        };
        let v = self.eval(&bin)?;
        self.store(l, v)
    }

    fn eval_incdec(&mut self, target: &Expr, delta: i64, prefix: bool, discard: bool) -> R<Val> {
        let ty = self.type_of(target)?;
        if ty == Type::Bool {
            return err(target.pos, "++/-- on a bool or bit");
        }
        let (mn, mn16_lo) = if delta > 0 {
            ("inc", "inc")
        } else {
            ("dec", "dec")
        };
        // Directly addressed?
        let simple = match &target.kind {
            ExprKind::Var(_) | ExprKind::Index(..) => self.simple(target)?,
            _ => None,
        };
        if let Some(Val {
            loc: Loc::Mem(m), ..
        }) = simple
        {
            let a0 = format!("[{}]", m);
            let a1 = format!("[{}+1]", m);
            if ty.size() == 1 {
                if !prefix && !discard {
                    self.emit(format!("mov a,{}", a0));
                    let t = self.acc_to_temp();
                    self.emit(format!("{} {}", mn, a0));
                    return Ok(Val::new(t, ty));
                }
                self.emit(format!("{} {}", mn, a0));
                return Ok(Val::new(Loc::Mem(m), ty));
            }
            let old = if !prefix && !discard {
                Some(self.copy16_to_temp(&Val::new(Loc::Mem(m.clone()), ty)))
            } else {
                None
            };
            if delta > 0 {
                self.emit(format!("{} {}", mn16_lo, a0));
                self.emit("sz z");
                self.emit(format!("inc {}", a1));
            } else {
                let l = self.new_label();
                self.emit(format!("sz {}", a0));
                self.emit(format!("jmp {}", l));
                self.emit(format!("dec {}", a1));
                self.emit_label(&l);
                self.emit(format!("dec {}", a0));
            }
            return Ok(match old {
                Some(t) => Val::new(t, ty),
                None => Val::new(Loc::Mem(m), ty),
            });
        }
        // Indirect element.
        if let ExprKind::Index(name, idx) = &target.kind {
            if ty.size() == 1 {
                self.load_mp0(name, idx, target.pos)?;
                if !prefix && !discard {
                    self.emit("mov a,iar0");
                    let t = self.acc_to_temp();
                    self.emit(format!("{} iar0", mn));
                    return Ok(Val::new(t, ty));
                }
                self.emit(format!("{} iar0", mn));
                return Ok(Val::new(Loc::Ind, ty));
            }
            // 16-bit element: read-modify-write through a temp.
            let v = self.eval(target)?;
            let v = self.materialize16(v);
            let t = self.copy16_to_temp(&v);
            let old = if !prefix && !discard {
                Some(self.copy16_to_temp(&Val::new(t.clone(), ty)))
            } else {
                None
            };
            let one = Expr {
                kind: ExprKind::Int(delta),
                pos: target.pos,
            };
            let _ = one;
            let a0 = self.addr(&t, 0);
            let a1 = self.addr(&t, 1);
            if delta > 0 {
                self.emit(format!("inc {}", a0));
                self.emit("sz z");
                self.emit(format!("inc {}", a1));
            } else {
                let l = self.new_label();
                self.emit(format!("sz {}", a0));
                self.emit(format!("jmp {}", l));
                self.emit(format!("dec {}", a1));
                self.emit_label(&l);
                self.emit(format!("dec {}", a0));
            }
            let stored = self.store(target, Val::new(t.clone(), ty))?;
            return Ok(match old {
                Some(o) => Val::new(o, ty),
                None => stored,
            });
        }
        err(target.pos, "invalid ++/-- target")
    }

    // -----------------------------------------------------------------
    // Calls
    // -----------------------------------------------------------------

    fn eval_call(&mut self, name: &str, args: &[Expr], pos: Pos) -> R<Val> {
        let void = Val::new(Loc::Imm(0), Type::Void);
        match name {
            "halt" | "nop" | "clr_wdt" | "ei" | "di" | "enable_interrupts"
            | "disable_interrupts" => {
                if !args.is_empty() {
                    return err(pos, format!("{}() takes no arguments", name));
                }
                match name {
                    "halt" => self.emit("halt"),
                    "nop" => self.emit("nop"),
                    "clr_wdt" => self.emit("clr wdt"),
                    "ei" | "enable_interrupts" => self.emit("set emi"),
                    _ => self.emit("clr emi"),
                }
                return Ok(void);
            }
            _ => {}
        }
        let f = match self.fns.get(name) {
            Some(f) => f.clone(),
            None => return err(pos, format!("undefined function '{}'", name)),
        };
        if f.isr.is_some() {
            return err(
                pos,
                format!("interrupt handler '{}' cannot be called directly", name),
            );
        }
        if args.len() != f.params.len() {
            return err(
                pos,
                format!(
                    "'{}' expects {} argument(s), got {}",
                    name,
                    f.params.len(),
                    args.len()
                ),
            );
        }
        let any_call = args.iter().any(contains_call);
        let mut param_off = 0usize;
        let mut slots: Vec<(usize, Type)> = Vec::new();
        for p in &f.params {
            slots.push((param_off, p.ty));
            param_off += p.ty.size();
        }
        if any_call {
            // Evaluate everything into temporaries first.
            let mut temps = Vec::new();
            for (arg, (_, pty)) in args.iter().zip(&slots) {
                let v = self.eval(arg)?;
                let v = self.convert(v, *pty);
                let t = if pty.size() == 1 {
                    self.load_acc(&v);
                    self.free(&v.loc);
                    self.acc_to_temp()
                } else {
                    let v = self.materialize16(v);
                    let t = self.copy16_to_temp(&v);
                    if t != v.loc {
                        self.free(&v.loc);
                    }
                    t
                };
                temps.push(t);
            }
            for (t, (off, pty)) in temps.iter().zip(&slots) {
                for b in 0..pty.size() {
                    self.byte_to_acc(t, b);
                    self.emit(format!("mov [F{}+{}],a", f.id, off + b));
                }
            }
            for t in temps.iter().rev() {
                self.free(t);
            }
        } else {
            for (arg, (off, pty)) in args.iter().zip(&slots) {
                let v = self.eval(arg)?;
                let v = self.convert(v, *pty);
                if pty.size() == 1 {
                    self.load_acc(&v);
                    self.emit(format!("mov [F{}+{}],a", f.id, off));
                } else {
                    let v = self.materialize16(v);
                    for b in 0..2 {
                        self.byte_to_acc(&v.loc, b);
                        self.emit(format!("mov [F{}+{}],a", f.id, off + b));
                    }
                    self.free(&v.loc);
                }
            }
        }
        self.cur().calls.insert(name.to_string());
        self.emit(format!("call _{}", name));
        match f.ret.size() {
            0 => Ok(void),
            1 => Ok(Val::new(Loc::Acc, f.ret)),
            _ => {
                let t = self.alloc_temp(2);
                let a0 = self.addr(&t, 0);
                let a1 = self.addr(&t, 1);
                self.emit("mov a,[__ret0]");
                self.emit(format!("mov {},a", a0));
                self.emit("mov a,[__ret1]");
                self.emit(format!("mov {},a", a1));
                Ok(Val::new(t, f.ret))
            }
        }
    }

    // -----------------------------------------------------------------
    // Stores
    // -----------------------------------------------------------------

    fn store(&mut self, target: &Expr, v: Val) -> R<Val> {
        match &target.kind {
            ExprKind::Var(name) => {
                let t = self.resolve_var(name, target.pos)?;
                match t.loc {
                    Loc::Imm(_) => err(target.pos, format!("cannot assign to constant '{}'", name)),
                    Loc::Bit(m, b) => self.store_bit(&Loc::Bit(m, b), v),
                    Loc::Mem(m) => self.store_mem(&m, t.ty, v),
                    _ => unreachable!(),
                }
            }
            ExprKind::Index(name, idx) => {
                if let Some(g) = self.globals.get(name) {
                    if let GlobalKind::ConstTable(_) = g.kind {
                        return err(
                            target.pos,
                            format!("cannot assign to const table '{}'", name),
                        );
                    }
                }
                if let Some(i) = self.fold(idx) {
                    let t = self.const_index(name, i, target.pos)?.unwrap();
                    if let Loc::Mem(m) = t.loc {
                        return self.store_mem(&m, t.ty, v);
                    }
                    unreachable!();
                }
                let ety = self.type_of(target)?;
                let v = self.convert(v, ety);
                if ety.size() == 1 {
                    // Keep the value safe while computing the index.
                    let saved = match v.loc {
                        Loc::Acc => Some(self.acc_to_temp()),
                        Loc::Ind => {
                            self.emit("mov a,iar0");
                            Some(self.acc_to_temp())
                        }
                        Loc::Bit(..) | Loc::BitInd(_) => {
                            self.load_acc(&v);
                            Some(self.acc_to_temp())
                        }
                        _ => None,
                    };
                    self.load_mp0(name, idx, target.pos)?;
                    match &saved {
                        Some(t) => {
                            let a = self.addr(t, 0);
                            self.emit(format!("mov a,{}", a));
                        }
                        None => self.load_acc(&v),
                    }
                    self.emit("mov iar0,a");
                    if let Some(t) = saved {
                        self.free(&t);
                    }
                    self.free(&v.loc);
                    Ok(Val::new(Loc::Acc, ety))
                } else {
                    let v = self.materialize16(v);
                    self.load_mp0(name, idx, target.pos)?;
                    self.byte_to_acc(&v.loc, 0);
                    self.emit("mov iar0,a");
                    self.emit("inc mp0");
                    self.byte_to_acc(&v.loc, 1);
                    self.emit("mov iar0,a");
                    Ok(v)
                }
            }
            ExprKind::Bit(base, bit) => {
                let b = self.eval(base)?;
                if b.ty.size() != 1 {
                    return err(target.pos, "bit access requires an 8-bit variable");
                }
                match b.loc {
                    Loc::Mem(m) => self.store_bit(&Loc::Bit(m, *bit), v),
                    Loc::Ind => self.store_bit(&Loc::BitInd(*bit), v),
                    _ => err(
                        target.pos,
                        "bit access requires a variable or array element",
                    ),
                }
            }
            _ => err(target.pos, "invalid assignment target"),
        }
    }

    fn store_mem(&mut self, m: &str, ty: Type, v: Val) -> R<Val> {
        let v = self.convert(v, ty);
        if ty.size() == 1 {
            self.load_acc(&v);
            self.free(&v.loc);
            self.emit(format!("mov [{}],a", m));
            Ok(Val::new(Loc::Acc, ty))
        } else {
            let v = self.materialize16(v);
            let dst = Loc::Mem(m.to_string());
            self.copy16_into(&v.loc, &dst);
            self.free(&v.loc);
            Ok(Val::new(dst, ty))
        }
    }

    fn store_bit(&mut self, bit: &Loc, v: Val) -> R<Val> {
        let text = match bit {
            Loc::Bit(m, b) => format!("[{}].{}", m, b),
            Loc::BitInd(b) => format!("iar0.{}", b),
            _ => unreachable!(),
        };
        if let Loc::Imm(x) = v.loc {
            if x != 0 {
                self.emit(format!("set {}", text));
            } else {
                self.emit(format!("clr {}", text));
            }
            return Ok(Val::new(Loc::Imm((x != 0) as i64), Type::Bool));
        }
        // The value may be in ACC or elsewhere; clear the bit first (does not
        // touch ACC), then set it if the value is true.
        if let Loc::BitInd(_) = bit {
            // Value could itself be at IAR0 — copy it out first.
            if matches!(v.loc, Loc::Ind | Loc::BitInd(_)) {
                self.load_acc(&v);
                let t = self.acc_to_temp();
                let r = self.store_bit(bit, Val::new(t.clone(), Type::U8));
                self.free(&t);
                return r;
            }
        }
        let skip = self.new_label();
        self.emit(format!("clr {}", text));
        self.val_cond(&v, &skip, false);
        self.emit(format!("set {}", text));
        self.emit_label(&skip);
        self.free(&v.loc);
        Ok(Val::new(bit.clone(), Type::Bool))
    }

    // -----------------------------------------------------------------
    // Conditions
    // -----------------------------------------------------------------

    /// Jump to `label` when the truth value of `v` equals `jump_if`.
    fn val_cond(&mut self, v: &Val, label: &str, jump_if: bool) {
        match &v.loc {
            Loc::Imm(x) => {
                if (*x != 0) == jump_if {
                    self.emit(format!("jmp {}", label));
                }
            }
            Loc::Bit(m, b) => {
                let text = format!("[{}].{}", m, b);
                self.jump_on_bit(&text, jump_if, label);
            }
            Loc::BitInd(b) => {
                let text = format!("iar0.{}", b);
                self.jump_on_bit(&text, jump_if, label);
            }
            _ => {
                if v.ty.size() == 2 {
                    self.byte_to_acc(&v.loc, 0);
                    let a = self.addr(&v.loc, 1);
                    self.emit(format!("or a,{}", a));
                } else {
                    self.load_acc(v);
                    self.emit("or a,0");
                }
                // Z=1 means false.
                self.jump_on_bit("z", !jump_if, label);
            }
        }
    }

    /// Jump to `label` if the bit is set (`when_set`) or clear.
    fn jump_on_bit(&mut self, bit: &str, when_set: bool, label: &str) {
        if when_set {
            self.emit(format!("sz {}", bit));
        } else {
            self.emit(format!("snz {}", bit));
        }
        self.emit(format!("jmp {}", label));
    }

    /// Jump to `label` when `e` evaluates to `jump_if`.
    fn cond(&mut self, e: &Expr, label: &str, jump_if: bool) -> R<()> {
        if let Some(v) = self.fold(e) {
            if (v != 0) == jump_if {
                self.emit(format!("jmp {}", label));
            }
            return Ok(());
        }
        match &e.kind {
            ExprKind::Unary(UnOp::LNot, x) => self.cond(x, label, !jump_if),
            ExprKind::Binary(BinOp::LAnd, a, b) => {
                if jump_if {
                    let skip = self.new_label();
                    self.cond(a, &skip, false)?;
                    self.cond(b, label, true)?;
                    self.emit_label(&skip);
                } else {
                    self.cond(a, label, false)?;
                    self.cond(b, label, false)?;
                }
                Ok(())
            }
            ExprKind::Binary(BinOp::LOr, a, b) => {
                if jump_if {
                    self.cond(a, label, true)?;
                    self.cond(b, label, true)?;
                } else {
                    let skip = self.new_label();
                    self.cond(a, &skip, true)?;
                    self.cond(b, label, false)?;
                    self.emit_label(&skip);
                }
                Ok(())
            }
            ExprKind::Binary(op, l, r) if op.is_comparison() => {
                self.cmp_cond(*op, l, r, label, jump_if)
            }
            _ => {
                let v = self.eval(e)?;
                self.val_cond(&v, label, jump_if);
                self.free(&v.loc);
                Ok(())
            }
        }
    }

    fn cmp_cond(&mut self, op: BinOp, l: &Expr, r: &Expr, label: &str, jump_if: bool) -> R<()> {
        let tl = self.type_of(l)?;
        let tr = self.type_of(r)?;
        // A bit compared with a constant: `PA.3 == 1`
        let ty = Type::common(tl, tr);
        let signed = ty.is_signed();
        // Comparison flag semantics: (flag, want)
        //   Eq: Z==1  Ne: Z==0  Lt: C==0 (l-r)  Ge: C==1 (l-r)  Gt: C==0 (r-l)  Le: C==1 (r-l)
        let (flag, want, swap) = match op {
            BinOp::Eq => ("z", true, false),
            BinOp::Ne => ("z", false, false),
            BinOp::Lt => ("c", false, false),
            BinOp::Ge => ("c", true, false),
            BinOp::Gt => ("c", false, true),
            BinOp::Le => ("c", true, true),
            _ => unreachable!(),
        };
        let (a, b) = if swap { (r, l) } else { (l, r) };
        // We compute a - b (or a ^ b for equality).
        let is_eq = matches!(op, BinOp::Eq | BinOp::Ne);
        if ty.size() == 1 {
            if !signed || is_eq {
                let mn = if is_eq { "xor" } else { "sub" };
                if let Some(bv) = self.simple(b)? {
                    let av = self.eval(a)?;
                    let av = self.convert(av, ty);
                    self.load_acc(&av);
                    self.free(&av.loc);
                    let bv = self.convert(bv, ty);
                    self.emit_op_acc(mn, &bv);
                } else if is_eq && self.simple(a)?.is_some() {
                    let av = self.simple(a)?.unwrap();
                    let bv = self.eval(b)?;
                    let bv = self.convert(bv, ty);
                    self.load_acc(&bv);
                    self.free(&bv.loc);
                    let av = self.convert(av, ty);
                    self.emit_op_acc(mn, &av);
                } else {
                    let bv = self.eval(b)?;
                    let bv = self.convert(bv, ty);
                    self.load_acc(&bv);
                    self.free(&bv.loc);
                    let t = self.acc_to_temp();
                    let av = self.eval(a)?;
                    let av = self.convert(av, ty);
                    self.load_acc(&av);
                    self.free(&av.loc);
                    self.emit_op_acc(mn, &Val::new(t.clone(), ty));
                    self.free(&t);
                }
            } else {
                // signed ordering: flip sign bits, compare unsigned
                let bv = self.eval(b)?;
                let bv = self.convert(bv, ty);
                self.load_acc(&bv);
                self.free(&bv.loc);
                self.emit("xor a,128");
                let t = self.acc_to_temp();
                let av = self.eval(a)?;
                let av = self.convert(av, ty);
                self.load_acc(&av);
                self.free(&av.loc);
                self.emit("xor a,128");
                self.emit_op_acc("sub", &Val::new(t.clone(), ty));
                self.free(&t);
            }
        } else {
            let av = self.eval16(a)?;
            let av = self.convert(av, ty);
            let bv = self.eval16(b)?;
            let bv = self.convert(bv, ty);
            let bv = if !is_eq {
                if let Loc::Imm(_) = bv.loc {
                    Val::new(self.copy16_to_temp(&bv), ty)
                } else {
                    bv
                }
            } else {
                bv
            };
            if is_eq {
                self.byte_to_acc(&av.loc, 0);
                match &bv.loc {
                    Loc::Imm(x) => self.emit(format!("xor a,{}", x & 0xFF)),
                    _ => {
                        let b0 = self.addr(&bv.loc, 0);
                        self.emit(format!("xor a,{}", b0));
                    }
                }
                let t = self.acc_to_temp();
                self.byte_to_acc(&av.loc, 1);
                match &bv.loc {
                    Loc::Imm(x) => self.emit(format!("xor a,{}", (x >> 8) & 0xFF)),
                    _ => {
                        let b1 = self.addr(&bv.loc, 1);
                        self.emit(format!("xor a,{}", b1));
                    }
                }
                let ta = self.addr(&t, 0);
                self.emit(format!("or a,{}", ta));
                self.free(&t);
            } else {
                let (aloc, bloc) = if signed {
                    // Flip sign bits of copies of both high bytes.
                    let ta = self.copy16_to_temp(&av);
                    let tb = self.copy16_to_temp(&bv);
                    for t in [&ta, &tb] {
                        let a1 = self.addr(t, 1);
                        self.emit(format!("mov a,{}", a1));
                        self.emit("xor a,128");
                        self.emit(format!("mov {},a", a1));
                    }
                    (ta, tb)
                } else {
                    (av.loc.clone(), bv.loc.clone())
                };
                self.byte_to_acc(&aloc, 0);
                let b0 = self.addr(&bloc, 0);
                self.emit(format!("sub a,{}", b0));
                self.byte_to_acc(&aloc, 1);
                let b1 = self.addr(&bloc, 1);
                self.emit(format!("sbc a,{}", b1));
                if signed {
                    self.free(&bloc);
                    self.free(&aloc);
                }
            }
            self.free(&bv.loc);
            self.free(&av.loc);
        }
        // cond true  <=> flag == want ; jump when cond == jump_if
        self.jump_on_bit(flag, want == jump_if, label);
        Ok(())
    }

    // -----------------------------------------------------------------
    // Statements
    // -----------------------------------------------------------------

    fn stmts(&mut self, list: &[Stmt]) -> R<()> {
        self.cur().scopes.push(HashMap::new());
        let saved = self.cur().locals_size;
        let mut result = Ok(());
        for s in list {
            if let Err(e) = self.stmt(s) {
                result = Err(e);
                break;
            }
        }
        self.cur().scopes.pop();
        self.cur().locals_size = saved;
        result
    }

    fn declare_local(&mut self, d: &VarDecl) -> R<()> {
        if d.at.is_some() {
            return err(d.pos, "absolute placement is only allowed for globals");
        }
        if self.cur().scopes.last().unwrap().contains_key(&d.name) {
            return err(
                d.pos,
                format!("'{}' already declared in this scope", d.name),
            );
        }
        let array_len = self.array_len_of(d)?;
        if d.is_const {
            if array_len.is_some() {
                return err(d.pos, "const arrays must be global");
            }
            let v = match &d.init {
                Some(Initializer::Scalar(e)) => match self.fold(e) {
                    Some(v) => v,
                    None => return err(d.pos, "const initializer must be a constant expression"),
                },
                _ => return err(d.pos, "const variable needs an initializer"),
            };
            let local = Local {
                ty: d.ty,
                len: None,
                off: None,
                constant: Some(d.ty.wrap(v)),
            };
            self.cur()
                .scopes
                .last_mut()
                .unwrap()
                .insert(d.name.clone(), local);
            return Ok(());
        }
        let count = array_len.unwrap_or(1);
        let size = count * d.ty.size();
        let off = self.cur().locals_size;
        {
            let c = self.cur();
            c.locals_size += size;
            if c.locals_size > c.locals_max {
                c.locals_max = c.locals_size;
            }
        }
        let local = Local {
            ty: d.ty,
            len: array_len,
            off: Some(off),
            constant: None,
        };
        self.cur()
            .scopes
            .last_mut()
            .unwrap()
            .insert(d.name.clone(), local);
        match &d.init {
            None => {}
            Some(Initializer::Scalar(e)) => {
                let v = self.eval(e)?;
                let target = Expr {
                    kind: ExprKind::Var(d.name.clone()),
                    pos: d.pos,
                };
                self.store(&target, v)?;
            }
            Some(Initializer::List(items)) => {
                for (i, item) in items.iter().enumerate() {
                    let v = self.eval(item)?;
                    let target = Expr {
                        kind: ExprKind::Index(
                            d.name.clone(),
                            Box::new(Expr {
                                kind: ExprKind::Int(i as i64),
                                pos: d.pos,
                            }),
                        ),
                        pos: d.pos,
                    };
                    self.store(&target, v)?;
                }
            }
        }
        Ok(())
    }

    fn stmt(&mut self, s: &Stmt) -> R<()> {
        // Temporaries never survive a statement.
        self.cur().temp_sp = 0;
        match &s.kind {
            StmtKind::Empty => Ok(()),
            StmtKind::Expr(e) => {
                match &e.kind {
                    ExprKind::IncDec {
                        target,
                        delta,
                        prefix,
                    } => {
                        self.eval_incdec(target, *delta, *prefix, true)?;
                    }
                    _ => {
                        self.eval(e)?;
                    }
                }
                Ok(())
            }
            StmtKind::Decl(d) => self.declare_local(d),
            StmtKind::Block(list) => self.stmts(list),
            StmtKind::If(c, then, els) => {
                let lelse = self.new_label();
                self.cond(c, &lelse, false)?;
                self.stmt(then)?;
                match els {
                    Some(e) => {
                        let lend = self.new_label();
                        self.emit(format!("jmp {}", lend));
                        self.emit_label(&lelse);
                        self.stmt(e)?;
                        self.emit_label(&lend);
                    }
                    None => self.emit_label(&lelse),
                }
                Ok(())
            }
            StmtKind::While(c, body) => {
                let ltop = self.new_label();
                let lend = self.new_label();
                self.emit_label(&ltop);
                self.cond(c, &lend, false)?;
                self.cur().loops.push((lend.clone(), ltop.clone()));
                let r = self.stmt(body);
                self.cur().loops.pop();
                r?;
                self.emit(format!("jmp {}", ltop));
                self.emit_label(&lend);
                Ok(())
            }
            StmtKind::DoWhile(body, c) => {
                let ltop = self.new_label();
                let lcont = self.new_label();
                let lend = self.new_label();
                self.emit_label(&ltop);
                self.cur().loops.push((lend.clone(), lcont.clone()));
                let r = self.stmt(body);
                self.cur().loops.pop();
                r?;
                self.emit_label(&lcont);
                self.cond(c, &ltop, true)?;
                self.emit_label(&lend);
                Ok(())
            }
            StmtKind::For(init, c, step, body) => {
                self.cur().scopes.push(HashMap::new());
                let saved = self.cur().locals_size;
                let r = (|| -> R<()> {
                    if let Some(i) = init {
                        match &i.kind {
                            StmtKind::Block(list) => {
                                for s in list {
                                    self.stmt(s)?;
                                }
                            }
                            _ => self.stmt(i)?,
                        }
                    }
                    let ltop = self.new_label();
                    let lcont = self.new_label();
                    let lend = self.new_label();
                    self.emit_label(&ltop);
                    if let Some(c) = c {
                        self.cond(c, &lend, false)?;
                    }
                    self.cur().loops.push((lend.clone(), lcont.clone()));
                    let r = self.stmt(body);
                    self.cur().loops.pop();
                    r?;
                    self.emit_label(&lcont);
                    if let Some(st) = step {
                        self.cur().temp_sp = 0;
                        match &st.kind {
                            ExprKind::IncDec {
                                target,
                                delta,
                                prefix,
                            } => {
                                self.eval_incdec(target, *delta, *prefix, true)?;
                            }
                            _ => {
                                self.eval(st)?;
                            }
                        }
                    }
                    self.emit(format!("jmp {}", ltop));
                    self.emit_label(&lend);
                    Ok(())
                })();
                self.cur().scopes.pop();
                self.cur().locals_size = saved;
                r
            }
            StmtKind::Switch(subject, cases) => {
                let ty = self.type_of(subject)?;
                let v = self.eval(subject)?;
                let v = if ty.size() == 1 {
                    self.load_acc(&v);
                    self.free(&v.loc);
                    Val::new(self.acc_to_temp(), ty)
                } else {
                    let v = self.materialize16(v);
                    let t = self.copy16_to_temp(&v);
                    Val::new(t, ty)
                };
                let lend = self.new_label();
                let mut labels = Vec::new();
                let mut default_label = None;
                for c in cases {
                    let l = self.new_label();
                    if c.is_default {
                        default_label = Some(l.clone());
                    }
                    for val_expr in &c.values {
                        let val = match self.fold(val_expr) {
                            Some(v) => ty.wrap(v),
                            None => {
                                return err(
                                    val_expr.pos,
                                    "case label must be a constant expression",
                                )
                            }
                        };
                        if ty.size() == 1 {
                            let a = self.addr(&v.loc, 0);
                            self.emit(format!("mov a,{}", a));
                            self.emit(format!("xor a,{}", val & 0xFF));
                            self.emit("sz z");
                            self.emit(format!("jmp {}", l));
                        } else {
                            let next = self.new_label();
                            let a0 = self.addr(&v.loc, 0);
                            let a1 = self.addr(&v.loc, 1);
                            self.emit(format!("mov a,{}", a0));
                            self.emit(format!("xor a,{}", val & 0xFF));
                            self.emit("snz z");
                            self.emit(format!("jmp {}", next));
                            self.emit(format!("mov a,{}", a1));
                            self.emit(format!("xor a,{}", (val >> 8) & 0xFF));
                            self.emit("sz z");
                            self.emit(format!("jmp {}", l));
                            self.emit_label(&next);
                        }
                    }
                    labels.push(l);
                }
                self.free(&v.loc);
                self.emit(format!(
                    "jmp {}",
                    default_label.clone().unwrap_or_else(|| lend.clone())
                ));
                let cont = self
                    .cur()
                    .loops
                    .last()
                    .map(|l| l.1.clone())
                    .unwrap_or_default();
                self.cur().loops.push((lend.clone(), cont));
                let mut result = Ok(());
                for (c, l) in cases.iter().zip(&labels) {
                    self.emit_label(l);
                    if let Err(e) = self.stmts(&c.body) {
                        result = Err(e);
                        break;
                    }
                }
                self.cur().loops.pop();
                result?;
                self.emit_label(&lend);
                Ok(())
            }
            StmtKind::Break => {
                let l = match self.cur().loops.last() {
                    Some((b, _)) => b.clone(),
                    None => return err(s.pos, "'break' outside of a loop or switch"),
                };
                self.emit(format!("jmp {}", l));
                Ok(())
            }
            StmtKind::Continue => {
                let l = match self.cur().loops.iter().rev().find(|(_, c)| !c.is_empty()) {
                    Some((_, c)) => c.clone(),
                    None => return err(s.pos, "'continue' outside of a loop"),
                };
                self.emit(format!("jmp {}", l));
                Ok(())
            }
            StmtKind::Return(e) => {
                let ret = self.cur().ret;
                match (e, ret) {
                    (None, Type::Void) => {}
                    (None, _) => return err(s.pos, "missing return value"),
                    (Some(_), Type::Void) => {
                        return err(s.pos, "void function cannot return a value")
                    }
                    (Some(e), t) => {
                        let v = self.eval(e)?;
                        let v = self.convert(v, t);
                        if t.size() == 1 {
                            self.load_acc(&v);
                        } else {
                            let v = self.materialize16(v);
                            self.copy16_into(&v.loc, &Loc::Mem("__ret0".into()));
                            self.needs_ret16 = true;
                        }
                    }
                }
                if self.cur().isr {
                    let l = self.cur().exit_label.clone();
                    self.emit(format!("jmp {}", l));
                } else {
                    self.emit("ret");
                }
                Ok(())
            }
            StmtKind::Asm(text) => {
                for line in text.lines() {
                    let t = line.trim();
                    if !t.is_empty() {
                        if t.ends_with(':') {
                            self.cur().lines.push(t.to_string());
                        } else {
                            self.emit(t.to_string());
                        }
                    }
                }
                // Inline assembly may use anything: be conservative.
                self.cur().uses.mp0 = true;
                self.cur().uses.table = true;
                Ok(())
            }
        }
    }

    // -----------------------------------------------------------------
    // Functions and program
    // -----------------------------------------------------------------

    fn gen_function(&mut self, f: &FnDecl) -> R<()> {
        let info = self.fns.get(&f.name).unwrap().clone();
        let exit_label = format!("_{}_exit", f.name);
        let mut cur = Cur {
            id: info.id,
            ret: f.ret,
            isr: f.interrupt.is_some(),
            scopes: vec![HashMap::new()],
            locals_size: 0,
            locals_max: 0,
            temp_sp: 0,
            temp_max: 0,
            loops: Vec::new(),
            lines: Vec::new(),
            calls: BTreeSet::new(),
            uses: Uses::default(),
            exit_label: exit_label.clone(),
        };
        for p in &f.params {
            let off = cur.locals_size;
            cur.locals_size += p.ty.size();
            cur.scopes[0].insert(
                p.name.clone(),
                Local {
                    ty: p.ty,
                    len: None,
                    off: Some(off),
                    constant: None,
                },
            );
        }
        cur.locals_max = cur.locals_size;
        self.cur = Some(cur);
        self.emit_label(&format!("_{}", f.name));
        let body_result = self.stmts(&f.body);
        let cur = self.cur.take().unwrap();
        let (lines, locals_max, temp_max) = (cur.lines, cur.locals_max, cur.temp_max);
        let (calls, uses) = (cur.calls, cur.uses);
        body_result?;
        let mut lines = lines;
        let last = lines
            .last()
            .map(|l| l.trim().to_string())
            .unwrap_or_default();
        if f.interrupt.is_some() {
            lines.push(format!("{}:", exit_label));
        } else if last != "ret" {
            if f.ret != Type::Void {
                self.warnings.push(format!(
                    "{}:{}: function '{}' may reach its end without returning a value",
                    f.pos.line, f.pos.col, f.name
                ));
            }
            lines.push("        ret".into());
        }
        let info = self.fns.get_mut(&f.name).unwrap();
        info.lines = lines;
        info.frame_size = locals_max + temp_max;
        info.calls = calls;
        info.uses = uses;
        info.generated = true;
        // T symbol: temporaries start after locals.
        info.lines
            .insert(0, format!("T{} equ F{}+{}", info.id, info.id, locals_max));
        Ok(())
    }

    fn declare_global(&mut self, d: &VarDecl) -> R<()> {
        if self.globals.contains_key(&d.name) || self.fns.contains_key(&d.name) {
            return err(d.pos, format!("'{}' already defined", d.name));
        }
        if device::sfr_by_name(&d.name).is_some() && d.name.chars().all(|c| !c.is_ascii_lowercase())
        {
            return err(
                d.pos,
                format!("'{}' is a special function register name", d.name),
            );
        }
        let sym = format!("_{}", d.name);
        let array_len = self.array_len_of(d)?;
        let kind = if d.is_const {
            match (&d.init, array_len) {
                (Some(Initializer::Scalar(e)), None) => match self.fold(e) {
                    Some(v) => GlobalKind::ConstScalar(d.ty.wrap(v)),
                    None => return err(d.pos, "const initializer must be a constant expression"),
                },
                (Some(Initializer::List(items)), Some(n)) => {
                    let mut vals = Vec::with_capacity(n);
                    for it in items {
                        match self.fold(it) {
                            Some(v) => vals.push(d.ty.wrap(v) & 0xFFFF),
                            None => {
                                return err(
                                    it.pos,
                                    "const table elements must be constant expressions",
                                )
                            }
                        }
                    }
                    vals.resize(n, 0);
                    GlobalKind::ConstTable(vals)
                }
                _ => return err(d.pos, "const variable needs an initializer"),
            }
        } else {
            GlobalKind::Ram
        };
        if d.at.is_some() && d.is_const {
            return err(d.pos, "const variables cannot be placed at an address");
        }
        self.globals.insert(
            d.name.clone(),
            Global {
                ty: d.ty,
                len: array_len,
                sym,
                kind,
                fixed: d.at,
                pos: d.pos,
            },
        );
        self.global_order.push(d.name.clone());
        Ok(())
    }

    /// Generate startup initialisation code for a global.
    fn init_global(&mut self, d: &VarDecl) -> R<()> {
        if d.is_const {
            return Ok(());
        }
        let init = match &d.init {
            Some(i) => i.clone(),
            None => return Ok(()),
        };
        // Use a throw-away "current function" context so that expression
        // evaluation works; only constant initialisers are allowed.
        match init {
            Initializer::Scalar(e) => {
                let v = match self.fold(&e) {
                    Some(v) => v,
                    None => return err(e.pos, "global initializers must be constant expressions"),
                };
                let target = Expr {
                    kind: ExprKind::Var(d.name.clone()),
                    pos: d.pos,
                };
                let lit = Expr {
                    kind: ExprKind::Int(v),
                    pos: e.pos,
                };
                let vv = self.eval(&lit)?;
                self.store(&target, vv)?;
            }
            Initializer::List(items) => {
                for (i, it) in items.iter().enumerate() {
                    let v = match self.fold(it) {
                        Some(v) => v,
                        None => {
                            return err(it.pos, "global initializers must be constant expressions")
                        }
                    };
                    let target = Expr {
                        kind: ExprKind::Index(
                            d.name.clone(),
                            Box::new(Expr {
                                kind: ExprKind::Int(i as i64),
                                pos: d.pos,
                            }),
                        ),
                        pos: d.pos,
                    };
                    let lit = Expr {
                        kind: ExprKind::Int(v),
                        pos: it.pos,
                    };
                    let vv = self.eval(&lit)?;
                    self.store(&target, vv)?;
                }
            }
        }
        Ok(())
    }

    /// Compile a whole program.
    pub fn compile(mut self, prog: &Program) -> Result<Output, Vec<CompileError>> {
        let mut errors = Vec::new();
        // Declarations.
        for d in &prog.globals {
            if let Err(e) = self.declare_global(d) {
                errors.push(e);
            }
        }
        for (i, f) in prog.functions.iter().enumerate() {
            if self.fns.contains_key(&f.name) || self.globals.contains_key(&f.name) {
                errors.push(CompileError {
                    line: f.pos.line,
                    col: f.pos.col,
                    message: format!("'{}' already defined", f.name),
                });
                continue;
            }
            if let Some(v) = f.interrupt {
                if device::vector_name(v).is_none() {
                    errors.push(CompileError {
                        line: f.pos.line,
                        col: f.pos.col,
                        message: format!("{:#04x} is not an HT66F0185 interrupt vector", v),
                    });
                }
                if f.name == "main" {
                    errors.push(CompileError {
                        line: f.pos.line,
                        col: f.pos.col,
                        message: "main cannot be an interrupt handler".into(),
                    });
                }
            }
            self.fns.insert(
                f.name.clone(),
                FnInfo {
                    id: i,
                    ret: f.ret,
                    params: f.params.clone(),
                    isr: f.interrupt,
                    calls: BTreeSet::new(),
                    frame_size: 0,
                    uses: Uses::default(),
                    lines: Vec::new(),
                    generated: false,
                },
            );
            self.fn_order.push(f.name.clone());
        }
        if !self.fns.contains_key("main") {
            errors.push(CompileError {
                line: 1,
                col: 1,
                message: "no 'main' function defined".into(),
            });
        }
        if !errors.is_empty() {
            return Err(errors);
        }
        // Startup initialisers (generated inside a pseudo-function context).
        self.cur = Some(Cur {
            id: usize::MAX,
            ret: Type::Void,
            isr: false,
            scopes: vec![HashMap::new()],
            locals_size: 0,
            locals_max: 0,
            temp_sp: 0,
            temp_max: 0,
            loops: Vec::new(),
            lines: Vec::new(),
            calls: BTreeSet::new(),
            uses: Uses::default(),
            exit_label: String::new(),
        });
        for d in &prog.globals {
            if let Err(e) = self.init_global(d) {
                errors.push(e);
            }
        }
        let start = self.cur.take().unwrap();
        if start.temp_max > 0 {
            errors.push(CompileError {
                line: 1,
                col: 1,
                message: "internal: startup code used temporaries".into(),
            });
        }
        self.startup = start.lines;
        // Functions.
        for f in &prog.functions {
            if let Err(e) = self.gen_function(f) {
                errors.push(e);
            }
        }
        if !errors.is_empty() {
            return Err(errors);
        }
        self.link().map_err(|e| vec![e])
    }

    /// Lay out RAM, check the call graph and produce the final assembly.
    fn link(mut self) -> R<Output> {
        // --- call graph checks -------------------------------------------
        let names: Vec<String> = self.fn_order.clone();
        // recursion (cycle) detection
        {
            fn dfs(
                n: &str,
                fns: &BTreeMap<String, FnInfo>,
                stack: &mut Vec<String>,
                done: &mut BTreeSet<String>,
            ) -> Option<Vec<String>> {
                if let Some(i) = stack.iter().position(|s| s == n) {
                    return Some(stack[i..].to_vec());
                }
                if done.contains(n) {
                    return None;
                }
                stack.push(n.to_string());
                if let Some(f) = fns.get(n) {
                    for c in &f.calls {
                        if let Some(cycle) = dfs(c, fns, stack, done) {
                            return Some(cycle);
                        }
                    }
                }
                stack.pop();
                done.insert(n.to_string());
                None
            }
            let mut done = BTreeSet::new();
            for n in &names {
                if let Some(cycle) = dfs(n, &self.fns, &mut Vec::new(), &mut done) {
                    return Err(CompileError {
                        line: 1,
                        col: 1,
                        message: format!(
                            "recursion is not supported (hardware stack only): {}",
                            cycle.join(" -> ")
                        ),
                    });
                }
            }
        }
        // reachability sets
        let reach = |root: &str, fns: &BTreeMap<String, FnInfo>| -> BTreeSet<String> {
            let mut seen = BTreeSet::new();
            let mut todo = vec![root.to_string()];
            while let Some(n) = todo.pop() {
                if !seen.insert(n.clone()) {
                    continue;
                }
                if let Some(f) = fns.get(&n) {
                    for c in &f.calls {
                        todo.push(c.clone());
                    }
                }
            }
            seen
        };
        let main_ctx = reach("main", &self.fns);
        let isrs: Vec<String> = names
            .iter()
            .filter(|n| self.fns[*n].isr.is_some())
            .cloned()
            .collect();
        let mut isr_ctx: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for i in &isrs {
            isr_ctx.insert(i.clone(), reach(i, &self.fns));
        }
        for (i, ctx) in &isr_ctx {
            for n in ctx {
                if n != i && main_ctx.contains(n) && self.fns.contains_key(n) {
                    self.warnings.push(format!(
                        "function '{}' is called both from main code and from interrupt handler '{}'; it is not re-entrant",
                        n, i
                    ));
                }
            }
        }
        // stack depth check (8 levels, one reserved per interrupt nesting)
        fn depth(n: &str, fns: &BTreeMap<String, FnInfo>) -> usize {
            let f = match fns.get(n) {
                Some(f) => f,
                None => return 1, // runtime routine (may call one more)
            };
            1 + f.calls.iter().map(|c| depth(c, fns)).max().unwrap_or(0)
        }
        let main_depth = depth("main", &self.fns) + 1; // +1 for the runtime's nested call
        let isr_depth = isrs
            .iter()
            .map(|i| depth(i, &self.fns) + 1)
            .max()
            .unwrap_or(0);
        if main_depth + isr_depth > device::STACK_LEVELS {
            self.warnings.push(format!(
                "call nesting may exceed the {}-level hardware stack (main: {}, interrupt: {})",
                device::STACK_LEVELS,
                main_depth,
                isr_depth
            ));
        }

        // --- RAM layout ---------------------------------------------------
        let mut equ_lines: Vec<String> = Vec::new();
        let mut ram = device::GP_RAM_START as usize;
        let mut fixed_used: Vec<(u8, usize, String)> = Vec::new();
        for name in &self.global_order {
            let g = self.globals[name].clone();
            match g.kind {
                GlobalKind::Ram => {
                    let size = g.len.unwrap_or(1) * g.ty.size();
                    if let Some(addr) = g.fixed {
                        equ_lines.push(format!("{} equ [{}]", g.sym, addr));
                        fixed_used.push((addr, size, name.clone()));
                    } else {
                        if ram + size > 0x100 {
                            return err(
                                g.pos,
                                format!("out of data memory while allocating '{}'", name),
                            );
                        }
                        equ_lines.push(format!("{} equ [{}]", g.sym, ram));
                        ram += size;
                    }
                }
                GlobalKind::ConstScalar(_) => {}
                GlobalKind::ConstTable(_) => {}
            }
        }
        // runtime scratch, 16-bit return slot, ISR save slots
        let alloc = |name: &str, size: usize, ram: &mut usize, equ: &mut Vec<String>| -> R<()> {
            if *ram + size > 0x100 {
                return Err(CompileError {
                    line: 1,
                    col: 1,
                    message: format!("out of data memory while allocating '{}'", name),
                });
            }
            equ.push(format!("{} equ [{}]", name, *ram));
            *ram += size;
            Ok(())
        };
        for i in 0..self.scratch_bytes {
            alloc(&format!("__r{}", i), 1, &mut ram, &mut equ_lines)?;
        }
        if self.needs_ret16 {
            alloc("__ret0", 1, &mut ram, &mut equ_lines)?;
            alloc("__ret1", 1, &mut ram, &mut equ_lines)?;
        }
        // ISR save slots depend on what the ISR context uses.
        let mut isr_uses: BTreeMap<String, Uses> = BTreeMap::new();
        for (i, ctx) in &isr_ctx {
            let mut u = Uses::default();
            for n in ctx {
                if let Some(f) = self.fns.get(n) {
                    u = u.union(f.uses);
                }
                if n.starts_with("__") {
                    u.runtime = true;
                }
            }
            isr_uses.insert(i.clone(), u);
            alloc(&format!("__sv_acc_{}", i), 1, &mut ram, &mut equ_lines)?;
            alloc(&format!("__sv_st_{}", i), 1, &mut ram, &mut equ_lines)?;
            if u.mp0 {
                alloc(&format!("__sv_mp0_{}", i), 1, &mut ram, &mut equ_lines)?;
            }
            if u.table {
                alloc(&format!("__sv_tblp_{}", i), 1, &mut ram, &mut equ_lines)?;
                alloc(&format!("__sv_tbhp_{}", i), 1, &mut ram, &mut equ_lines)?;
            }
            if u.runtime {
                alloc(
                    &format!("__sv_rt_{}", i),
                    self.scratch_bytes,
                    &mut ram,
                    &mut equ_lines,
                )?;
            }
        }
        let frames_start = ram;

        // --- frame layout (overlay) -----------------------------------------
        let mut base: BTreeMap<String, usize> = BTreeMap::new();
        // callers map
        let mut callers: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for n in &names {
            for c in &self.fns[n].calls {
                callers.entry(c.clone()).or_default().push(n.clone());
            }
        }
        base.insert("main".into(), frames_start);
        // Unreachable, non-ISR functions: treat as roots at frames_start.
        let mut all_reach: BTreeSet<String> = main_ctx.clone();
        for ctx in isr_ctx.values() {
            all_reach.extend(ctx.iter().cloned());
        }
        for n in &names {
            if !all_reach.contains(n) && self.fns[n].isr.is_none() {
                base.insert(n.clone(), frames_start);
            }
        }
        let relax = |base: &mut BTreeMap<String, usize>,
                     fns: &BTreeMap<String, FnInfo>,
                     callers: &BTreeMap<String, Vec<String>>,
                     names: &[String],
                     roots: &BTreeSet<String>| {
            loop {
                let mut changed = false;
                for n in names {
                    if roots.contains(n) {
                        continue;
                    }
                    let mut b = base.get(n).copied().unwrap_or(0);
                    if let Some(cs) = callers.get(n) {
                        for c in cs {
                            if let Some(cb) = base.get(c) {
                                let cand = cb + fns[c].frame_size;
                                if cand > b {
                                    b = cand;
                                }
                            }
                        }
                    }
                    if base.get(n).copied() != Some(b) {
                        base.insert(n.clone(), b);
                        changed = true;
                    }
                }
                if !changed {
                    break;
                }
            }
        };
        let mut roots: BTreeSet<String> = base.keys().cloned().collect();
        // Main context first.
        let main_names: Vec<String> = names
            .iter()
            .filter(|n| main_ctx.contains(*n))
            .cloned()
            .collect();
        relax(&mut base, &self.fns, &callers, &main_names, &roots);
        let main_extent = main_names
            .iter()
            .map(|n| base[n] + self.fns[n].frame_size)
            .max()
            .unwrap_or(frames_start);
        for i in &isrs {
            base.insert(i.clone(), main_extent);
            roots.insert(i.clone());
        }
        relax(&mut base, &self.fns, &callers, &names, &roots);
        let mut ram_end = frames_start;
        let mut frames = Vec::new();
        for n in &names {
            let f = &self.fns[n];
            let b = base[n];
            let end = b + f.frame_size;
            if end > 0x100 {
                return Err(CompileError {
                    line: 1,
                    col: 1,
                    message: format!(
                        "out of data memory: frame of '{}' would end at {:#04x}",
                        n, end
                    ),
                });
            }
            ram_end = ram_end.max(end);
            equ_lines.push(format!("F{} equ {}", f.id, b));
            frames.push((n.clone(), b as u8, f.frame_size));
        }
        for (addr, size, name) in &fixed_used {
            let a = *addr as usize;
            if a >= 0x80 && a + size > device::GP_RAM_START as usize && a < ram_end {
                self.warnings.push(format!(
                    "fixed-address variable '{}' at {:#04x} overlaps allocated data memory",
                    name, addr
                ));
            }
        }

        // --- assemble the text ---------------------------------------------
        let mut out = String::new();
        out.push_str("; generated by htc for the Holtek HT66F0185\n");
        for l in &equ_lines {
            out.push_str(l);
            out.push('\n');
        }
        out.push('\n');
        out.push_str("        org 000h\n        jmp __start\n");
        let mut vectors: BTreeMap<u16, String> = BTreeMap::new();
        for i in &isrs {
            let v = self.fns[i].isr.unwrap();
            if let Some(other) = vectors.insert(v, i.clone()) {
                return Err(CompileError {
                    line: 1,
                    col: 1,
                    message: format!(
                        "interrupt vector {:#04x} used by both '{}' and '{}'",
                        v, other, i
                    ),
                });
            }
        }
        for (v, name) in &vectors {
            out.push_str(&format!(
                "        org {:03x}h        ; {} interrupt\n        jmp _{}\n",
                v,
                device::vector_name(*v).unwrap_or("?"),
                name
            ));
        }
        let code_start = vectors.keys().max().map(|v| v + 4).unwrap_or(1);
        out.push_str(&format!("        org {:03x}h\n", code_start));
        out.push_str("__start:\n");
        out.push_str("        clr bp\n");
        out.push_str("        mov a,080h\n        mov mp0,a\n__clr_ram:\n        clr iar0\n        inc mp0\n        sz mp0\n        jmp __clr_ram\n");
        for l in &self.startup {
            out.push_str(l);
            out.push('\n');
        }
        out.push_str("        call _main\n__exit:\n        halt\n        jmp __exit\n");
        for n in &names {
            let f = self.fns[n].clone();
            out.push('\n');
            let mut lines = peephole(f.lines.clone());
            if f.isr.is_some() {
                let u = isr_uses[n];
                let mut pro = vec![
                    format!("        mov [__sv_acc_{}],a", n),
                    "        mov a,status".into(),
                    format!("        mov [__sv_st_{}],a", n),
                ];
                let mut epi = Vec::new();
                if u.mp0 {
                    pro.push("        mov a,mp0".into());
                    pro.push(format!("        mov [__sv_mp0_{}],a", n));
                    epi.push(format!("        mov a,[__sv_mp0_{}]", n));
                    epi.push("        mov mp0,a".into());
                }
                if u.table {
                    pro.push("        mov a,tblp".into());
                    pro.push(format!("        mov [__sv_tblp_{}],a", n));
                    pro.push("        mov a,tbhp".into());
                    pro.push(format!("        mov [__sv_tbhp_{}],a", n));
                    epi.push(format!("        mov a,[__sv_tblp_{}]", n));
                    epi.push("        mov tblp,a".into());
                    epi.push(format!("        mov a,[__sv_tbhp_{}]", n));
                    epi.push("        mov tbhp,a".into());
                }
                if u.runtime {
                    for i in 0..self.scratch_bytes {
                        pro.push(format!("        mov a,[__r{}]", i));
                        pro.push(format!("        mov [__sv_rt_{}+{}],a", n, i));
                        epi.push(format!("        mov a,[__sv_rt_{}+{}]", n, i));
                        epi.push(format!("        mov [__r{}],a", i));
                    }
                }
                epi.push(format!("        mov a,[__sv_st_{}]", n));
                epi.push("        mov status,a".into());
                epi.push(format!("        mov a,[__sv_acc_{}]", n));
                epi.push("        reti".into());
                // lines[0] is the T equ, lines[1] the label
                lines.splice(2..2, pro);
                lines.extend(epi);
            }
            for l in &lines {
                out.push_str(l);
                out.push('\n');
            }
        }
        // runtime
        let mut emitted = BTreeSet::new();
        for r in &self.runtime_used {
            for (name, src) in runtime::routine_source(r) {
                if emitted.insert(name) {
                    out.push_str(src);
                }
            }
        }
        // const tables
        let mut any_table = false;
        for name in &self.global_order {
            let g = &self.globals[name];
            if let GlobalKind::ConstTable(vals) = &g.kind {
                if !any_table {
                    out.push_str("\n; --- constant tables (program memory)\n");
                    any_table = true;
                }
                out.push_str(&format!("{}:\n", g.sym));
                for chunk in vals.chunks(8) {
                    let items: Vec<String> = chunk.iter().map(|v| format!("{}", v)).collect();
                    out.push_str(&format!("        dc {}\n", items.join(",")));
                }
            }
        }
        Ok(Output {
            asm: out,
            warnings: self.warnings,
            ram_used: ram_end - device::GP_RAM_START as usize,
            frames,
        })
    }
}

impl Default for Gen {
    fn default() -> Self {
        Self::new()
    }
}

/// Tiny peephole optimiser over generated assembly lines.
///
/// * `mov [X],a` followed by `mov a,[X]` — the reload is redundant.
/// * `jmp L` immediately followed by `L:` — the jump is redundant.
/// * `mov a,…` immediately followed by another `mov a,…` — the first load is dead.
fn peephole(lines: Vec<String>) -> Vec<String> {
    fn is_skip(line: &str) -> bool {
        let t = line.trim();
        ["sz ", "snz ", "siz ", "sdz ", "sza ", "siza ", "sdza "]
            .iter()
            .any(|m| t.starts_with(m))
    }
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    for line in lines {
        let t = line.trim();
        // Never touch an instruction that is the target of a skip.
        let guarded = out.len() >= 2 && is_skip(&out[out.len() - 2]);
        if let (Some(prev), false) = (out.last(), guarded) {
            let p = prev.trim();
            // redundant reload
            if let (Some(dst), Some(src)) = (
                p.strip_prefix("mov ").and_then(|x| x.strip_suffix(",a")),
                t.strip_prefix("mov a,"),
            ) {
                if dst == src && !dst.starts_with("iar") && !dst.starts_with("pcl") {
                    continue;
                }
            }
            // jump to the next line
            if let Some(target) = p.strip_prefix("jmp ") {
                if let Some(label) = t.strip_suffix(':') {
                    if target == label {
                        out.pop();
                        out.push(line);
                        continue;
                    }
                }
            }
            // dead load into ACC
            if p.starts_with("mov a,") && t.starts_with("mov a,") && !p.contains("iar") {
                out.pop();
            }
        }
        // `sz b / jmp A / jmp B / A:`  →  `snz b / jmp B / A:` (bit skips only)
        if let Some(label) = t.strip_suffix(':') {
            let n = out.len();
            if n >= 3 {
                let (s0, s1, s2) = (
                    out[n - 3].trim().to_string(),
                    out[n - 2].trim().to_string(),
                    out[n - 1].trim().to_string(),
                );
                let skip_inv = if let Some(b) = s0.strip_prefix("sz ") {
                    if b.contains('.') {
                        Some(format!("snz {}", b))
                    } else {
                        None
                    }
                } else {
                    s0.strip_prefix("snz ").map(|b| format!("sz {}", b))
                };
                let not_guarded = n < 4 || !is_skip(&out[n - 4]);
                if let (Some(inv), Some(a), Some(_b)) =
                    (skip_inv, s1.strip_prefix("jmp "), s2.strip_prefix("jmp "))
                {
                    if a == label && not_guarded {
                        let jmp_b = out[n - 1].clone();
                        out.truncate(n - 3);
                        out.push(format!("        {}", inv));
                        out.push(jmp_b);
                    }
                }
            }
        }
        out.push(line);
    }
    out
}
