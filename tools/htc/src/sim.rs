//! Instruction-level simulator of the HT66F0185 core.
//!
//! The CPU core (ALU, flags, skip logic, hardware stack, indirect addressing
//! through MP0/IAR0 and MP1/IAR1 with bank switching, PCL writes, table
//! reads) is modelled faithfully.  Peripherals are not simulated: their
//! registers behave like plain RAM, which is enough to test generated code.
//! Interrupts can be injected with [`Cpu::interrupt`].

use std::fmt;

use crate::device::{self, sfr, PROGRAM_WORDS, STACK_LEVELS};
use crate::hex::Image;
use crate::isa::{Instruction, Op, Operand};

/// Why [`Cpu::run`] stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// A `HALT` instruction was executed.
    Halt,
    /// The cycle budget was exhausted.
    CycleLimit,
    /// An undefined instruction word was fetched.
    IllegalInstruction(u16, u16),
    /// The hardware stack overflowed (CALL with 8 levels in use).
    StackOverflow,
    /// A `RET`/`RETI` was executed with an empty stack.
    StackUnderflow,
    /// A breakpoint address was reached.
    Breakpoint(u16),
}

impl fmt::Display for Stop {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Stop::Halt => write!(f, "halt"),
            Stop::CycleLimit => write!(f, "cycle limit reached"),
            Stop::IllegalInstruction(a, w) => {
                write!(f, "illegal instruction {:04x} at {:03x}", w, a)
            }
            Stop::StackOverflow => write!(f, "hardware stack overflow"),
            Stop::StackUnderflow => write!(f, "hardware stack underflow"),
            Stop::Breakpoint(a) => write!(f, "breakpoint at {:03x}", a),
        }
    }
}

pub struct Cpu {
    pub rom: Vec<u16>,
    /// Bank 0 data memory (SFRs 00h..7Fh and general purpose 80h..FFh).
    pub ram0: [u8; 256],
    /// Bank 1 general purpose data memory (80h..FFh; lower half unused).
    pub ram1: [u8; 256],
    pub pc: u16,
    pub stack: Vec<u16>,
    pub cycles: u64,
    pub halted: bool,
    /// Optional trace sink: every executed instruction is pushed here.
    pub trace: Option<Vec<String>>,
    pub breakpoints: Vec<u16>,
    /// Bank-1-only EEC register (40h in bank 1).
    eec: u8,
    /// `CLR WDT1`/`CLR WDT2` alternation state.
    wdt_pre: u8,
}

const C: u8 = 1 << 0;
const AC: u8 = 1 << 1;
const Z: u8 = 1 << 2;
const OV: u8 = 1 << 3;
const PDF: u8 = 1 << 4;
const TO: u8 = 1 << 5;

impl Cpu {
    pub fn new(image: &Image) -> Self {
        let mut rom = image.words().to_vec();
        rom.resize(PROGRAM_WORDS, 0);
        Cpu {
            rom,
            ram0: [0; 256],
            ram1: [0; 256],
            pc: device::RESET_VECTOR,
            stack: Vec::new(),
            cycles: 0,
            halted: false,
            trace: None,
            breakpoints: Vec::new(),
            eec: 0,
            wdt_pre: 0,
        }
    }

    /// Reset the core (RAM contents are retained, as on the real device).
    pub fn reset(&mut self) {
        self.pc = device::RESET_VECTOR;
        self.stack.clear();
        self.halted = false;
        self.ram0[sfr::BP as usize] = 0;
        self.ram0[sfr::INTC0 as usize] = 0;
    }

    pub fn acc(&self) -> u8 {
        self.ram0[sfr::ACC as usize]
    }

    pub fn status(&self) -> u8 {
        self.ram0[sfr::STATUS as usize]
    }

    pub fn carry(&self) -> bool {
        self.status() & C != 0
    }

    pub fn zero(&self) -> bool {
        self.status() & Z != 0
    }

    /// Read bank-0 memory directly (no side effects; IAR reads return 0).
    pub fn peek(&self, addr: u8) -> u8 {
        self.ram0[addr as usize]
    }

