//! Device description of the Holtek HT66F0185.
//!
//! Sources: HT66F0175/HT66F0185 datasheet rev. 1.50 (Special Purpose Data
//! Memory Structure, Interrupt Scheme, STATUS/INTC register descriptions).

/// Program memory size in 16-bit words (4K × 16).
pub const PROGRAM_WORDS: usize = 0x1000;
/// Highest program address.
pub const PROGRAM_LAST: u16 = 0x0FFF;
/// Number of hardware stack levels.
pub const STACK_LEVELS: usize = 8;
/// First address of general purpose data memory (in every bank).
pub const GP_RAM_START: u8 = 0x80;
/// Number of general purpose data-memory banks.
pub const RAM_BANKS: usize = 2;
/// True EEPROM size in bytes.
pub const EEPROM_BYTES: usize = 128;
/// Reset vector.
pub const RESET_VECTOR: u16 = 0x000;

/// Special function register addresses (data memory, bank 0 and 1 unless noted).
pub mod sfr {
    pub const IAR0: u8 = 0x00;
    pub const MP0: u8 = 0x01;
    pub const IAR1: u8 = 0x02;
    pub const MP1: u8 = 0x03;
    pub const BP: u8 = 0x04;
    pub const ACC: u8 = 0x05;
    pub const PCL: u8 = 0x06;
    pub const TBLP: u8 = 0x07;
    pub const TBLH: u8 = 0x08;
    pub const TBHP: u8 = 0x09;
    pub const STATUS: u8 = 0x0A;
    pub const SMOD: u8 = 0x0B;
    pub const LVDC: u8 = 0x0C;
    pub const INTEG: u8 = 0x0D;
    pub const INTC0: u8 = 0x0E;
    pub const INTC1: u8 = 0x0F;
    pub const INTC2: u8 = 0x10;
    pub const MFI0: u8 = 0x11;
    pub const MFI1: u8 = 0x12;
    pub const MFI2: u8 = 0x13;
    pub const PA: u8 = 0x14;
    pub const PAC: u8 = 0x15;
    pub const PAPU: u8 = 0x16;
    pub const PAWU: u8 = 0x17;
    pub const TMPC: u8 = 0x19;
    pub const WDTC: u8 = 0x1A;
    pub const TBC: u8 = 0x1B;
    pub const CTRL: u8 = 0x1C;
    pub const LVRC: u8 = 0x1D;
    pub const EEA: u8 = 0x1E;
    pub const EED: u8 = 0x1F;
    pub const SADOL: u8 = 0x20;
    pub const SADOH: u8 = 0x21;
    pub const SADC0: u8 = 0x22;
    pub const SADC1: u8 = 0x23;
    pub const SADC2: u8 = 0x24;
    pub const PB: u8 = 0x25;
    pub const PBC: u8 = 0x26;
    pub const PBPU: u8 = 0x27;
    pub const TM2C0: u8 = 0x28;
    pub const TM2C1: u8 = 0x29;
    pub const TM2DL: u8 = 0x2A;
    pub const TM2DH: u8 = 0x2B;
    pub const TM2AL: u8 = 0x2C;
    pub const TM2AH: u8 = 0x2D;
    pub const TM2RP: u8 = 0x2E;
    pub const TM0C0: u8 = 0x2F;
    pub const TM0C1: u8 = 0x30;
    pub const TM0DL: u8 = 0x31;
    pub const TM0DH: u8 = 0x32;
    pub const TM0AL: u8 = 0x33;
    pub const TM0AH: u8 = 0x34;
    pub const TM0RP: u8 = 0x35;
    pub const TM1C0: u8 = 0x37;
    pub const TM1C1: u8 = 0x38;
    pub const TM1DL: u8 = 0x39;
    pub const TM1DH: u8 = 0x3A;
    pub const TM1AL: u8 = 0x3B;
    pub const TM1AH: u8 = 0x3C;
    pub const TM1RPL: u8 = 0x3D;
    pub const TM1RPH: u8 = 0x3E;
    pub const CPC: u8 = 0x3F;
    /// EEPROM control register: only accessible in bank 1 (via MP1/IAR1).
    pub const EEC: u8 = 0x40;
    pub const PC: u8 = 0x41;
    pub const PCC: u8 = 0x42;
    pub const PCPU: u8 = 0x43;
    pub const ACERL: u8 = 0x44;
    pub const SIMC0: u8 = 0x45;
    pub const SIMC1: u8 = 0x46;
    pub const SIMD: u8 = 0x47;
    pub const SIMA: u8 = 0x48;
    pub const SIMC2: u8 = 0x48;
    pub const SIMTOC: u8 = 0x49;
    pub const SLCDC0: u8 = 0x4A;
    pub const SLCDC1: u8 = 0x4B;
    pub const SLCDC2: u8 = 0x4C;
    pub const SLCDC3: u8 = 0x4D;
    pub const SLCDC4: u8 = 0x4E;
    pub const SLEDC0: u8 = 0x4F;
    pub const SLEDC1: u8 = 0x50;
    pub const IFS: u8 = 0x51;
    pub const PD: u8 = 0x52;
    pub const PDC: u8 = 0x53;
    pub const PDPU: u8 = 0x54;
    pub const USR: u8 = 0x55;
    pub const UCR1: u8 = 0x56;
    pub const UCR2: u8 = 0x57;
    pub const BRG: u8 = 0x58;
    pub const TXR_RXR: u8 = 0x59;
}

