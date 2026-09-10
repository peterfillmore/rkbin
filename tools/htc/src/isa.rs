//! Instruction set architecture of the Holtek HT66F0185 8-bit RISC core.
//!
//! Every instruction is a single 16-bit program-memory word.  The encoding
//! was recovered from the HT-IDE3000 cross-assembler description files
//! (`*.fmt`) and cross-checked against `HGASM` listing files:
//!
//! | operand class        | fixed-bit mask | operand bits                                  |
//! |----------------------|----------------|-----------------------------------------------|
//! | none                 | `0xFFFF`       | –                                             |
//! | data memory `[m]`    | `0xBF80`       | `m[6:0]` in bits 6..0, `m[7]` in bit 14       |
//! | immediate `x`        | `0xFF00`       | `x[7:0]` in bits 7..0                         |
//! | program address      | `0x3800`       | `a[10:0]` in bits 10..0, `a[11]` in bit 14    |
//! | bit `[m].i`          | `0xBC00`       | `m[6:0]` in bits 6..0, `i` in bits 9..7, `m[7]` in bit 14 |
//!
//! `TABRD` and `TABRDC` share one encoding (the assembler treats them as
//! aliases, as HT-IDE does); `TABRDL` has its own.

use std::fmt;

/// Number of 16-bit words of program memory on the HT66F0185.
pub const PROGRAM_WORDS: usize = 0x1000;

/// Instruction mnemonics.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Op {
    // --- no operand ---------------------------------------------------
    Nop,
    ClrWdt,
    ClrWdt1,
    ClrWdt2,
    Halt,
    Ret,
    Reti,
    // --- data memory operand `[m]` ------------------------------------
    MovMA, // MOV [m],A
    Cpla,
    Cpl,
    Sub,  // SUB A,[m]
    Subm, // SUBM A,[m]
    Add,
    Addm,
    Xor,
    Xorm,
    Or,
    Orm,
    And,
    Andm,
    MovAM, // MOV A,[m]
    Sza,
    Sz,
    Swapa,
    Swap,
    Sbc,
    Sbcm,
    Adc,
    Adcm,
    Inca,
    Inc,
    Deca,
    Dec,
    Siza,
    Siz,
    Sdza,
    Sdz,
    Rla,
    Rl,
    Rra,
    Rr,
    Rlca,
    Rlc,
    Rrca,
    Rrc,
    Tabrd,
    Tabrdc,
    Tabrdl,
    Daa,
    Clr, // CLR [m]
    Set, // SET [m]
    // --- immediate operand `x` ----------------------------------------
    RetAI, // RET A,x
    SubI,  // SUB A,x
    AddI,
    XorI,
    OrI,
    AndI,
    MovAI, // MOV A,x
    // --- program address ----------------------------------------------
    Call,
    Jmp,
    // --- bit operand `[m].i` ------------------------------------------
    SetBit,
    ClrBit,
    SnzBit,
    SzBit,
}

/// The operand class of an instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperandKind {
    None,
    Mem,
    Imm,
    Addr,
    Bit,
}

/// One decoded instruction operand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operand {
    None,
    /// Data-memory address 0x00..=0xFF.
    Mem(u8),
    /// Immediate byte.
    Imm(u8),
    /// Program-memory address 0x000..=0xFFF.
    Addr(u16),
    /// Data-memory address and bit number 0..=7.
    Bit(u8, u8),
}

/// A decoded/encodable instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Instruction {
    pub op: Op,
    pub operand: Operand,
}

/// Static description of one opcode.
#[derive(Clone, Copy, Debug)]
pub struct OpInfo {
    pub op: Op,
    /// Fixed bits of the encoding.
    pub base: u16,
    pub kind: OperandKind,
    /// Assembler mnemonic (lower case).
    pub mnemonic: &'static str,
    /// Cycle count (minimum; skips and PCL writes add one).
    pub cycles: u8,
}

macro_rules! table {
    ($( $op:ident, $base:expr, $kind:ident, $mn:expr, $cy:expr; )*) => {
        /// Complete opcode table.
        pub const OPCODES: &[OpInfo] = &[ $( OpInfo { op: Op::$op, base: $base, kind: OperandKind::$kind, mnemonic: $mn, cycles: $cy }, )* ];
    };
}

