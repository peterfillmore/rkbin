//! Disassembler producing re-assemblable Holtek syntax.

use crate::device;
use crate::hex::Image;
use crate::isa::{Instruction, Op, Operand};

/// Format one word at `addr` as a source line (without address prefix).
pub fn format_word(word: u16) -> String {
    match Instruction::decode(word) {
        Some(ins) => format_instruction(&ins),
        None => format!("dc 0{:04x}h", word),
    }
}

/// Format an instruction using symbolic SFR names where possible.
pub fn format_instruction(ins: &Instruction) -> String {
    format_instruction_with(ins, &|a| format!("L{:03x}", a))
}

/// Format an instruction, rendering program addresses with `target`.
pub fn format_instruction_with(ins: &Instruction, target: &dyn Fn(u16) -> String) -> String {
    let mem = |m: u8| -> String {
        match device::sfr_name(m) {
            Some(n) => n.to_ascii_lowercase(),
            None => format!("[0{:02x}h]", m),
        }
    };
    let mn = ins.op.mnemonic();
    match (ins.op, ins.operand) {
        (_, Operand::None) => mn.to_string(),
        (Op::MovMA, Operand::Mem(m)) => format!("mov {},a", mem(m)),
        (
            Op::Sub
            | Op::Subm
            | Op::Add
            | Op::Addm
            | Op::Xor
            | Op::Xorm
            | Op::Or
            | Op::Orm
            | Op::And
            | Op::Andm
            | Op::MovAM
            | Op::Sbc
            | Op::Sbcm
            | Op::Adc
            | Op::Adcm,
            Operand::Mem(m),
        ) => format!("{} a,{}", mn, mem(m)),
        (_, Operand::Mem(m)) => format!("{} {}", mn, mem(m)),
        (_, Operand::Imm(x)) => format!("{} a,0{:02x}h", mn, x),
        (_, Operand::Addr(a)) => format!("{} {}", mn, target(a)),
        (_, Operand::Bit(m, i)) => format!("{} {}.{}", mn, mem(m), i),
    }
}

/// Disassemble a whole image into an assembler-compatible listing.
#[allow(clippy::needless_range_loop)]
pub fn disassemble(image: &Image, with_addresses: bool) -> String {
    let end = image.end();
    let mut targets = vec![false; end.max(1)];
    for a in 0..end {
        if let Some(Instruction {
            operand: Operand::Addr(t),
            ..
        }) = Instruction::decode(image.get(a as u16))
        {
            if (t as usize) < end && image.is_used(t) {
                targets[t as usize] = true;
            }
        }
    }
    let mut out = String::new();
    let mut last_used = true;
    for a in 0..end {
        let used = image.is_used(a as u16);
        if !used {
            last_used = false;
            continue;
        }
        if !last_used {
            out.push_str(&format!("\n        org 0{:03x}h\n", a));
            last_used = true;
        }
        if a == 0 {
            out.push_str("        org 000h\n");
        }
        if let Some(name) = device::vector_name(a as u16) {
            out.push_str(&format!("; --- {} interrupt vector\n", name));
        }
        if targets[a] {
            out.push_str(&format!("L{:03x}:\n", a));
        }
        let word = image.get(a as u16);
        let text = match Instruction::decode(word) {
            Some(ins) => format_instruction_with(&ins, &|t| {
                if (t as usize) < end && image.is_used(t) {
                    format!("L{:03x}", t)
                } else {
                    format!("0{:03x}h", t)
                }
            }),
            None => format!("dc 0{:04x}h", word),
        };
        if with_addresses {
            out.push_str(&format!("{:03X}  {:04X}    {}\n", a, word, text));
        } else {
            out.push_str(&format!("        {}\n", text));
        }
    }
    out
}
