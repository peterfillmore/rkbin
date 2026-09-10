//! Differential test over random *nested* expression trees, evaluated by a
//! reference interpreter that implements the language's typing rules.
mod common;
use common::*;
use htc::lang::ast::{BinOp, Expr, ExprKind, Pos, Type, UnOp};
use htc::lang::parser::fold_const;

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

const VARS: &[(&str, Type)] = &[
    ("a", Type::U8),
    ("b", Type::I8),
    ("c", Type::U16),
    ("d", Type::I16),
    ("e", Type::U8),
    ("f", Type::U16),
];
const BINOPS: &[BinOp] = &[
    BinOp::Add,
    BinOp::Sub,
    BinOp::Mul,
    BinOp::Div,
    BinOp::Rem,
    BinOp::And,
    BinOp::Or,
    BinOp::Xor,
    BinOp::Shl,
    BinOp::Shr,
    BinOp::Lt,
    BinOp::Le,
    BinOp::Gt,
    BinOp::Ge,
    BinOp::Eq,
    BinOp::Ne,
    BinOp::LAnd,
    BinOp::LOr,
];

fn pos() -> Pos {
    Pos { line: 1, col: 1 }
}

fn gen(rng: &mut Rng, depth: u32) -> Expr {
    let kind = if depth == 0 {
        rng.range(3)
    } else {
        rng.range(10)
    };
    let kind = match kind {
        0 | 1 => {
            let (n, _) = VARS[rng.range(VARS.len() as u64) as usize];
            ExprKind::Var(n.to_string())
        }
        2 => {
            let v = match rng.range(4) {
                0 => rng.range(8) as i64,
                1 => rng.range(256) as i64,
                2 => -(rng.range(129) as i64),
                _ => rng.range(65536) as i64,
            };
            if rng.range(3) == 0 {
                let t = VARS[rng.range(VARS.len() as u64) as usize].1;
                ExprKind::Cast(
                    t,
                    Box::new(Expr {
                        kind: ExprKind::Int(v),
                        pos: pos(),
                    }),
                )
            } else {
                ExprKind::Int(v)
            }
        }
        3 => {
            let op = [UnOp::Neg, UnOp::Not, UnOp::LNot][rng.range(3) as usize];
            ExprKind::Unary(op, Box::new(gen(rng, depth - 1)))
        }
        4 => {
            let t = VARS[rng.range(VARS.len() as u64) as usize].1;
            ExprKind::Cast(t, Box::new(gen(rng, depth - 1)))
        }
        5 => ExprKind::Ternary(
            Box::new(gen(rng, depth - 1)),
            Box::new(gen(rng, depth - 1)),
            Box::new(gen(rng, depth - 1)),
        ),
        _ => {
            let op = BINOPS[rng.range(BINOPS.len() as u64) as usize];
            let l = gen(rng, depth - 1);
            let mut r = gen(rng, depth - 1);
            if matches!(op, BinOp::Shl | BinOp::Shr) && rng.range(2) == 0 {
                r = Expr {
                    kind: ExprKind::Int(rng.range(18) as i64),
                    pos: pos(),
                };
            }
            ExprKind::Binary(op, Box::new(l), Box::new(r))
        }
    };
    Expr { kind, pos: pos() }
}

fn src(e: &Expr) -> String {
    match &e.kind {
        ExprKind::Int(v) => {
            if *v < 0 {
                format!("({})", v)
            } else {
                v.to_string()
            }
        }
        ExprKind::Var(n) => n.clone(),
        ExprKind::Cast(t, x) => format!("(({}){})", t, src(x)),
        ExprKind::Unary(op, x) => {
            let s = match op {
                UnOp::Neg => "-",
                UnOp::Not => "~",
                UnOp::LNot => "!",
            };
            format!("({}{})", s, src(x))
        }
        ExprKind::Binary(op, l, r) => format!("({} {} {})", src(l), op.symbol(), src(r)),
        ExprKind::Ternary(c, a, b) => format!("({} ? {} : {})", src(c), src(a), src(b)),
        _ => unreachable!(),
    }
}

fn var_type(n: &str) -> Type {
    VARS.iter().find(|v| v.0 == n).unwrap().1
}