table! {
    Nop,     0x0000, None, "nop", 1;
    ClrWdt,  0x0001, None, "clr wdt", 1;
    ClrWdt1, 0x0001, None, "clr wdt1", 1;
    ClrWdt2, 0x0005, None, "clr wdt2", 1;
    Halt,    0x0002, None, "halt", 1;
    Ret,     0x0003, None, "ret", 2;
    Reti,    0x0004, None, "reti", 2;

    MovMA,   0x0080, Mem, "mov", 1;
    Cpla,    0x0100, Mem, "cpla", 1;
    Cpl,     0x0180, Mem, "cpl", 1;
    Sub,     0x0200, Mem, "sub", 1;
    Subm,    0x0280, Mem, "subm", 1;
    Add,     0x0300, Mem, "add", 1;
    Addm,    0x0380, Mem, "addm", 1;
    Xor,     0x0400, Mem, "xor", 1;
    Xorm,    0x0480, Mem, "xorm", 1;
    Or,      0x0500, Mem, "or", 1;
    Orm,     0x0580, Mem, "orm", 1;
    And,     0x0600, Mem, "and", 1;
    Andm,    0x0680, Mem, "andm", 1;
    MovAM,   0x0700, Mem, "mov", 1;
    Sza,     0x1000, Mem, "sza", 1;
    Sz,      0x1080, Mem, "sz", 1;
    Swapa,   0x1100, Mem, "swapa", 1;
    Swap,    0x1180, Mem, "swap", 1;
    Sbc,     0x1200, Mem, "sbc", 1;
    Sbcm,    0x1280, Mem, "sbcm", 1;
    Adc,     0x1300, Mem, "adc", 1;
    Adcm,    0x1380, Mem, "adcm", 1;
    Inca,    0x1400, Mem, "inca", 1;
    Inc,     0x1480, Mem, "inc", 1;
    Deca,    0x1500, Mem, "deca", 1;
    Dec,     0x1580, Mem, "dec", 1;
    Siza,    0x1600, Mem, "siza", 1;
    Siz,     0x1680, Mem, "siz", 1;
    Sdza,    0x1700, Mem, "sdza", 1;
    Sdz,     0x1780, Mem, "sdz", 1;
    Rla,     0x1800, Mem, "rla", 1;
    Rl,      0x1880, Mem, "rl", 1;
    Rra,     0x1900, Mem, "rra", 1;
    Rr,      0x1980, Mem, "rr", 1;
    Rlca,    0x1A00, Mem, "rlca", 1;
    Rlc,     0x1A80, Mem, "rlc", 1;
    Rrca,    0x1B00, Mem, "rrca", 1;
    Rrc,     0x1B80, Mem, "rrc", 1;
    Tabrd,   0x1D00, Mem, "tabrd", 2;
    Tabrdc,  0x1D00, Mem, "tabrdc", 2;
    Tabrdl,  0x1D80, Mem, "tabrdl", 2;
    Daa,     0x1E80, Mem, "daa", 1;
    Clr,     0x1F00, Mem, "clr", 1;
    Set,     0x1F80, Mem, "set", 1;

    RetAI,   0x0900, Imm, "ret", 2;
    SubI,    0x0A00, Imm, "sub", 1;
    AddI,    0x0B00, Imm, "add", 1;
    XorI,    0x0C00, Imm, "xor", 1;
    OrI,     0x0D00, Imm, "or", 1;
    AndI,    0x0E00, Imm, "and", 1;
    MovAI,   0x0F00, Imm, "mov", 1;

    Call,    0x2000, Addr, "call", 2;
    Jmp,     0x2800, Addr, "jmp", 2;

    SetBit,  0x3000, Bit, "set", 1;
    ClrBit,  0x3400, Bit, "clr", 1;
    SnzBit,  0x3800, Bit, "snz", 1;
    SzBit,   0x3C00, Bit, "sz", 1;
}

impl OperandKind {
    /// Mask of the bits that are fixed by the opcode for this operand class.
    pub fn mask(self) -> u16 {
        match self {
            OperandKind::None => 0xFFFF,
            OperandKind::Mem => 0xBF80,
            OperandKind::Imm => 0xFF00,
            OperandKind::Addr => 0x3800,
            OperandKind::Bit => 0xBC00,
        }
    }
}

