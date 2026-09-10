//! `htc` — an open-source toolchain for the Holtek HT66F0185 8-bit MCU.
//!
//! * [`isa`]    — instruction encoding/decoding
//! * [`device`] — memory map, special function registers, interrupt vectors
//! * [`asm`]    — Holtek-syntax assembler
//! * [`hex`]    — Intel HEX / binary program images
//! * [`disasm`] — disassembler
//! * [`sim`]    — instruction-level simulator of the core
//! * [`lang`]   — compiler for the C-like `htc` language

pub mod asm;
pub mod device;
pub mod disasm;
pub mod hex;
pub mod isa;
pub mod lang;
pub mod sim;
