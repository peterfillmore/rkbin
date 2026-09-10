//! Abstract syntax tree of the `htc` language.

use std::fmt;

/// Scalar types.  All values are 8 or 16 bits wide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Type {
    Void,
    Bool,
    U8,
    I8,
    U16,
    I16,
}

impl Type {
    pub fn size(self) -> usize {
        match self {
            Type::Void => 0,
            Type::Bool | Type::U8 | Type::I8 => 1,
            Type::U16 | Type::I16 => 2,
        }
    }

    pub fn is_signed(self) -> bool {
        matches!(self, Type::I8 | Type::I16)
    }

    pub fn is_integer(self) -> bool {
        !matches!(self, Type::Void)
    }

    pub fn from_name(s: &str) -> Option<Type> {
        Some(match s {
            "void" => Type::Void,
            "bool" => Type::Bool,
            "u8" | "uint8_t" | "unsigned" | "char" | "uchar" => Type::U8,
            "i8" | "int8_t" | "schar" => Type::I8,
            "u16" | "uint16_t" => Type::U16,
            "i16" | "int16_t" | "int" | "short" => Type::I16,
            _ => return None,
        })
    }

    /// Common type of a binary arithmetic operation.
    pub fn common(a: Type, b: Type) -> Type {
        let size = a.size().max(b.size()).max(1);
        let signed = a.is_signed() || b.is_signed();
        match (size, signed) {
            (1, false) => Type::U8,
            (1, true) => Type::I8,
            (_, false) => Type::U16,
            (_, true) => Type::I16,
        }
    }

    /// Truncate/extend a constant to this type's value range (wrapping).
    pub fn wrap(self, v: i64) -> i64 {
        match self {
            Type::Void => 0,
            Type::Bool => (v != 0) as i64,
            Type::U8 => v & 0xFF,
            Type::I8 => ((v & 0xFF) as u8) as i8 as i64,
            Type::U16 => v & 0xFFFF,
            Type::I16 => ((v & 0xFFFF) as u16) as i16 as i64,
        }
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Type::Void => "void",
            Type::Bool => "bool",
            Type::U8 => "u8",
            Type::I8 => "i8",
            Type::U16 => "u16",
            Type::I16 => "i16",
        };
        write!(f, "{}", s)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    And,
    Or,
    Xor,
    Shl,
    Shr,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    LAnd,
    LOr,
}

impl BinOp {
    pub fn is_comparison(self) -> bool {
        matches!(
            self,
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
        )
    }

    pub fn is_logical(self) -> bool {
        matches!(self, BinOp::LAnd | BinOp::LOr)
    }

    pub fn symbol(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Rem => "%",
            BinOp::And => "&",
            BinOp::Or => "|",
            BinOp::Xor => "^",
            BinOp::Shl => "<<",
            BinOp::Shr => ">>",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::LAnd => "&&",
            BinOp::LOr => "||",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
    LNot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pos {
    pub line: usize,
    pub col: usize,
}

#[derive(Debug, Clone)]
pub struct Expr {
    pub kind: ExprKind,
    pub pos: Pos,
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    Int(i64),
    Bool(bool),
    Var(String),
    Index(String, Box<Expr>),
    /// `lvalue.bit` — bit access on an 8-bit lvalue.
    Bit(Box<Expr>, u8),
    Call(String, Vec<Expr>),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    Assign(Box<Expr>, Box<Expr>),
    /// `lhs op= rhs`
    CompoundAssign(BinOp, Box<Expr>, Box<Expr>),
    /// `++x`, `--x` (prefix) or `x++`, `x--` (postfix)
    IncDec {
        target: Box<Expr>,
        delta: i64,
        prefix: bool,
    },
    Cast(Type, Box<Expr>),
    Ternary(Box<Expr>, Box<Expr>, Box<Expr>),
}

#[derive(Debug, Clone)]
pub struct Stmt {
    pub kind: StmtKind,
    pub pos: Pos,
}

#[derive(Debug, Clone)]
pub enum StmtKind {
    Expr(Expr),
    /// Local variable declaration.
    Decl(VarDecl),
    Block(Vec<Stmt>),
    If(Expr, Box<Stmt>, Option<Box<Stmt>>),
    While(Expr, Box<Stmt>),
    DoWhile(Box<Stmt>, Expr),
    For(Option<Box<Stmt>>, Option<Expr>, Option<Expr>, Box<Stmt>),
    Switch(Expr, Vec<SwitchCase>),
    Break,
    Continue,
    Return(Option<Expr>),
    Asm(String),
    Empty,
}

#[derive(Debug, Clone)]
pub struct SwitchCase {
    /// Constant case labels (folded by the code generator).
    pub values: Vec<Expr>,
    pub is_default: bool,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone)]
pub struct VarDecl {
    pub name: String,
    pub ty: Type,
    /// Array length (None for scalars); `Some(0)` when it is taken from
    /// the initializer or from `array_len_expr`.
    pub array_len: Option<usize>,
    /// Unresolved array length expression (may reference named constants).
    pub array_len_expr: Option<Expr>,
    pub is_const: bool,
    /// Absolute placement `@ addr`.
    pub at: Option<u8>,
    pub init: Option<Initializer>,
    pub pos: Pos,
}

#[derive(Debug, Clone)]
pub enum Initializer {
    Scalar(Expr),
    List(Vec<Expr>),
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub ty: Type,
}

#[derive(Debug, Clone)]
pub struct FnDecl {
    pub name: String,
    pub ret: Type,
    pub params: Vec<Param>,
    pub body: Vec<Stmt>,
    /// Interrupt vector address if this is an ISR.
    pub interrupt: Option<u16>,
    pub pos: Pos,
}

#[derive(Debug, Clone, Default)]
pub struct Program {
    pub globals: Vec<VarDecl>,
    pub functions: Vec<FnDecl>,
}