impl Op {
    pub fn info(self) -> &'static OpInfo {
        OPCODES.iter().find(|i| i.op == self).expect("opcode in table")
    }

    pub fn kind(self) -> OperandKind {
        self.info().kind
    }

    pub fn mnemonic(self) -> &'static str {
        self.info().mnemonic
    }

    /// True for the "skip" instructions, which need one extra cycle when
    /// the skip is taken.
    pub fn is_skip(self) -> bool {
        matches!(
            self,
            Op::Sza | Op::Sz | Op::Siza | Op::Siz | Op::Sdza | Op::Sdz | Op::SnzBit | Op::SzBit
        )
    }

    /// True when the instruction unconditionally changes program flow.
    pub fn is_branch(self) -> bool {
        matches!(self, Op::Jmp | Op::Call | Op::Ret | Op::Reti | Op::RetAI)
    }
}

/// Error produced when an instruction cannot be encoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncodeError {
    OperandMismatch { op: Op, operand: Operand },
    AddressOutOfRange(u16),
    BitOutOfRange(u8),
}

impl fmt::Display for EncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EncodeError::OperandMismatch { op, operand } => {
                write!(f, "operand {:?} does not match instruction {:?}", operand, op)
            }
            EncodeError::AddressOutOfRange(a) => {
                write!(f, "program address {:#05x} exceeds 4K words", a)
            }
            EncodeError::BitOutOfRange(b) => write!(f, "bit number {} out of range 0..7", b),
        }
    }
}

impl std::error::Error for EncodeError {}

impl Instruction {
    pub const fn new(op: Op, operand: Operand) -> Self {
        Instruction { op, operand }
    }

    pub const fn simple(op: Op) -> Self {
        Instruction { op, operand: Operand::None }
    }

    pub const fn mem(op: Op, m: u8) -> Self {
        Instruction { op, operand: Operand::Mem(m) }
    }

    pub const fn imm(op: Op, x: u8) -> Self {
        Instruction { op, operand: Operand::Imm(x) }
    }

    pub const fn addr(op: Op, a: u16) -> Self {
        Instruction { op, operand: Operand::Addr(a) }
    }

    pub const fn bit(op: Op, m: u8, i: u8) -> Self {
        Instruction { op, operand: Operand::Bit(m, i) }
    }

    /// Encode into a 16-bit program-memory word.
    pub fn encode(&self) -> Result<u16, EncodeError> {
        let info = self.op.info();
        let mismatch = || EncodeError::OperandMismatch { op: self.op, operand: self.operand };
        match (info.kind, self.operand) {
            (OperandKind::None, Operand::None) => Ok(info.base),
            (OperandKind::Mem, Operand::Mem(m)) => Ok(info.base | encode_mem(m)),
            (OperandKind::Imm, Operand::Imm(x)) => Ok(info.base | x as u16),
            (OperandKind::Addr, Operand::Addr(a)) => {
                if a as usize >= PROGRAM_WORDS {
                    return Err(EncodeError::AddressOutOfRange(a));
                }
                Ok(info.base | (a & 0x07FF) | ((a & 0x0800) << 3))
            }
            (OperandKind::Bit, Operand::Bit(m, i)) => {
                if i > 7 {
                    return Err(EncodeError::BitOutOfRange(i));
                }
                Ok(info.base | encode_mem(m) | ((i as u16) << 7))
            }
            _ => Err(mismatch()),
        }
    }

    /// Decode a program-memory word.  Returns `None` for undefined encodings.
    pub fn decode(word: u16) -> Option<Instruction> {
        // Order matters: the no-operand class must be tried first, and the
        // classes are otherwise disjoint by construction.
        for kind in [
            OperandKind::None,
            OperandKind::Mem,
            OperandKind::Imm,
            OperandKind::Addr,
            OperandKind::Bit,
        ] {
            let fixed = word & kind.mask();
            if let Some(info) = OPCODES.iter().find(|i| i.kind == kind && i.base == fixed) {
                let operand = match kind {
                    OperandKind::None => Operand::None,
                    OperandKind::Mem => Operand::Mem(decode_mem(word)),
                    OperandKind::Imm => Operand::Imm((word & 0xFF) as u8),
                    OperandKind::Addr => Operand::Addr((word & 0x07FF) | ((word >> 3) & 0x0800)),
                    OperandKind::Bit => Operand::Bit(decode_mem(word), ((word >> 7) & 7) as u8),
                };
                return Some(Instruction { op: info.op, operand });
            }
        }
        None
    }

    /// Base cycle count of the instruction.
    pub fn cycles(&self) -> u8 {
        self.op.info().cycles
    }
}

fn encode_mem(m: u8) -> u16 {
    ((m & 0x7F) as u16) | (((m & 0x80) as u16) << 7)
}

fn decode_mem(word: u16) -> u8 {
    ((word & 0x7F) as u8) | (((word >> 14) & 1) as u8) << 7
}

