#![allow(dead_code)]
use htc::asm::assemble_str;
use htc::lang;
use htc::sim::{Cpu, Stop};

pub struct Run {
    pub cpu: Cpu,
    pub asm: String,
    pub warnings: Vec<String>,
}

pub fn compile(src: &str) -> lang::Output {
    match lang::compile("test.htc", src) {
        Ok(o) => o,
        Err(errs) => panic!(
            "compile failed:\n{}",
            errs.iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("\n")
        ),
    }
}

pub fn build(src: &str) -> Run {
    let out = compile(src);
    let asm = match assemble_str("gen.asm", &out.asm) {
        Ok(a) => a,
        Err(errs) => panic!(
            "generated assembly failed:\n{}\n--- asm ---\n{}",
            errs.iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("\n"),
            out.asm
        ),
    };
    Run {
        cpu: Cpu::new(&asm.image),
        asm: out.asm,
        warnings: out.warnings,
    }
}

pub fn run(src: &str) -> Run {
    let mut r = build(src);
    let stop = r.cpu.run(5_000_000);
    assert_eq!(
        stop,
        Stop::Halt,
        "program did not halt cleanly (pc={:03x})\n--- asm ---\n{}",
        r.cpu.pc,
        r.asm
    );
    r
}

pub fn compile_err(src: &str) -> String {
    match lang::compile("test.htc", src) {
        Ok(_) => panic!("expected a compile error"),
        Err(errs) => errs
            .iter()
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
            .join("\n"),
    }
}
