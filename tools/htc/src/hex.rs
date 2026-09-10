//! Program image container plus Intel HEX / raw binary readers and writers.
//!
//! Program memory is 16-bit wide.  In byte-oriented formats every word is
//! stored little-endian at byte address `2 * word_address`, which is the
//! convention used by Holtek's own tools and by generic device programmers.

use std::fmt;

use crate::device::PROGRAM_WORDS;

/// A program-memory image: 4096 words plus a "used" mask.
#[derive(Clone)]
pub struct Image {
    words: Vec<u16>,
    used: Vec<bool>,
}

impl Default for Image {
    fn default() -> Self {
        Self::new()
    }
}

impl Image {
    pub fn new() -> Self {
        Image { words: vec![0; PROGRAM_WORDS], used: vec![false; PROGRAM_WORDS] }
    }

    pub fn set(&mut self, addr: u16, word: u16) {
        let a = addr as usize;
        self.words[a] = word;
        self.used[a] = true;
    }

    pub fn get(&self, addr: u16) -> u16 {
        self.words[addr as usize]
    }

    pub fn is_used(&self, addr: u16) -> bool {
        self.used[addr as usize]
    }

    pub fn words(&self) -> &[u16] {
        &self.words
    }

    /// Address one past the last used word (0 if empty).
    pub fn end(&self) -> usize {
        self.used.iter().rposition(|&u| u).map(|p| p + 1).unwrap_or(0)
    }

    /// Number of used words.
    pub fn used_words(&self) -> usize {
        self.used.iter().filter(|&&u| u).count()
    }

    /// Serialise to Intel HEX (16-byte records, little-endian words).
    pub fn to_intel_hex(&self) -> String {
        let mut out = String::new();
        let end = self.end();
        let mut addr = 0usize;
        while addr < end {
            // Skip unused stretches, collect a run of up to 8 used words.
            if !self.used[addr] {
                addr += 1;
                continue;
            }
            let mut run = Vec::new();
            while addr < end && self.used[addr] && run.len() < 8 {
                run.push(self.words[addr]);
                addr += 1;
            }
            let byte_addr = (addr - run.len()) * 2;
            let mut bytes = Vec::with_capacity(run.len() * 2 + 4);
            bytes.push((run.len() * 2) as u8);
            bytes.push((byte_addr >> 8) as u8);
            bytes.push(byte_addr as u8);
            bytes.push(0x00);
            for w in &run {
                bytes.push(*w as u8);
                bytes.push((*w >> 8) as u8);
            }
            push_record(&mut out, &bytes);
        }
        push_record(&mut out, &[0x00, 0x00, 0x00, 0x01]);
        out
    }

    /// Serialise to a raw little-endian binary covering `0..end()`.
    pub fn to_binary(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(self.end() * 2);
        for w in &self.words[..self.end()] {
            v.push(*w as u8);
            v.push((*w >> 8) as u8);
        }
        v
    }

    /// Parse a raw little-endian binary.
    pub fn from_binary(bytes: &[u8]) -> Result<Image, ImageError> {
        if bytes.len() % 2 != 0 {
            return Err(ImageError::OddLength);
        }
        if bytes.len() / 2 > PROGRAM_WORDS {
            return Err(ImageError::TooLarge(bytes.len() / 2));
        }
        let mut img = Image::new();
        for (i, pair) in bytes.chunks(2).enumerate() {
            img.set(i as u16, u16::from_le_bytes([pair[0], pair[1]]));
        }
        Ok(img)
    }