impl fmt::Display for Instruction {
    /// Formats in Holtek assembler syntax, e.g. `mov a,[20h]`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mn = self.op.mnemonic();
        match (self.op, self.operand) {
            (_, Operand::None) => write!(f, "{}", mn),
            (Op::MovMA, Operand::Mem(m)) => write!(f, "mov [{:02x}h],a", m),
            (
                Op::Sub | Op::Subm | Op::Add | Op::Addm | Op::Xor | Op::Xorm | Op::Or | Op::Orm
                | Op::And | Op::Andm | Op::MovAM | Op::Sbc | Op::Sbcm | Op::Adc | Op::Adcm,
                Operand::Mem(m),
            ) => write!(f, "{} a,[{:02x}h]", mn, m),
            (_, Operand::Mem(m)) => write!(f, "{} [{:02x}h]", mn, m),
            (_, Operand::Imm(x)) => write!(f, "{} a,{:02x}h", mn, x),
            (_, Operand::Addr(a)) => write!(f, "{} {:03x}h", mn, a),
            (_, Operand::Bit(m, i)) => write!(f, "{} [{:02x}h].{}", mn, m, i),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_encodings_from_hgasm_listings() {
        // Values taken from HT-IDE3000 `HGASM` listing files.
        let cases = [
            (Instruction::bit(Op::SzBit, 0x0A, 2), 0x3D0A), // sz z
            (Instruction::mem(Op::Inc, 0x07), 0x1487),       // inc tblp
            (Instruction::mem(Op::Tabrd, 0x01), 0x1D01),     // tabrd mp0
            (Instruction::mem(Op::MovAM, 0x08), 0x0708),     // mov a,tblh
            (Instruction::mem(Op::MovMA, 0x07), 0x0087),     // mov tblp,a
            (Instruction::simple(Op::ClrWdt2), 0x0005),
            (Instruction::mem(Op::Orm, 0x20), 0x05A0),       // orm a,[32]
            (Instruction::mem(Op::Adcm, 0x04), 0x1384),
            (Instruction::mem(Op::Siza, 0x03), 0x1603),
            (Instruction::mem(Op::Sdz, 0x05), 0x1785),
            (Instruction::mem(Op::Rrc, 0x80), 0x5B80),
            (Instruction::mem(Op::Clr, 0x85), 0x5F05),
            (Instruction::imm(Op::MovAI, 0x55), 0x0F55),
            (Instruction::addr(Op::Jmp, 0x042), 0x2842),
            (Instruction::addr(Op::Call, 0x359), 0x2359),
            (Instruction::addr(Op::Jmp, 0xE00), 0x6E00),
            (Instruction::bit(Op::SzBit, 0x0A, 4), 0x3E0A), // sz pdf
        ];
        for (ins, word) in cases {
            assert_eq!(ins.encode().unwrap(), word, "{}", ins);
            assert_eq!(Instruction::decode(word).unwrap(), ins, "{:04x}", word);
        }
    }

    #[test]
    fn roundtrip_every_opcode() {
        for info in OPCODES {
            let operands: Vec<Operand> = match info.kind {
                OperandKind::None => vec![Operand::None],
                OperandKind::Mem => (0..=255u8).map(Operand::Mem).collect(),
                OperandKind::Imm => (0..=255u8).map(Operand::Imm).collect(),
                OperandKind::Addr => (0..0x1000u16).map(Operand::Addr).collect(),
                OperandKind::Bit => (0..=255u8)
                    .flat_map(|m| (0..8u8).map(move |i| Operand::Bit(m, i)))
                    .collect(),
            };
            for operand in operands {
                let ins = Instruction::new(info.op, operand);
                let word = ins.encode().unwrap();
                let back = Instruction::decode(word).unwrap();
                // TABRD and TABRDC alias each other.
                let expect_op = if info.op == Op::Tabrdc { Op::Tabrd } else { info.op };
                // CLR WDT1 aliases CLR WDT.
                let expect_op = if expect_op == Op::ClrWdt1 { Op::ClrWdt } else { expect_op };
                assert_eq!(back.op, expect_op);
                assert_eq!(back.operand, operand);
            }
        }
    }

    #[test]
    fn undefined_words_do_not_decode() {
        assert!(Instruction::decode(0x0006).is_none());
        assert!(Instruction::decode(0x1C00).is_none());
        assert!(Instruction::decode(0x8000).is_none());
        assert!(Instruction::decode(0x4F00).is_none());
    }
}
