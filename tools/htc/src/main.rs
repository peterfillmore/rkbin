//! `htc` command line: compile, assemble, disassemble and simulate programs
//! for the Holtek HT66F0185.

use std::path::{Path, PathBuf};
use std::process::exit;

use htc::asm::{self, Assembler, FsIncludes};
use htc::device;
use htc::disasm;
use htc::hex::Image;
use htc::lang;
use htc::sim::{Cpu, Stop};

const USAGE: &str = "htc — open-source toolchain for the Holtek HT66F0185

USAGE:
    htc build  <file.htc> [-o <out.hex>] [--asm <out.asm>] [--bin <out.bin>] [--lst <out.lst>] [-q]
    htc asm    <file.asm> [-o <out.hex>] [--bin <out.bin>] [--lst <out.lst>] [-q]
    htc disasm <file.hex|file.bin> [--addr]
    htc run    <file.htc|file.asm|file.hex|file.bin> [--cycles <n>] [--trace] [--dump <from>-<to>] [--irq <vector>@<cycle>]
    htc regs

COMMANDS:
    build    compile an htc source file to Intel HEX (default: <file>.hex)
    asm      assemble Holtek assembly source to Intel HEX
    disasm   disassemble a program image
    run      run a program in the built-in core simulator and dump RAM
    regs     list the special function registers and interrupt vectors
";

fn main() {
    // Exit quietly when stdout is closed early (e.g. `htc regs | head`).
    #[cfg(unix)]
    unsafe {
        extern "C" {
            fn signal(sig: i32, handler: usize) -> usize;
        }
        signal(13, 0); // SIGPIPE, SIG_DFL
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args[0] == "-h" || args[0] == "--help" {
        print!("{}", USAGE);
        exit(if args.is_empty() { 2 } else { 0 });
    }
    let result = match args[0].as_str() {
        "build" => cmd_build(&args[1..]),
        "asm" => cmd_asm(&args[1..]),
        "disasm" => cmd_disasm(&args[1..]),
        "run" => cmd_run(&args[1..]),
        "regs" => {
            cmd_regs();
            Ok(())
        }
        "--version" | "-V" => {
            println!("htc {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        other => Err(format!("unknown command '{}'\n\n{}", other, USAGE)),
    };
    if let Err(e) = result {
        eprintln!("{}", e);
        exit(1);
    }
}

struct Opts {
    positional: Vec<String>,
    flags: Vec<(String, Option<String>)>,
}

fn parse_opts(args: &[String], with_value: &[&str]) -> Result<Opts, String> {
    let mut o = Opts {
        positional: Vec::new(),
        flags: Vec::new(),
    };
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a.starts_with('-') && a.len() > 1 {
            if with_value.contains(&a.as_str()) {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| format!("option {} needs a value", a))?;
                o.flags.push((a.clone(), Some(v.clone())));
                i += 2;
            } else {
                o.flags.push((a.clone(), None));
                i += 1;
            }
        } else {
            o.positional.push(a.clone());
            i += 1;
        }
    }
    Ok(o)
}

impl Opts {
    fn value(&self, name: &str) -> Option<&str> {
        self.flags
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .and_then(|(_, v)| v.as_deref())
    }
    fn has(&self, name: &str) -> bool {
        self.flags.iter().any(|(n, _)| n == name)
    }
}

fn read(path: &str) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {}", path, e))
}

fn write(path: &Path, data: &[u8]) -> Result<(), String> {
    std::fs::write(path, data).map_err(|e| format!("cannot write {}: {}", path.display(), e))
}

fn with_ext(path: &str, ext: &str) -> PathBuf {
    Path::new(path).with_extension(ext)
}

fn assemble_source(path: &str, source: &str) -> Result<asm::Assembled, String> {
    let base = Path::new(path)
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_default();
    let resolver = FsIncludes { base };
    Assembler::new(&resolver)
        .assemble(path, source)
        .map_err(|errs| {
            errs.iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("\n")
        })
}

fn compile_source(path: &str, source: &str) -> Result<lang::Output, String> {
    lang::compile(path, source).map_err(|errs| {
        errs.iter()
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    })
}