    /// Parse Intel HEX text (record types 00/01/04 supported).
    pub fn from_intel_hex(text: &str) -> Result<Image, ImageError> {
        let mut img = Image::new();
        let mut upper: u32 = 0;
        let mut pending: Option<(usize, u8)> = None; // (word address, low byte)
        for (lineno, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() {
                continue;
            }
            let line = line
                .strip_prefix(':')
                .ok_or(ImageError::Syntax(lineno + 1, "missing ':'"))?;
            let bytes = parse_hex_bytes(line).ok_or(ImageError::Syntax(lineno + 1, "bad hex digits"))?;
            if bytes.len() < 5 {
                return Err(ImageError::Syntax(lineno + 1, "record too short"));
            }
            let sum = bytes.iter().fold(0u8, |a, b| a.wrapping_add(*b));
            if sum != 0 {
                return Err(ImageError::Syntax(lineno + 1, "checksum mismatch"));
            }
            let len = bytes[0] as usize;
            let addr = ((bytes[1] as u32) << 8) | bytes[2] as u32;
            let kind = bytes[3];
            let data = &bytes[4..4 + len];
            match kind {
                0x00 => {
                    for (i, b) in data.iter().enumerate() {
                        let byte_addr = (upper + addr + i as u32) as usize;
                        let word_addr = byte_addr / 2;
                        if word_addr >= PROGRAM_WORDS {
                            return Err(ImageError::TooLarge(word_addr + 1));
                        }
                        if byte_addr % 2 == 0 {
                            pending = Some((word_addr, *b));
                        } else {
                            let low = match pending.take() {
                                Some((a, l)) if a == word_addr => l,
                                _ => img.get(word_addr as u16) as u8,
                            };
                            img.set(word_addr as u16, u16::from_le_bytes([low, *b]));
                        }
                    }
                    if let Some((a, l)) = pending.take() {
                        // Lone low byte at the end of a record: keep it.
                        img.set(a as u16, (img.get(a as u16) & 0xFF00) | l as u16);
                        pending = Some((a, l));
                    }
                }
                0x01 => break,
                0x04 => {
                    if data.len() != 2 {
                        return Err(ImageError::Syntax(lineno + 1, "bad extended address"));
                    }
                    upper = (((data[0] as u32) << 8) | data[1] as u32) << 16;
                }
                0x02 | 0x03 | 0x05 => {}
                _ => return Err(ImageError::Syntax(lineno + 1, "unknown record type")),
            }
        }
        Ok(img)
    }
}

fn push_record(out: &mut String, bytes: &[u8]) {
    out.push(':');
    let mut sum: u8 = 0;
    for b in bytes {
        out.push_str(&format!("{:02X}", b));
        sum = sum.wrapping_add(*b);
    }
    out.push_str(&format!("{:02X}\n", (!sum).wrapping_add(1)));
}

fn parse_hex_bytes(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

#[derive(Debug)]
pub enum ImageError {
    OddLength,
    TooLarge(usize),
    Syntax(usize, &'static str),
}

impl fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImageError::OddLength => write!(f, "binary image has an odd number of bytes"),
            ImageError::TooLarge(w) => write!(f, "image needs {} words but the device has {}", w, PROGRAM_WORDS),
            ImageError::Syntax(l, m) => write!(f, "hex line {}: {}", l, m),
        }
    }
}

impl std::error::Error for ImageError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip() {
        let mut img = Image::new();
        img.set(0, 0x2830);
        img.set(1, 0x0F55);
        img.set(0x30, 0x0087);
        img.set(0xFFF, 0xABCD);
        let hex = img.to_intel_hex();
        assert!(hex.starts_with(":040000003028550F"));
        assert!(hex.ends_with(":00000001FF\n"));
        let back = Image::from_intel_hex(&hex).unwrap();
        for a in 0..PROGRAM_WORDS as u16 {
            assert_eq!(back.get(a), img.get(a), "addr {:03x}", a);
            assert_eq!(back.is_used(a), img.is_used(a), "used {:03x}", a);
        }
        let bin = img.to_binary();
        assert_eq!(bin.len(), 0x2000);
        let back2 = Image::from_binary(&bin).unwrap();
        assert_eq!(back2.get(0xFFF), 0xABCD);
    }
}