    /// Read a little-endian 16-bit value from bank 0.
    pub fn peek16(&self, addr: u8) -> u16 {
        self.ram0[addr as usize] as u16 | ((self.ram0[addr.wrapping_add(1) as usize] as u16) << 8)
    }

    pub fn poke(&mut self, addr: u8, value: u8) {
        self.ram0[addr as usize] = value;
    }

    fn bank(&self) -> u8 {
        self.ram0[sfr::BP as usize] & 1
    }

    /// Resolve a data-memory address to a concrete cell, following IAR0/IAR1.
    /// Returns `(bank, address)`; `None` for an unimplemented read (returns 0).
    fn resolve(&self, addr: u8) -> Option<(u8, u8)> {
        match addr {
            sfr::IAR0 => {
                let mp = self.ram0[sfr::MP0 as usize];
                if mp == sfr::IAR0 || mp == sfr::IAR1 {
                    None
                } else {
                    Some((0, mp))
                }
            }
            sfr::IAR1 => {
                let mp = self.ram0[sfr::MP1 as usize];
                if mp == sfr::IAR0 || mp == sfr::IAR1 {
                    None
                } else {
                    Some((self.bank(), mp))
                }
            }
            _ => Some((0, addr)),
        }
    }

    fn read(&self, addr: u8) -> u8 {
        match self.resolve(addr) {
            None => 0,
            Some((bank, a)) => {
                if a == sfr::EEC {
                    return if bank == 1 { self.eec } else { 0 };
                }
                if a < 0x80 || bank == 0 {
                    self.ram0[a as usize]
                } else {
                    self.ram1[a as usize]
                }
            }
        }
    }

    /// Write with side effects; returns true when PCL was written (extra cycle).
    fn write(&mut self, addr: u8, value: u8) -> bool {
        let (bank, a) = match self.resolve(addr) {
            None => return false,
            Some(x) => x,
        };
        match a {
            sfr::PCL => {
                self.pc = (self.pc & 0x0F00) | value as u16;
                self.ram0[a as usize] = value;
                return true;
            }
            sfr::TBLH => return false, // read-only
            sfr::STATUS => {
                // TO and PDF cannot be written by software.
                let keep = self.ram0[a as usize] & (TO | PDF);
                self.ram0[a as usize] = (value & !(TO | PDF)) | keep;
                return false;
            }
            sfr::EEC => {
                if bank == 1 {
                    self.eec = value;
                }
                return false;
            }
            _ => {}
        }
        if a < 0x80 || bank == 0 {
            self.ram0[a as usize] = value;
        } else {
            self.ram1[a as usize] = value;
        }
        false
    }

    fn set_flags(&mut self, set: u8, clear: u8) {
        let s = self.ram0[sfr::STATUS as usize];
        self.ram0[sfr::STATUS as usize] = (s & !clear) | set;
    }

    fn set_z(&mut self, result: u8) {
        if result == 0 {
            self.set_flags(Z, 0);
        } else {
            self.set_flags(0, Z);
        }
    }

    fn add(&mut self, a: u8, b: u8, carry_in: u8) -> u8 {
        let sum = a as u16 + b as u16 + carry_in as u16;
        let r = sum as u8;
        let mut set = 0;
        let mut clear = 0;
        if sum > 0xFF {
            set |= C
        } else {
            clear |= C
        }
        if (a & 0x0F) + (b & 0x0F) + carry_in > 0x0F {
            set |= AC
        } else {
            clear |= AC
        }
        if ((a ^ r) & (b ^ r) & 0x80) != 0 {
            set |= OV
        } else {
            clear |= OV
        }
        if r == 0 {
            set |= Z
        } else {
            clear |= Z
        }
        self.set_flags(set, clear);
        r
    }

