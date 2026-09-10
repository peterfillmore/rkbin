mod common;
use common::*;

#[test]
fn arithmetic_8bit() {
    let r = run("
        u8 a; u8 b; u8 c; u8 d; u8 e; u8 f;
        void main() {
            u8 x = 200; u8 y = 100;
            a = x + y;          // 44 (wraps)
            b = x - y;          // 100
            c = y - x;          // 156
            d = (x & 0x0F) | 0x30;  // 0x38
            e = ~x;             // 55
            f = -y;             // 156
        }");
    assert_eq!(&r.cpu.ram0[0x80..0x86], &[44, 100, 156, 0x38, 55, 156]);
}

#[test]
fn arithmetic_16bit() {
    let r = run("
        u16 a; u16 b; u16 c; i16 d; u16 e; u16 f;
        void main() {
            u16 x = 60000; u16 y = 12345;
            a = x + y;          // 6809 (wraps)
            b = x - y;          // 47655
            c = y - x;          // 17881
            d = -300;
            d = d + 100;        // -200
            e = (x ^ y) & 0x0FF0;
            f = y >> 3;         // 1543
        }");
    assert_eq!(r.cpu.peek16(0x80), 6809);
    assert_eq!(r.cpu.peek16(0x82), 47655);
    assert_eq!(r.cpu.peek16(0x84), 17881);
    assert_eq!(r.cpu.peek16(0x86) as i16, -200);
    assert_eq!(r.cpu.peek16(0x88), (60000u16 ^ 12345) & 0x0FF0);
    assert_eq!(r.cpu.peek16(0x8a), 1543);
}

#[test]
fn mul_div_rem() {
    let r = run("
        u8 r0; u8 r1; u8 r2; i8 r3; i8 r4;
        u16 w0; u16 w1; u16 w2; i16 w3; i16 w4;
        void main() {
            u8 a = 23; u8 b = 7;
            r0 = a * b;      // 161
            r1 = a / b;      // 3
            r2 = a % b;      // 2
            i8 sa = -23; i8 sb = 7;
            r3 = sa / sb;    // -3
            r4 = sa % sb;    // -2
            u16 x = 1234; u16 y = 45;
            w0 = x * y;      // 55530
            w1 = x / y;      // 27
            w2 = x % y;      // 19
            i16 sx = -1234; i16 sy = 45;
            w3 = sx / sy;    // -27
            w4 = sx % sy;    // -19
        }");
    assert_eq!(&r.cpu.ram0[0x80..0x83], &[161, 3, 2]);
    assert_eq!(r.cpu.peek(0x83) as i8, -3);
    assert_eq!(r.cpu.peek(0x84) as i8, -2);
    assert_eq!(r.cpu.peek16(0x85), 55530);
    assert_eq!(r.cpu.peek16(0x87), 27);
    assert_eq!(r.cpu.peek16(0x89), 19);
    assert_eq!(r.cpu.peek16(0x8b) as i16, -27);
    assert_eq!(r.cpu.peek16(0x8d) as i16, -19);
}

#[test]
fn comparisons_and_logic() {
    let r = run("
        u8 out[12];
        void main() {
            u8 a = 5; u8 b = 200; i8 sa = -5; i8 sb = 100;
            u16 x = 300; u16 y = 60000; i16 sx = -300; i16 sy = 20000;
            out[0] = a < b;      // 1 (unsigned)
            out[1] = sa < sb;    // 1 (signed)
            out[2] = (i8)b < sa; // 1 : -56 < -5
            out[3] = x <= y;     // 1
            out[4] = sx < sy;    // 1
            out[5] = (i16)y < sx;// 1 : -5536 < -300
            out[6] = a == 5 && b != 5;   // 1
            out[7] = a > b || x > y;     // 0
            out[8] = !(a >= b);          // 1
            out[9] = (a < b) + (x == 300) + (sx >= sy); // 2
            out[10] = x != 300;  // 0
            out[11] = a == b ? 9 : 8;    // 8
        }");
    assert_eq!(
        &r.cpu.ram0[0x80..0x8c],
        &[1, 1, 1, 1, 1, 1, 1, 0, 1, 2, 0, 8]
    );
}

#[test]
fn control_flow() {
    let r = run("
        u8 sum; u8 n; u8 sw; u8 dw; u8 cont;
        void main() {
            u8 i;
            for (i = 0; i < 10; i++) {
                if (i == 7) break;
                if (i & 1) continue;
                sum += i;               // 0+2+4+6 = 12
            }
            n = 0;
            while (n < 250) n += 50;    // 250
            u8 k = 0;
            do { k++; } while (k < 3);  // 3
            dw = k;
            switch (i) {
                case 1: sw = 1; break;
                case 7: sw = 7;         // falls through
                case 8: sw += 1; break;
                default: sw = 99;
            }
            cont = 0;
            for (u8 j = 0; j < 5; j++) { if (j == 2) continue; cont++; }  // 4
        }");
    assert_eq!(&r.cpu.ram0[0x80..0x85], &[12, 250, 8, 3, 4]);
}

#[test]
fn arrays_and_indexing() {
    let r = run("
        u8 buf[6] = {1, 2, 3};
        u16 words[3];
        u8 res; u16 wres; u8 idx; u8 bit;
        void main() {
            u8 i;
            for (i = 3; i < 6; i++) buf[i] = buf[i - 1] + buf[i - 2];  // 5 8 13
            res = buf[5];
            for (i = 0; i < 3; i++) words[i] = (u16)buf[i] * 1000;
            wres = words[2] + words[1];   // 5000
            i = 1;
            buf[i]++;
            buf[i] += buf[i + 1];         // 2+1+3 = 6
            idx = buf[1];
            buf[i].7 = 1;
            bit = buf[1].7;
            words[i]--;
        }");
    assert_eq!(&r.cpu.ram0[0x80..0x86], &[1, 6 | 0x80, 3, 5, 8, 13]);
    assert_eq!(r.cpu.peek(0x8C), 13);
    assert_eq!(r.cpu.peek16(0x8D), 5000);
    assert_eq!(r.cpu.peek(0x8F), 6);
    assert_eq!(r.cpu.peek(0x90), 1);
    assert_eq!(r.cpu.peek16(0x88), 1999);
}

#[test]
fn const_tables() {
    let r = run("
        const u8 sine[] = {0, 49, 90, 117, 127, 117, 90, 49};
        const u16 big[] = {1000, 2000, 3000, 65535};
        const u8 N = 3;
        u8 out; u16 wout; u8 s; u16 acc;
        void main() {
            u8 i = N + 1;
            out = sine[i];        // 127
            wout = big[N];        // 65535
            s = 0;
            for (i = 0; i < 8; i++) s += sine[i] >> 4;  // 0+3+5+7+7+7+5+3 = 37
            acc = big[1] + big[2];
        }");
    assert_eq!(r.cpu.peek(0x80), 127);
    assert_eq!(r.cpu.peek16(0x81), 65535);
    assert_eq!(r.cpu.peek(0x83), 37);
    assert_eq!(r.cpu.peek16(0x84), 5000);
}

#[test]
fn functions_and_frames() {
    let r = run("
        u8 out; u16 wout; u8 depth;
        u8 sq(u8 x) { return x * x; }
        u16 sum_sq(u8 n) { u16 s = 0; u8 i; for (i = 1; i <= n; i++) s += sq(i); return s; }
        u8 max3(u8 a, u8 b, u8 c) { u8 m = a; if (b > m) m = b; if (c > m) m = c; return m; }
        u16 twice(u16 v) { return v + v; }
        void main() {
            out = max3(sq(3), 7, sq(2) + sq(1));   // 9
            wout = twice(sum_sq(10));               // 2*385 = 770
            depth = max3(1, max3(2, 9, 3), max3(4, 5, 6)); // 9
        }");
    assert_eq!(r.cpu.peek(0x80), 9);
    assert_eq!(r.cpu.peek16(0x81), 770);
    assert_eq!(r.cpu.peek(0x83), 9);
}

#[test]
fn sfr_and_bits() {
    let r = run("
        u8 out; u8 flags; u8 cnt;
        void main() {
            PAC = 0x0F;          // direction register
            PA = 0;
            PA.3 = 1;
            PA.0 = PAC.1;
            flags = 0;
            flags.5 = PA.3 && !PA.2;
            if (PA.3) cnt++;
            if (!PA.1) cnt++;
            if (flags.5 == 1) cnt++;
            out = PA;
            ei();
            if (EMI) cnt++;
            di();
        }");
    assert_eq!(r.cpu.peek(0x80), 0x09);
    assert_eq!(r.cpu.peek(0x81), 0x20);
    assert_eq!(r.cpu.peek(0x82), 4);
    assert_eq!(r.cpu.peek(0x15), 0x0F);
    assert_eq!(r.cpu.peek(0x0E) & 1, 0);
}

#[test]
fn shifts() {
    let r = run("
        u8 o[8]; u16 w[4]; i8 s[2]; i16 sw[2];
        void main() {
            u8 x = 0xC5; u8 n = 3;
            o[0] = x << 1;      // 0x8A
            o[1] = x >> 2;      // 0x31
            o[2] = x << n;      // 0x28
            o[3] = x >> n;      // 0x18
            o[4] = 1 << 7;      // 0x80
            u16 y = 0xC5A3;
            w[0] = y << 1;      // 0x8B46
            w[1] = y >> 4;      // 0x0C5A
            w[2] = y >> n;      // 0x18B4
            w[3] = y << 9;      // 0x4600
            i8 sx = -100;
            s[0] = sx >> 2;     // -25
            s[1] = sx >> n;     // -13
            i16 swx = -1000;
            sw[0] = swx >> 3;   // -125
            sw[1] = swx >> n;   // -125
        }");
    assert_eq!(&r.cpu.ram0[0x80..0x85], &[0x8A, 0x31, 0x28, 0x18, 0x80]);
    assert_eq!(r.cpu.peek16(0x88), 0x8B46);
    assert_eq!(r.cpu.peek16(0x8A), 0x0C5A);
    assert_eq!(r.cpu.peek16(0x8C), 0x18B4);
    assert_eq!(r.cpu.peek16(0x8E), 0x4600);
    assert_eq!(r.cpu.peek(0x90) as i8, -25);
    assert_eq!(r.cpu.peek(0x91) as i8, -13);
    assert_eq!(r.cpu.peek16(0x92) as i16, -125);
    assert_eq!(r.cpu.peek16(0x94) as i16, -125);
}

#[test]
fn casts_and_conversions() {
    let r = run("
        u16 a; i16 b; u8 c; u8 d; bool e; u16 f;
        void main() {
            u8 x = 200; i8 sx = -3;
            a = x;              // 200 (zero extend)
            b = sx;             // -3 (sign extend)
            u16 w = 0x1234;
            c = (u8)w;          // 0x34
            d = (u8)(w >> 8);   // 0x12
            e = (bool)w;        // 1
            f = (u16)(i16)sx;   // 0xFFFD
        }");
    assert_eq!(r.cpu.peek16(0x80), 200);
    assert_eq!(r.cpu.peek16(0x82) as i16, -3);
    assert_eq!(r.cpu.peek(0x84), 0x34);
    assert_eq!(r.cpu.peek(0x85), 0x12);
    assert_eq!(r.cpu.peek(0x86), 1);
    assert_eq!(r.cpu.peek16(0x87), 0xFFFD);
}

#[test]
fn increments_and_compound() {
    let r = run("
        u8 o[6]; u16 w[3];
        void main() {
            u8 x = 5;
            o[0] = x++;         // 5, x=6
            o[1] = ++x;         // 7
            o[2] = x--;         // 7, x=6
            o[3] = --x;         // 5
            x *= 3;             // 15
            x -= 4;             // 11
            x |= 0x40;          // 0x4B
            x ^= 0x0B;          // 0x40
            x >>= 2;            // 0x10
            x <<= 1;            // 0x20
            o[4] = x;
            x /= 3;             // 10
            x %= 4;             // 2
            o[5] = x;
            u16 y = 255;
            y++;                // 256
            w[0] = y;
            y--; y--;           // 254
            w[1] = y;
            y += 1000;          // 1254
            y -= 4;             // 1250
            y *= 3;             // 3750
            w[2] = y;
        }");
    assert_eq!(&r.cpu.ram0[0x80..0x86], &[5, 7, 7, 5, 0x20, 2]);
    assert_eq!(r.cpu.peek16(0x86), 256);
    assert_eq!(r.cpu.peek16(0x88), 254);
    assert_eq!(r.cpu.peek16(0x8A), 3750);
}

#[test]
fn interrupt_handler_saves_state() {
    let mut r = build(
        "
        u8 ticks; u8 result; u8 tbl_result;
        u8 scratch[4];
        const u8 table[] = {5, 6, 7, 8};
        u8 mul(u8 a, u8 b) { return a * b; }
        interrupt TB0 void tick() {
            ticks++;
            scratch[ticks] = table[ticks] * 2;   // uses MP0, TBLP/TBHP and __mul8
        }
        void main() {
            u8 i;
            ei();
            result = 0;
            for (i = 0; i < 20; i++) {
                result += mul(i, 3) + table[i & 3];
            }
            di();
            tbl_result = scratch[1] + scratch[2];
        }",
    );
    // Fire the interrupt at several points while main is running.
    let mut fired = 0;
    for _ in 0..40 {
        let stop = r.cpu.run(37);
        if stop == htc::sim::Stop::Halt {
            break;
        }
        if r.cpu.peek(0x0E) & 1 != 0 && fired < 2 {
            assert!(r.cpu.interrupt(0x1C));
            fired += 1;
        }
    }
    let stop = r.cpu.run(1_000_000);
    assert_eq!(stop, htc::sim::Stop::Halt);
    assert_eq!(fired, 2);
    assert_eq!(r.cpu.peek(0x80), 2);
    // result = sum(i*3) + sum(table[i&3]) = 3*190 + 5*(5+6+7+8) = 570+130 = 700 mod 256 = 188
    assert_eq!(r.cpu.peek(0x81), 188);
    assert_eq!(r.cpu.peek(0x82), 12 + 14);
    assert!(r.asm.contains("reti"));
}

#[test]
fn inline_asm() {
    let r = run("
        u8 out;
        void main() {
            out = 1;
            asm {
                mov a,[_out]
                add a,41
                mov [_out],a
            }
        }");
    assert_eq!(r.cpu.peek(0x80), 42);
}

#[test]
fn halt_and_main_exit() {
    let r = run("u8 x; void main() { x = 1; halt(); x = 2; }");
    assert_eq!(r.cpu.peek(0x80), 1);
}

#[test]
fn diagnostics() {
    assert!(compile_err("void main() { x = 1; }").contains("undefined variable 'x'"));
    assert!(
        compile_err("void main() { }  void f() { g(); } void g() { f(); }").contains("recursion")
    );
    assert!(compile_err("u8 a[3]; void main() { a[5] = 1; }").contains("out of bounds"));
    assert!(compile_err("void main() { break; }").contains("break"));
    assert!(compile_err("void f() {}").contains("main"));
    assert!(compile_err("u8 f(u8 a) { return a; } void main() { f(); }").contains("expects 1"));
    assert!(compile_err("void main() { u8 x = 1 / 0; }").contains("division by zero"));
    assert!(compile_err("u8 big[200]; u8 more[100]; void main() {}").contains("out of data memory"));
    assert!(compile_err("void main() { u8 x = 1 }").contains("expected"));
}

#[test]
fn named_constants_fold() {
    let r = run("
        const u8 N = 4;
        const u8 MASK = 0x0F;
        const u8 TXEN = 0x80; const u8 RXEN = 0x40;
        const u8 TABLE[] = {3, 5, 7, 11};
        u8 buf[N * 2];
        u8 a; u8 b; u8 c; u8 d;
        void main() {
            const u8 LOCAL = N + 1;
            u8 i;
            for (i = 0; i < N * 2; i++) buf[i] = i * LOCAL;
            a = TXEN | RXEN;             // folded to 0xC0
            b = buf[N] & MASK;           // 20 & 15 = 4
            c = TABLE[N - 1] + TABLE[0]; // 14, folded
            switch (b) { case N: d = 1; break; case LOCAL: d = 2; break; default: d = 3; }
        }");
    assert_eq!(&r.cpu.ram0[0x80..0x88], &[0, 5, 10, 15, 20, 25, 30, 35]);
    assert_eq!(&r.cpu.ram0[0x88..0x8C], &[0xC0, 4, 14, 1]);
    assert!(
        r.asm.contains("mov a,192"),
        "constant should be folded:\n{}",
        r.asm
    );
    assert!(compile_err("const u8 N = 0; u8 buf[N]; void main() {}").contains("array length"));
    assert!(
        compile_err("u8 n; void main() { switch (n) { case n: break; } }").contains("constant")
    );
}