fn literal_type(v: i64) -> Type {
    if (0..=255).contains(&v) {
        Type::U8
    } else if (-128..0).contains(&v) {
        Type::I8
    } else if (256..=65535).contains(&v) {
        Type::U16
    } else {
        Type::I16
    }
}

fn const_type(e: &Expr, v: i64) -> Type {
    match &e.kind {
        ExprKind::Bool(_) => Type::Bool,
        ExprKind::Cast(t, _) => *t,
        ExprKind::Binary(op, ..) if op.is_comparison() || op.is_logical() => Type::Bool,
        ExprKind::Unary(UnOp::LNot, _) => Type::Bool,
        _ => literal_type(v),
    }
}

fn type_of(e: &Expr) -> Type {
    if let Some(v) = fold_const(e) {
        return const_type(e, v);
    }
    match &e.kind {
        ExprKind::Int(v) => literal_type(*v),
        ExprKind::Var(n) => var_type(n),
        ExprKind::Cast(t, _) => *t,
        ExprKind::Unary(UnOp::LNot, _) => Type::Bool,
        ExprKind::Unary(_, x) => {
            let t = type_of(x);
            if t == Type::Bool {
                Type::U8
            } else {
                t
            }
        }
        ExprKind::Binary(op, l, r) => {
            if op.is_comparison() || op.is_logical() {
                Type::Bool
            } else if matches!(op, BinOp::Shl | BinOp::Shr) {
                let t = type_of(l);
                if t == Type::Bool {
                    Type::U8
                } else {
                    t
                }
            } else {
                Type::common(type_of(l), type_of(r))
            }
        }
        ExprKind::Ternary(_, a, b) => Type::common(type_of(a), type_of(b)),
        _ => unreachable!(),
    }
}

/// Interpret with the language semantics.  `None` on division by zero.
fn eval(e: &Expr, env: &[(&str, i64)]) -> Option<i64> {
    let ty = type_of(e);
    if let Some(v) = fold_const(e) {
        return Some(ty.wrap(v));
    }
    let as_type = |v: i64, t: Type| -> i64 {
        if t.is_signed() {
            t.wrap(v)
        } else {
            v & ((1i64 << (t.size() * 8)) - 1)
        }
    };
    Some(match &e.kind {
        ExprKind::Int(v) => *v,
        ExprKind::Var(n) => env.iter().find(|v| v.0 == *n).unwrap().1,
        ExprKind::Cast(t, x) => t.wrap(eval(x, env)?),
        ExprKind::Unary(op, x) => {
            let v = eval(x, env)?;
            match op {
                UnOp::Neg => ty.wrap(-v),
                UnOp::Not => ty.wrap(!v),
                UnOp::LNot => (v == 0) as i64,
            }
        }
        ExprKind::Ternary(c, a, b) => {
            let cv = eval(c, env)?;
            let (va, vb) = (eval(a, env), eval(b, env));
            ty.wrap(if cv != 0 { va? } else { vb? })
        }
        ExprKind::Binary(op, l, r) => {
            let tl = type_of(l);
            let tr = type_of(r);
            let common = Type::common(tl, tr);
            match op {
                BinOp::LAnd => {
                    let a = eval(l, env)?;
                    let b = eval(r, env);
                    // Short-circuit: the right side is not evaluated when a == 0.
                    if a == 0 {
                        0
                    } else {
                        (b? != 0) as i64
                    }
                }
                BinOp::LOr => {
                    let a = eval(l, env)?;
                    let b = eval(r, env);
                    if a != 0 {
                        1
                    } else {
                        (b? != 0) as i64
                    }
                }
                _ => {
                    let a = eval(l, env)?;
                    let b = eval(r, env)?;
                    match op {
                        BinOp::Add => common.wrap(a + b),
                        BinOp::Sub => common.wrap(a - b),
                        BinOp::Mul => common.wrap(a.wrapping_mul(b)),
                        BinOp::Div | BinOp::Rem => {
                            let (x, y) = (as_type(a, common), as_type(b, common));
                            if y == 0 {
                                return None;
                            }
                            common.wrap(if *op == BinOp::Div {
                                x.wrapping_div(y)
                            } else {
                                x.wrapping_rem(y)
                            })
                        }
                        BinOp::And => common.wrap(a & b),
                        BinOp::Or => common.wrap(a | b),
                        BinOp::Xor => common.wrap(a ^ b),
                        BinOp::Shl => {
                            let n = (b & 0xFF) as u32;
                            ty.wrap(if n >= 16 { 0 } else { a << n })
                        }
                        BinOp::Shr => {
                            let n = (b & 0xFF) as u32;
                            let x = as_type(a, ty);
                            ty.wrap(if n >= 63 {
                                if x < 0 {
                                    -1
                                } else {
                                    0
                                }
                            } else {
                                x >> n
                            })
                        }
                        _ => {
                            let (x, y) = (as_type(a, common), as_type(b, common));
                            (match op {
                                BinOp::Lt => x < y,
                                BinOp::Le => x <= y,
                                BinOp::Gt => x > y,
                                BinOp::Ge => x >= y,
                                BinOp::Eq => x == y,
                                BinOp::Ne => x != y,
                                _ => unreachable!(),
                            }) as i64
                        }
                    }
                }
            }
        }
        _ => unreachable!(),
    })
}