    /// `a - b - borrow_in`; C is set when no borrow occurs.
    fn sub(&mut self, a: u8, b: u8, borrow_in: u8) -> u8 {
        let diff = a as i16 - b as i16 - borrow_in as i16;
        let r = diff as u8;
        let mut set = 0;
        let mut clear = 0;
        if diff >= 0 {
            set |= C
        } else {
            clear |= C
        }
        if (a & 0x0F) as i16 - (b & 0x0F) as i16 - borrow_in as i16 >= 0 {
            set |= AC
        } else {
            clear |= AC
        }
        if ((a ^ b) & (a ^ r) & 0x80) != 0 {
            set |= OV
        } else {
            clear |= OV
        }
        if r == 0 {
            set |= Z
        } else {
            clear |= Z
        }
        self.set_flags(set, clear);
        r
    }

    fn push(&mut self, value: u16) -> Result<(), Stop> {
        if self.stack.len() >= STACK_LEVELS {
            return Err(Stop::StackOverflow);
        }
        self.stack.push(value);
        Ok(())
    }

    fn pop(&mut self) -> Result<u16, Stop> {
        self.stack.pop().ok_or(Stop::StackUnderflow)
    }

    /// Inject an interrupt: if EMI is set, push PC, clear EMI, jump to vector.
    /// Returns true if the interrupt was taken.
    pub fn interrupt(&mut self, vector: u16) -> bool {
        if self.ram0[sfr::INTC0 as usize] & 1 == 0 || self.stack.len() >= STACK_LEVELS {
            return false;
        }
        self.halted = false;
        self.stack.push(self.pc);
        self.ram0[sfr::INTC0 as usize] &= !1;
        self.pc = vector;
        self.cycles += 2;
        true
    }