fn cmd_build(args: &[String]) -> Result<(), String> {
    let o = parse_opts(args, &["-o", "--asm", "--bin", "--lst"])?;
    let input = o.positional.first().ok_or_else(|| USAGE.to_string())?;
    let source = read(input)?;
    let out = compile_source(input, &source)?;
    for w in &out.warnings {
        eprintln!("warning: {}", w);
    }
    if let Some(p) = o.value("--asm") {
        write(Path::new(p), out.asm.as_bytes())?;
    }
    let asm_name = format!("{}.asm", input);
    let assembled = assemble_source(&asm_name, &out.asm).map_err(|e| {
        format!("internal error: generated assembly failed to assemble:\n{}\n(use --asm to inspect the output)", e)
    })?;
    emit_outputs(&o, input, &assembled)?;
    if !o.has("-q") {
        let words = assembled.image.used_words();
        eprintln!(
            "program: {} / {} words ({:.1}%)   data: {} / 128 bytes of bank-0 RAM",
            words,
            device::PROGRAM_WORDS,
            words as f64 * 100.0 / device::PROGRAM_WORDS as f64,
            out.ram_used
        );
        for (name, base, size) in &out.frames {
            if *size > 0 {
                eprintln!(
                    "  frame {:<16} {:#04x}..{:#04x} ({} bytes)",
                    name,
                    base,
                    *base as usize + size - 1,
                    size
                );
            }
        }
    }
    Ok(())
}

fn emit_outputs(o: &Opts, input: &str, assembled: &asm::Assembled) -> Result<(), String> {
    let hex_path = o
        .value("-o")
        .map(PathBuf::from)
        .unwrap_or_else(|| with_ext(input, "hex"));
    write(&hex_path, assembled.image.to_intel_hex().as_bytes())?;
    if let Some(p) = o.value("--bin") {
        write(Path::new(p), &assembled.image.to_binary())?;
    }
    if let Some(p) = o.value("--lst") {
        write(
            Path::new(p),
            asm::format_listing(&assembled.listing).as_bytes(),
        )?;
    }
    if !o.has("-q") {
        eprintln!("wrote {}", hex_path.display());
    }
    Ok(())
}

fn cmd_asm(args: &[String]) -> Result<(), String> {
    let o = parse_opts(args, &["-o", "--bin", "--lst"])?;
    let input = o.positional.first().ok_or_else(|| USAGE.to_string())?;
    let source = read(input)?;
    let assembled = assemble_source(input, &source)?;
    emit_outputs(&o, input, &assembled)?;
    if !o.has("-q") {
        eprintln!("program: {} words", assembled.image.used_words());
    }
    Ok(())
}

fn load_image(path: &str) -> Result<Image, String> {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "hex" | "ihx" => Image::from_intel_hex(&read(path)?).map_err(|e| e.to_string()),
        "asm" | "s" => Ok(assemble_source(path, &read(path)?)?.image),
        "htc" | "c" => {
            let out = compile_source(path, &read(path)?)?;
            for w in &out.warnings {
                eprintln!("warning: {}", w);
            }
            Ok(assemble_source(path, &out.asm)?.image)
        }
        _ => {
            let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {}", path, e))?;
            Image::from_binary(&bytes).map_err(|e| e.to_string())
        }
    }
}

fn cmd_disasm(args: &[String]) -> Result<(), String> {
    let o = parse_opts(args, &[])?;
    let input = o.positional.first().ok_or_else(|| USAGE.to_string())?;
    let image = load_image(input)?;
    print!("{}", disasm::disassemble(&image, o.has("--addr")));
    Ok(())
}

