//! Assemble → disassemble → re-assemble must reproduce the same image, and
//! every instruction's `Display` form must be accepted by the assembler.
use htc::asm::assemble_str;
use htc::disasm;
use htc::isa::{Instruction, Operand, OperandKind, OPCODES};

#[test]
fn disassembly_reassembles_identically() {
    let src = "
        org 000h
        jmp start
        org 004h
        inc [90h]
        reti
        org 030h
      start:
        mov a,0abh
        mov wdtc,a
        mov a,low table
        mov tblp,a
        mov a,high table
        mov tbhp,a
        tabrd [80h]
        tabrdl [0f0h]
        set [0f0h].7
        snz status.2
        call sub
        sdz [0ffh]
        jmp start
        ret a,42h
      sub:
        clr wdt
        clr wdt2
        daa [81h]
        halt
        org 0f00h
      table:
        dc 1234h, 0abcdh, 'A'
    ";
    let a = assemble_str("a.asm", src).unwrap_or_else(|e| panic!("{}", e[0]));
    let text = disasm::disassemble(&a.image, false);
    let b = assemble_str("b.asm", &text).unwrap_or_else(|e| panic!("{}\n{}", e[0], text));
    for addr in 0..0x1000u16 {
        assert_eq!(
            a.image.is_used(addr),
            b.image.is_used(addr),
            "used {:03x}\n{}",
            addr,
            text
        );
        assert_eq!(
            a.image.get(addr),
            b.image.get(addr),
            "word {:03x}\n{}",
            addr,
            text
        );
    }
}

#[test]
fn display_form_is_assemblable() {
    let mut src = String::new();
    let mut expected = Vec::new();
    for info in OPCODES {
        let operand = match info.kind {
            OperandKind::None => Operand::None,
            OperandKind::Mem => Operand::Mem(0x9A),
            OperandKind::Imm => Operand::Imm(0xC3),
            OperandKind::Addr => Operand::Addr(0xABC),
            OperandKind::Bit => Operand::Bit(0xF5, 6),
        };
        let ins = Instruction::new(info.op, operand);
        src.push_str(&format!("        {}\n", ins));
        expected.push(ins.encode().unwrap());
    }
    let a = assemble_str("d.asm", &src).unwrap_or_else(|e| panic!("{}\n{}", e[0], src));
    for (i, w) in expected.iter().enumerate() {
        assert_eq!(
            a.image.get(i as u16),
            *w,
            "line {}: {}",
            i + 1,
            src.lines().nth(i).unwrap()
        );
    }
}