    /// Execute one instruction.
    pub fn step(&mut self) -> Result<(), Stop> {
        if self.halted {
            return Err(Stop::Halt);
        }
        let addr = self.pc;
        let word = self.rom[addr as usize];
        let ins = Instruction::decode(word).ok_or(Stop::IllegalInstruction(addr, word))?;
        if let Some(t) = &mut self.trace {
            t.push(format!("{:03x}: {:04x}  {}", addr, word, ins));
        }
        self.pc = (self.pc + 1) & 0x0FFF;
        self.ram0[sfr::PCL as usize] = self.pc as u8;
        self.cycles += ins.cycles() as u64;
        let acc = self.acc();
        let mut skip = false;
        let mut pcl_written = false;

        let m = match ins.operand {
            Operand::Mem(m) | Operand::Bit(m, _) => m,
            _ => 0,
        };
        let bitno = if let Operand::Bit(_, b) = ins.operand {
            b
        } else {
            0
        };
        let imm = if let Operand::Imm(x) = ins.operand {
            x
        } else {
            0
        };
        let target = if let Operand::Addr(a) = ins.operand {
            a
        } else {
            0
        };

        match ins.op {
            Op::Nop => {}
            Op::ClrWdt => {
                self.set_flags(0, TO | PDF);
                self.wdt_pre = 0;
            }
            Op::ClrWdt1 => {
                if self.wdt_pre == 2 {
                    self.set_flags(0, TO | PDF);
                    self.wdt_pre = 0;
                } else {
                    self.wdt_pre = 1;
                }
            }
            Op::ClrWdt2 => {
                if self.wdt_pre == 1 {
                    self.set_flags(0, TO | PDF);
                    self.wdt_pre = 0;
                } else {
                    self.wdt_pre = 2;
                }
            }
            Op::Halt => {
                self.set_flags(PDF, TO);
                self.halted = true;
                return Err(Stop::Halt);
            }
            Op::Ret => {
                self.pc = self.pop()?;
            }
            Op::RetAI => {
                self.pc = self.pop()?;
                self.ram0[sfr::ACC as usize] = imm;
            }
            Op::Reti => {
                self.pc = self.pop()?;
                self.ram0[sfr::INTC0 as usize] |= 1;
            }
            Op::MovMA => pcl_written = self.write(m, acc),
            Op::MovAM => self.ram0[sfr::ACC as usize] = self.read(m),
            Op::MovAI => self.ram0[sfr::ACC as usize] = imm,
            Op::Cpla => {
                let r = !self.read(m);
                self.set_z(r);
                self.ram0[sfr::ACC as usize] = r;
            }
            Op::Cpl => {
                let r = !self.read(m);
                self.set_z(r);
                pcl_written = self.write(m, r);
            }
            Op::Add | Op::AddI => {
                let b = if ins.op == Op::Add { self.read(m) } else { imm };
                let r = self.add(acc, b, 0);
                self.ram0[sfr::ACC as usize] = r;
            }
            Op::Addm => {
                let b = self.read(m);
                let r = self.add(acc, b, 0);
                pcl_written = self.write(m, r);
            }
            Op::Adc => {
                let b = self.read(m);
                let c = self.status() & C;
                let r = self.add(acc, b, c);
                self.ram0[sfr::ACC as usize] = r;
            }
            Op::Adcm => {
                let b = self.read(m);
                let c = self.status() & C;
                let r = self.add(acc, b, c);
                pcl_written = self.write(m, r);
            }
            Op::Sub | Op::SubI => {
                let b = if ins.op == Op::Sub { self.read(m) } else { imm };
                let r = self.sub(acc, b, 0);
                self.ram0[sfr::ACC as usize] = r;
            }
            Op::Subm => {
                let b = self.read(m);
                let r = self.sub(acc, b, 0);
                pcl_written = self.write(m, r);
            }
            Op::Sbc => {
                let b = self.read(m);
                let borrow = 1 - (self.status() & C);
                let r = self.sub(acc, b, borrow);
                self.ram0[sfr::ACC as usize] = r;
            }
            Op::Sbcm => {
                let b = self.read(m);
                let borrow = 1 - (self.status() & C);
                let r = self.sub(acc, b, borrow);
                pcl_written = self.write(m, r);
            }
            Op::And | Op::AndI | Op::Or | Op::OrI | Op::Xor | Op::XorI => {
                let b = match ins.op {
                    Op::And | Op::Or | Op::Xor => self.read(m),
                    _ => imm,
                };
                let r = match ins.op {
                    Op::And | Op::AndI => acc & b,
                    Op::Or | Op::OrI => acc | b,
                    _ => acc ^ b,
                };
                self.set_z(r);
                self.ram0[sfr::ACC as usize] = r;
            }
            Op::Andm | Op::Orm | Op::Xorm => {
                let b = self.read(m);
                let r = match ins.op {
                    Op::Andm => acc & b,
                    Op::Orm => acc | b,
                    _ => acc ^ b,
                };
                self.set_z(r);
                pcl_written = self.write(m, r);
            }
            Op::Sza => {
                let v = self.read(m);
                self.ram0[sfr::ACC as usize] = v;
                skip = v == 0;
            }
            Op::Sz => skip = self.read(m) == 0,
            Op::Swapa => {
                let v = self.read(m);
                self.ram0[sfr::ACC as usize] = v.rotate_left(4);
            }
            Op::Swap => {
                let v = self.read(m).rotate_left(4);
                pcl_written = self.write(m, v);
            }
            Op::Inca => {
                let r = self.read(m).wrapping_add(1);
                self.set_z(r);
                self.ram0[sfr::ACC as usize] = r;
            }
            Op::Inc => {
                let r = self.read(m).wrapping_add(1);
                self.set_z(r);
                pcl_written = self.write(m, r);
            }
            Op::Deca => {
                let r = self.read(m).wrapping_sub(1);
                self.set_z(r);
                self.ram0[sfr::ACC as usize] = r;
            }
            Op::Dec => {
                let r = self.read(m).wrapping_sub(1);
                self.set_z(r);
                pcl_written = self.write(m, r);
            }
            Op::Siza => {
                let r = self.read(m).wrapping_add(1);
                self.ram0[sfr::ACC as usize] = r;
                skip = r == 0;
            }
            Op::Siz => {
                let r = self.read(m).wrapping_add(1);
                pcl_written = self.write(m, r);
                skip = r == 0;
            }
            Op::Sdza => {
                let r = self.read(m).wrapping_sub(1);
                self.ram0[sfr::ACC as usize] = r;
                skip = r == 0;
            }
            Op::Sdz => {
                let r = self.read(m).wrapping_sub(1);
                pcl_written = self.write(m, r);
                skip = r == 0;
            }
            Op::Rla | Op::Rl => {
                let r = self.read(m).rotate_left(1);
                if ins.op == Op::Rla {
                    self.ram0[sfr::ACC as usize] = r;
                } else {
                    pcl_written = self.write(m, r);
                }
            }
            Op::Rra | Op::Rr => {
                let r = self.read(m).rotate_right(1);
                if ins.op == Op::Rra {
                    self.ram0[sfr::ACC as usize] = r;
                } else {
                    pcl_written = self.write(m, r);
                }
            }
            Op::Rlca | Op::Rlc => {
                let v = self.read(m);
                let cin = self.status() & C;
                let r = (v << 1) | cin;
                self.set_flags(
                    if v & 0x80 != 0 { C } else { 0 },
                    if v & 0x80 != 0 { 0 } else { C },
                );
                if ins.op == Op::Rlca {
                    self.ram0[sfr::ACC as usize] = r;
                } else {
                    pcl_written = self.write(m, r);
                }
            }
            Op::Rrca | Op::Rrc => {
                let v = self.read(m);
                let cin = self.status() & C;
                let r = (v >> 1) | (cin << 7);
                self.set_flags(
                    if v & 1 != 0 { C } else { 0 },
                    if v & 1 != 0 { 0 } else { C },
                );
                if ins.op == Op::Rrca {
                    self.ram0[sfr::ACC as usize] = r;
                } else {
                    pcl_written = self.write(m, r);
                }
            }
            Op::Tabrd | Op::Tabrdc | Op::Tabrdl => {
                let low = self.ram0[sfr::TBLP as usize] as u16;
                let page = match ins.op {
                    Op::Tabrd => (self.ram0[sfr::TBHP as usize] as u16 & 0x0F) << 8,
                    Op::Tabrdc => addr & 0x0F00,
                    _ => 0x0F00,
                };
                let w = self.rom[(page | low) as usize];
                self.ram0[sfr::TBLH as usize] = (w >> 8) as u8;
                pcl_written = self.write(m, w as u8);
            }
            Op::Daa => {
                let s = self.status();
                let mut r = acc;
                let mut carry = false;
                if (acc & 0x0F) > 9 || s & AC != 0 {
                    r = r.wrapping_add(0x06);
                }
                if (acc >> 4) > 9 || s & C != 0 || (r < acc) {
                    let (v, c) = r.overflowing_add(0x60);
                    r = v;
                    carry = c || s & C != 0 || (acc >> 4) > 9;
                }
                self.set_flags(if carry { C } else { 0 }, if carry { 0 } else { C });
                pcl_written = self.write(m, r);
            }
            Op::Clr => pcl_written = self.write(m, 0),
            Op::Set => pcl_written = self.write(m, 0xFF),
            Op::Call => {
                self.push(self.pc)?;
                self.pc = target;
            }
            Op::Jmp => self.pc = target,
            Op::SetBit => {
                let v = self.read(m) | (1 << bitno);
                pcl_written = self.write(m, v);
            }
            Op::ClrBit => {
                let v = self.read(m) & !(1 << bitno);
                pcl_written = self.write(m, v);
            }
            Op::SnzBit => skip = self.read(m) & (1 << bitno) != 0,
            Op::SzBit => skip = self.read(m) & (1 << bitno) == 0,
        }
        if skip {
            self.pc = (self.pc + 1) & 0x0FFF;
            self.cycles += 1;
        }
        if pcl_written {
            self.cycles += 1;
        }
        self.ram0[sfr::PCL as usize] = self.pc as u8;
        Ok(())
    }