/// Name/address table of all special function registers, in address order.
pub const SFR_TABLE: &[(&str, u8)] = &[
    ("IAR0", sfr::IAR0),
    ("MP0", sfr::MP0),
    ("IAR1", sfr::IAR1),
    ("MP1", sfr::MP1),
    ("BP", sfr::BP),
    ("ACC", sfr::ACC),
    ("PCL", sfr::PCL),
    ("TBLP", sfr::TBLP),
    ("TBLH", sfr::TBLH),
    ("TBHP", sfr::TBHP),
    ("STATUS", sfr::STATUS),
    ("SMOD", sfr::SMOD),
    ("LVDC", sfr::LVDC),
    ("INTEG", sfr::INTEG),
    ("INTC0", sfr::INTC0),
    ("INTC1", sfr::INTC1),
    ("INTC2", sfr::INTC2),
    ("MFI0", sfr::MFI0),
    ("MFI1", sfr::MFI1),
    ("MFI2", sfr::MFI2),
    ("PA", sfr::PA),
    ("PAC", sfr::PAC),
    ("PAPU", sfr::PAPU),
    ("PAWU", sfr::PAWU),
    ("TMPC", sfr::TMPC),
    ("WDTC", sfr::WDTC),
    ("TBC", sfr::TBC),
    ("CTRL", sfr::CTRL),
    ("LVRC", sfr::LVRC),
    ("EEA", sfr::EEA),
    ("EED", sfr::EED),
    ("SADOL", sfr::SADOL),
    ("SADOH", sfr::SADOH),
    ("SADC0", sfr::SADC0),
    ("SADC1", sfr::SADC1),
    ("SADC2", sfr::SADC2),
    ("PB", sfr::PB),
    ("PBC", sfr::PBC),
    ("PBPU", sfr::PBPU),
    ("TM2C0", sfr::TM2C0),
    ("TM2C1", sfr::TM2C1),
    ("TM2DL", sfr::TM2DL),
    ("TM2DH", sfr::TM2DH),
    ("TM2AL", sfr::TM2AL),
    ("TM2AH", sfr::TM2AH),
    ("TM2RP", sfr::TM2RP),
    ("TM0C0", sfr::TM0C0),
    ("TM0C1", sfr::TM0C1),
    ("TM0DL", sfr::TM0DL),
    ("TM0DH", sfr::TM0DH),
    ("TM0AL", sfr::TM0AL),
    ("TM0AH", sfr::TM0AH),
    ("TM0RP", sfr::TM0RP),
    ("TM1C0", sfr::TM1C0),
    ("TM1C1", sfr::TM1C1),
    ("TM1DL", sfr::TM1DL),
    ("TM1DH", sfr::TM1DH),
    ("TM1AL", sfr::TM1AL),
    ("TM1AH", sfr::TM1AH),
    ("TM1RPL", sfr::TM1RPL),
    ("TM1RPH", sfr::TM1RPH),
    ("CPC", sfr::CPC),
    ("EEC", sfr::EEC),
    ("PC", sfr::PC),
    ("PCC", sfr::PCC),
    ("PCPU", sfr::PCPU),
    ("ACERL", sfr::ACERL),
    ("SIMC0", sfr::SIMC0),
    ("SIMC1", sfr::SIMC1),
    ("SIMD", sfr::SIMD),
    ("SIMA", sfr::SIMA),
    ("SIMC2", sfr::SIMC2),
    ("SIMTOC", sfr::SIMTOC),
    ("SLCDC0", sfr::SLCDC0),
    ("SLCDC1", sfr::SLCDC1),
    ("SLCDC2", sfr::SLCDC2),
    ("SLCDC3", sfr::SLCDC3),
    ("SLCDC4", sfr::SLCDC4),
    ("SLEDC0", sfr::SLEDC0),
    ("SLEDC1", sfr::SLEDC1),
    ("IFS", sfr::IFS),
    ("PD", sfr::PD),
    ("PDC", sfr::PDC),
    ("PDPU", sfr::PDPU),
    ("USR", sfr::USR),
    ("UCR1", sfr::UCR1),
    ("UCR2", sfr::UCR2),
    ("BRG", sfr::BRG),
    ("TXR_RXR", sfr::TXR_RXR),
];