type Case = (Expr, Vec<(&'static str, i64)>, i64, Type);

fn check(seed: u64, count: usize, depth: u32) {
    let mut rng = Rng(seed);
    let mut failures = Vec::new();
    let mut tested = 0;
    let mut batch: Vec<Case> = Vec::new();
    let flush = |batch: &mut Vec<Case>, failures: &mut Vec<String>| {
        if batch.is_empty() {
            return;
        }
        let mut s = String::new();
        let mut addr = 0x80usize;
        let mut addrs = Vec::new();
        for (i, (_, _, _, ty)) in batch.iter().enumerate() {
            let rty = if *ty == Type::Bool { Type::U8 } else { *ty };
            s.push_str(&format!("{} r{};\n", rty, i));
            addrs.push((addr, rty));
            addr += rty.size();
        }
        for (n, t) in VARS {
            s.push_str(&format!("{} {};\n", t, n));
        }
        s.push_str("void main() {\n");
        for (i, (e, env, _, _)) in batch.iter().enumerate() {
            for (n, v) in env {
                s.push_str(&format!("    {} = {};\n", n, v));
            }
            s.push_str(&format!("    r{} = {};\n", i, src(e)));
        }
        s.push_str("}\n");
        let r = run(&s);
        for (i, (e, env, exp, _)) in batch.iter().enumerate() {
            let (addr, rty) = addrs[i];
            let got = if rty.size() == 1 {
                rty.wrap(r.cpu.peek(addr as u8) as i64)
            } else {
                rty.wrap(r.cpu.peek16(addr as u8) as i64)
            };
            if got != rty.wrap(*exp) {
                failures.push(format!(
                    "{} with {:?}: expected {}, got {}",
                    src(e),
                    env,
                    rty.wrap(*exp),
                    got
                ));
            }
        }
        batch.clear();
    };
    while tested < count {
        let e = gen(&mut rng, depth);
        let env: Vec<(&str, i64)> = VARS
            .iter()
            .map(|(n, t)| {
                let v = match rng.range(4) {
                    0 => rng.range(4) as i64,
                    1 => rng.range(256) as i64 - 128,
                    _ => rng.next() as i64,
                };
                (*n, t.wrap(v))
            })
            .collect();
        let exp = match eval(&e, &env) {
            Some(v) => v,
            None => continue,
        };
        let ty = type_of(&e);
        tested += 1;
        batch.push((e, env, exp, ty));
        if batch.len() == 6 {
            flush(&mut batch, &mut failures);
        }
    }
    flush(&mut batch, &mut failures);
    assert!(
        failures.is_empty(),
        "{} of {} failed:\n{}",
        failures.len(),
        tested,
        failures.join("\n")
    );
}

#[test]
fn random_trees_depth2() {
    check(0x1234_5678_9ABC_DEF1, 300, 2);
}

#[test]
fn random_trees_depth3() {
    check(0xDEAD_BEEF_CAFE_F00D, 300, 3);
}

#[test]
fn random_trees_depth4() {
    check(0x0BAD_5EED_1234_0001, 150, 4);
}