    /// Run until halt, error, breakpoint or `max_cycles` elapsed.
    pub fn run(&mut self, max_cycles: u64) -> Stop {
        let limit = self.cycles.saturating_add(max_cycles);
        loop {
            if self.cycles >= limit {
                return Stop::CycleLimit;
            }
            if let Err(stop) = self.step() {
                return stop;
            }
            if self.breakpoints.contains(&self.pc) {
                return Stop::Breakpoint(self.pc);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asm::assemble_str;

    fn run(src: &str) -> Cpu {
        let a = assemble_str("t.asm", src).unwrap_or_else(|e| panic!("{}", e[0]));
        let mut cpu = Cpu::new(&a.image);
        let stop = cpu.run(100_000);
        assert_eq!(stop, Stop::Halt, "program did not halt");
        cpu
    }

    #[test]
    fn arithmetic_and_flags() {
        let cpu = run("
            mov a, 250
            add a, 10        ; 260 -> 4, C=1
            mov [80h], a
            mov a, 5
            sub a, 7         ; -2 -> 0FEh, C=0 (borrow)
            mov [81h], a
            mov a, 0
            sbc a, [80h]     ; 0 - 4 - 1 = -5 -> 0FBh
            mov [82h], a
            mov a, 9
            add a, 8         ; 17 = 11h, AC=1
            daa [83h]        ; -> 17h
            halt
        ");
        assert_eq!(cpu.peek(0x80), 4);
        assert_eq!(cpu.peek(0x81), 0xFE);
        assert_eq!(cpu.peek(0x82), 0xFB);
        assert_eq!(cpu.peek(0x83), 0x17);
    }

    #[test]
    fn skips_loops_and_indirect() {
        let cpu = run("
            mov a, 4
            mov [90h], a
            mov a, 80h
            mov mp0, a
          loop:
            mov a, mp0
            mov iar0, a
            inc mp0
            sdz [90h]
            jmp loop
            set [91h].5
            snz [91h].5      ; bit set: skip
            jmp bad
            sz [91h].5       ; bit set: no skip
            jmp ok
            jmp bad
          ok:
            call sub
            mov [92h], a
            halt
          bad:
            mov a, 0eeh
            mov [92h], a
            halt
          sub:
            ret a, 42h
        ");
        assert_eq!(&cpu.ram0[0x80..0x84], &[0x80, 0x81, 0x82, 0x83]);
        assert_eq!(cpu.peek(0x91), 0x20);
        assert_eq!(cpu.peek(0x92), 0x42);
    }

    #[test]
    fn table_read_and_bank1() {
        let cpu = run("
            mov a, low table
            mov tblp, a
            mov a, high table
            mov tbhp, a
            tabrd [80h]
            mov a, tblh
            mov [81h], a
            inc tblp
            tabrdl [82h]
            set bp.0
            mov a, 0a0h
            mov mp1, a
            mov a, 77h
            mov iar1, a       ; bank 1, a0h
            clr bp.0
            mov a, iar1       ; bank 0, a0h (still 0)
            mov [83h], a
            halt
            org 0f10h
          table:
            dc 1234h, 0abcdh
        ");
        assert_eq!(cpu.peek(0x80), 0x34);
        assert_eq!(cpu.peek(0x81), 0x12);
        assert_eq!(cpu.peek(0x82), 0xCD);
        assert_eq!(cpu.peek(0x08), 0xAB);
        assert_eq!(cpu.ram1[0xA0], 0x77);
        assert_eq!(cpu.peek(0x83), 0);
    }

    #[test]
    fn pcl_jump_and_interrupt() {
        let a = assemble_str(
            "t.asm",
            "
            jmp start
            org 4
            inc [90h]
            reti
            org 30h
          start:
            set intc0.0
            mov a, low target
            mov pcl, a
            mov a, 1
            mov [91h], a
          target:
            halt
        ",
        )
        .unwrap();
        let mut cpu = Cpu::new(&a.image);
        assert_eq!(cpu.run(6), Stop::CycleLimit);
        assert!(cpu.interrupt(0x04));
        assert_eq!(cpu.pc, 4);
        assert_eq!(cpu.run(1000), Stop::Halt);
        assert_eq!(cpu.peek(0x90), 1);
        assert_eq!(cpu.peek(0x91), 0);
        assert_eq!(cpu.peek(sfr::INTC0), 1);
    }
}
