//! Differential test: random binary expressions compiled for the
//! HT66F0185 and run in the simulator versus a reference evaluation.
mod common;
use common::*;
use htc::lang::ast::Type;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn range(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn rand_val(rng: &mut Rng, t: Type) -> i64 {
    let interesting: &[i64] = &[
        0, 1, 2, 7, 8, 15, 16, 127, 128, 255, 256, 32767, 32768, 65535, -1, -2, -128, -32768,
    ];
    let v = if rng.range(3) == 0 {
        interesting[rng.range(interesting.len() as u64) as usize]
    } else {
        rng.next() as i64
    };
    t.wrap(v)
}

const OPS: &[&str] = &[
    "+", "-", "*", "/", "%", "&", "|", "^", "<<", ">>", "<", "<=", ">", ">=", "==", "!=", "&&",
    "||",
];

fn reference(op: &str, a: i64, b: i64, ta: Type, tb: Type) -> (i64, Type) {
    let common = Type::common(ta, tb);
    let bits = common.size() * 8;
    let boolean = |x: bool| (x as i64, Type::Bool);
    match op {
        "+" => (common.wrap(a + b), common),
        "-" => (common.wrap(a - b), common),
        "*" => (common.wrap(a.wrapping_mul(b)), common),
        "/" => {
            if b == 0 {
                return (0, Type::Void);
            }
            let (x, y) = if common.is_signed() {
                (common.wrap(a), common.wrap(b))
            } else {
                (a & ((1 << bits) - 1), b & ((1 << bits) - 1))
            };
            (common.wrap(x / y), common)
        }
        "%" => {
            if b == 0 {
                return (0, Type::Void);
            }
            let (x, y) = if common.is_signed() {
                (common.wrap(a), common.wrap(b))
            } else {
                (a & ((1 << bits) - 1), b & ((1 << bits) - 1))
            };
            (common.wrap(x % y), common)
        }
        "&" => (common.wrap(a & b), common),
        "|" => (common.wrap(a | b), common),
        "^" => (common.wrap(a ^ b), common),
        "<<" => {
            let t = ta;
            let n = (b & 0xFF) as u32;
            (t.wrap(if n >= 16 { 0 } else { a << n }), t)
        }
        ">>" => {
            let t = ta;
            let n = (b & 0xFF) as u32;
            let x = if t.is_signed() {
                t.wrap(a)
            } else {
                a & ((1 << (t.size() * 8)) - 1)
            };
            (
                t.wrap(if n >= 63 {
                    if x < 0 {
                        -1
                    } else {
                        0
                    }
                } else {
                    x >> n
                }),
                t,
            )
        }
        "<" | "<=" | ">" | ">=" | "==" | "!=" => {
            let (x, y) = if common.is_signed() {
                (common.wrap(a), common.wrap(b))
            } else {
                (a & ((1 << bits) - 1), b & ((1 << bits) - 1))
            };
            boolean(match op {
                "<" => x < y,
                "<=" => x <= y,
                ">" => x > y,
                ">=" => x >= y,
                "==" => x == y,
                _ => x != y,
            })
        }
        "&&" => boolean(a != 0 && b != 0),
        "||" => boolean(a != 0 || b != 0),
        _ => unreachable!(),
    }
}

#[test]
fn random_binary_expressions() {
    let types = [Type::U8, Type::I8, Type::U16, Type::I16];
    let mut rng = Rng(0x9E3779B97F4A7C15);
    let mut cases = Vec::new();
    for _ in 0..400 {
        let ta = types[rng.range(4) as usize];
        let tb = types[rng.range(4) as usize];
        let op = OPS[rng.range(OPS.len() as u64) as usize];
        let a = rand_val(&mut rng, ta);
        let mut b = rand_val(&mut rng, tb);
        if matches!(op, "<<" | ">>") {
            b = rng.range(20) as i64;
        }
        let b_const = rng.range(4) == 0;
        let a_const = !b_const && rng.range(6) == 0;
        cases.push((ta, tb, op, a, b, a_const, b_const));
    }
    // Batch cases into programs of 8 expressions each to keep compile time low.
    let mut failures = Vec::new();
    for chunk in cases.chunks(8) {
        let mut src = String::new();
        let mut expected = Vec::new();
        let mut addr = 0x80usize;
        for (i, (ta, tb, op, a, b, a_const, b_const)) in chunk.iter().enumerate() {
            let (exp, rt) = reference(op, *a, *b, *ta, *tb);
            if rt == Type::Void {
                continue;
            }
            let rty = if rt == Type::Bool { Type::U8 } else { rt };
            src.push_str(&format!("{} r{};\n", rty, i));
            expected.push((
                addr,
                rty,
                exp,
                format!(
                    "{} {} {} ({} {} {})",
                    a,
                    op,
                    b,
                    ta,
                    tb,
                    if *b_const {
                        "b const"
                    } else if *a_const {
                        "a const"
                    } else {
                        ""
                    }
                ),
            ));
            addr += rty.size();
        }
        for (i, (ta, tb, op, a, b, _, _)) in chunk.iter().enumerate() {
            let (_, rt) = reference(op, *a, *b, *ta, *tb);
            if rt == Type::Void {
                continue;
            }
            src.push_str(&format!("{} a{} = {};\n{} b{} = {};\n", ta, i, a, tb, i, b));
        }
        src.push_str("void main() {\n");
        for (i, (ta, tb, op, a, b, a_const, b_const)) in chunk.iter().enumerate() {
            let (_, rt) = reference(op, *a, *b, *ta, *tb);
            if rt == Type::Void {
                continue;
            }
            let lhs = if *a_const {
                format!("({}){}", ta, a)
            } else {
                format!("a{}", i)
            };
            let rhs = if *b_const {
                format!("({}){}", tb, b)
            } else {
                format!("b{}", i)
            };
            src.push_str(&format!("    r{} = {} {} {};\n", i, lhs, op, rhs));
        }
        src.push_str("}\n");
        let r = run(&src);
        for (addr, rty, exp, desc) in expected {
            let got = if rty.size() == 1 {
                rty.wrap(r.cpu.peek(addr as u8) as i64)
            } else {
                rty.wrap(r.cpu.peek16(addr as u8) as i64)
            };
            if got != exp {
                failures.push(format!("{}: expected {}, got {}", desc, exp, got));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