fn cmd_run(args: &[String]) -> Result<(), String> {
    let o = parse_opts(args, &["--cycles", "--dump", "--irq"])?;
    let input = o.positional.first().ok_or_else(|| USAGE.to_string())?;
    let image = load_image(input)?;
    let cycles: u64 = o
        .value("--cycles")
        .unwrap_or("1000000")
        .parse()
        .map_err(|_| "bad --cycles".to_string())?;
    let mut cpu = Cpu::new(&image);
    if o.has("--trace") {
        cpu.trace = Some(Vec::new());
    }
    let mut irqs: Vec<(u16, u64)> = Vec::new();
    for (n, v) in &o.flags {
        if n == "--irq" {
            let v = v.as_deref().unwrap_or("");
            let (vec, at) = v.split_once('@').ok_or("--irq needs <vector>@<cycle>")?;
            let vec = device::vector_by_name(vec)
                .or_else(|| parse_int(vec).map(|x| x as u16))
                .ok_or("bad vector")?;
            let at: u64 = at.parse().map_err(|_| "bad cycle")?;
            irqs.push((vec, at));
        }
    }
    irqs.sort_by_key(|i| i.1);
    let mut stop;
    let mut remaining = cycles;
    loop {
        let next_irq = irqs.first().map(|i| i.1);
        let budget = match next_irq {
            Some(at) if at > cpu.cycles => (at - cpu.cycles).min(remaining),
            Some(_) => 0,
            None => remaining,
        };
        stop = if budget > 0 {
            cpu.run(budget)
        } else {
            Stop::CycleLimit
        };
        if let Some(&(vec, at)) = irqs.first() {
            if cpu.cycles >= at && matches!(stop, Stop::CycleLimit | Stop::Halt) {
                irqs.remove(0);
                if !cpu.interrupt(vec) {
                    eprintln!(
                        "note: interrupt {:#04x} at cycle {} not taken (EMI clear or stack full)",
                        vec, at
                    );
                }
                if remaining > budget {
                    remaining -= budget;
                    continue;
                }
            }
        }
        break;
    }
    if let Some(t) = &cpu.trace {
        for l in t {
            println!("{}", l);
        }
    }
    println!(
        "stopped: {} after {} cycles at pc={:03x}",
        stop, cpu.cycles, cpu.pc
    );
    println!(
        "acc={:02x} status={:02x} (C={} AC={} Z={} OV={}) mp0={:02x} mp1={:02x} bp={:02x} tblp={:02x} tbhp={:02x} stack={:?}",
        cpu.acc(),
        cpu.status(),
        cpu.status() & 1,
        (cpu.status() >> 1) & 1,
        (cpu.status() >> 2) & 1,
        (cpu.status() >> 3) & 1,
        cpu.peek(device::sfr::MP0),
        cpu.peek(device::sfr::MP1),
        cpu.peek(device::sfr::BP),
        cpu.peek(device::sfr::TBLP),
        cpu.peek(device::sfr::TBHP),
        cpu.stack
    );
    let (from, to) = match o.value("--dump") {
        Some(r) => {
            let (a, b) = r.split_once('-').ok_or("--dump needs <from>-<to>")?;
            (
                parse_int(a).ok_or("bad --dump")? as usize,
                parse_int(b).ok_or("bad --dump")? as usize,
            )
        }
        None => (0x80, 0xFF),
    };
    let mut a = from & !0xF;
    while a <= to {
        let row: Vec<String> = (0..16)
            .map(|i| {
                let x = a + i;
                if x < from || x > to || x > 0xFF {
                    "  ".into()
                } else {
                    format!("{:02x}", cpu.peek(x as u8))
                }
            })
            .collect();
        println!("{:02x}: {}", a, row.join(" "));
        a += 16;
    }
    Ok(())
}

fn parse_int(s: &str) -> Option<i64> {
    let t = s.trim().to_ascii_lowercase();
    if let Some(h) = t.strip_prefix("0x") {
        i64::from_str_radix(h, 16).ok()
    } else if let Some(h) = t.strip_suffix('h') {
        i64::from_str_radix(h, 16).ok()
    } else {
        t.parse().ok()
    }
}

fn cmd_regs() {
    println!("Special function registers (data memory, bank 0/1):");
    for (name, addr) in device::SFR_TABLE {
        println!("  {:02X}h  {}", addr, name);
    }
    println!("\nNamed bits:");
    for (name, reg, bit) in device::BIT_TABLE {
        println!(
            "  {:<6} {}.{}",
            name,
            device::sfr_name(*reg).unwrap_or("?"),
            bit
        );
    }
    println!("\nInterrupt vectors:");
    for (name, addr) in device::VECTORS {
        println!("  {:03X}h  {}", addr, name);
    }
}