/// Named bits of special function registers: `(name, register, bit)`.
pub const BIT_TABLE: &[(&str, u8, u8)] = &[
    // STATUS
    ("C", sfr::STATUS, 0),
    ("AC", sfr::STATUS, 1),
    ("Z", sfr::STATUS, 2),
    ("OV", sfr::STATUS, 3),
    ("PDF", sfr::STATUS, 4),
    ("TO", sfr::STATUS, 5),
    // BP
    ("DMBP0", sfr::BP, 0),
    // INTC0
    ("EMI", sfr::INTC0, 0),
    ("INT0E", sfr::INTC0, 1),
    ("CPE", sfr::INTC0, 2),
    ("MF0E", sfr::INTC0, 3),
    ("INT0F", sfr::INTC0, 4),
    ("CPF", sfr::INTC0, 5),
    ("MF0F", sfr::INTC0, 6),
    // INTC1
    ("MF1E", sfr::INTC1, 0),
    ("MF2E", sfr::INTC1, 1),
    ("ADE", sfr::INTC1, 2),
    ("TB0E", sfr::INTC1, 3),
    ("MF1F", sfr::INTC1, 4),
    ("MF2F", sfr::INTC1, 5),
    ("ADF", sfr::INTC1, 6),
    ("TB0F", sfr::INTC1, 7),
    // INTC2
    ("TB1E", sfr::INTC2, 0),
    ("INT1E", sfr::INTC2, 1),
    ("SIME", sfr::INTC2, 2),
    ("URE", sfr::INTC2, 3),
    ("TB1F", sfr::INTC2, 4),
    ("INT1F", sfr::INTC2, 5),
    ("SIMF", sfr::INTC2, 6),
    ("URF", sfr::INTC2, 7),
];

/// Interrupt vectors of the HT66F0185 in priority order (highest first).
pub const VECTORS: &[(&str, u16)] = &[
    ("INT0", 0x04),
    ("CMP", 0x08),
    ("MF0", 0x0C),
    ("MF1", 0x10),
    ("MF2", 0x14),
    ("ADC", 0x18),
    ("TB0", 0x1C),
    ("TB1", 0x20),
    ("INT1", 0x24),
    ("SIM", 0x28),
    ("UART", 0x2C),
];

/// First program address not used by any interrupt vector.
pub const CODE_START: u16 = 0x30;

/// Look up a special function register by (case-insensitive) name.
pub fn sfr_by_name(name: &str) -> Option<u8> {
    SFR_TABLE
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|&(_, a)| a)
}

/// Look up a special function register name by address.
pub fn sfr_name(addr: u8) -> Option<&'static str> {
    SFR_TABLE.iter().find(|&&(_, a)| a == addr).map(|&(n, _)| n)
}

/// Look up a named register bit.
pub fn bit_by_name(name: &str) -> Option<(u8, u8)> {
    BIT_TABLE
        .iter()
        .find(|(n, _, _)| n.eq_ignore_ascii_case(name))
        .map(|&(_, r, b)| (r, b))
}

/// Look up an interrupt vector by name (`INT0`, `TB0`, ...).
pub fn vector_by_name(name: &str) -> Option<u16> {
    VECTORS
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|&(_, a)| a)
}

/// Name of the interrupt served by a vector address.
pub fn vector_name(addr: u16) -> Option<&'static str> {
    VECTORS.iter().find(|&&(_, a)| a == addr).map(|&(n, _)| n)
}
