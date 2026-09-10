# htc — an open-source toolchain for the Holtek HT66F0185

`htc` is a compiler, assembler, disassembler and core simulator for the
Holtek **HT66F0185** 8-bit A/D Flash MCU (and its smaller sibling, the
HT66F0175), written in Rust with no external dependencies.

* **Compiler** for `htc`, a small C-like language (8/16-bit integers,
  arrays, functions, interrupt handlers, bit access, constant tables in
  program memory, inline assembly).
* **Assembler** accepting Holtek HT-IDE style syntax with the device's
  special-function-register and bit names predefined.
* **Disassembler** producing re-assemblable source.
* **Simulator** of the CPU core (ALU, flags, skips, hardware stack, indirect
  addressing, bank switching, table reads, PCL jumps, interrupt injection),
  used by the test-suite to verify generated code end to end.
* Output as **Intel HEX** (little-endian 16-bit words at byte address
  `2 × word address`) or raw binary, plus listing files.

The instruction encoding was recovered from Holtek's own tool-chain
description files and verified against `HGASM` listing output (see
[docs/isa.md](docs/isa.md)); the device description follows the
HT66F0175/HT66F0185 datasheet rev. 1.50.

## Building

```sh
cd tools/htc
cargo build --release          # binary in target/release/htc
cargo test                     # unit, integration and randomized tests
```

## Quick start

```c
// blink.htc
void delay(u16 n) { while (n != 0) { n--; clr_wdt(); } }

void main() {
    PAC = 0xFE;                  // PA0 output
    while (true) {
        PA.0 = !PA.0;            // toggle the pin
        delay(20000);
    }
}
```

```sh
htc build blink.htc                     # writes blink.hex
htc build blink.htc --asm blink.asm     # also keep the generated assembly
htc run blink.htc --cycles 100000       # run in the simulator, dump RAM
```

Program `blink.hex` with any HT66F-capable programmer (Holtek e-Writer /
HOPE3000 accept Intel HEX files).

## Command line

```
htc build  <file.htc> [-o out.hex] [--asm out.asm] [--bin out.bin] [--lst out.lst] [-q]
htc asm    <file.asm> [-o out.hex] [--bin out.bin] [--lst out.lst] [-q]
htc disasm <file.hex|file.bin> [--addr]
htc run    <file.htc|.asm|.hex|.bin> [--cycles n] [--trace] [--dump from-to] [--irq VEC@cycle]
htc regs
```

`htc build` prints the program-memory usage and the RAM frame of every
function.  `htc run` executes until `halt`, an error, or the cycle budget,
then prints the CPU state and a RAM dump; `--irq TB0@500` injects an
interrupt through the named vector once 500 cycles have elapsed.

## The `htc` language in one page

```c
u8  counter;                     // globals live in bank-0 RAM (80h..FFh), zeroed at reset
u16 total = 1000;                // initialised at start-up
i8  temperature;                 // i8/i16 are two's complement
bool ready;
u8  buffer[16];                  // arrays (up to 256 elements)
u8  shadow @ 0xF0;               // absolute placement
const u8  SINE[] = {0, 49, 90};  // const arrays live in program memory (TABRD)
const u16 LIMIT = 4000;          // const scalars are compile-time constants

u8 add(u8 a, u8 b) { return a + b; }        // functions: static frames, no recursion
u16 wide(u16 x) { return x * 3 + LIMIT; }   // 16-bit arithmetic (mul/div via runtime library)

interrupt TB0 void tick() { counter++; }    // ISR bound to the Time Base 0 vector (1Ch)

void main() {
    PAC = 0x0F;                  // special function registers are predefined (upper case)
    PA.3 = 1;                    // bit access on any u8 lvalue
    if (PA.2 && !ready) ready = true;
    EMI = 1;                     // named register bits: C, Z, EMI, TB0E, INT0F, ...
    for (u8 i = 0; i < 16; i++) buffer[i] = SINE[i & 3] + add(i, 2);
    switch (counter) { case 1: total++; break; default: total = 0; }
    asm { mov a,[_counter]  ; inline assembly, globals are visible as _name
          mov [_shadow],a }
    halt();                      // intrinsics: halt() nop() clr_wdt() ei() di()
}
```

* Types: `void`, `bool`, `u8`, `i8`, `u16`, `i16` (aliases `uint8_t`,
  `int8_t`, `uint16_t`, `int16_t`, `char`, `int`).
* Arithmetic is performed in the wider of the two operand types (never
  wider than 16 bits; signed if either side is signed) and wraps.
  Constant expressions are folded at compile time.
* Statements: `if/else`, `while`, `do/while`, `for`, `switch`, `break`,
  `continue`, `return`, blocks, declarations anywhere, `asm { }`.
* Operators: all of C's arithmetic, bitwise, logical, comparison,
  assignment, compound assignment, `++`/`--`, casts, `?:`, plus `.n` bit access.
* Interrupt handlers: `interrupt INT0|CMP|MF0|MF1|MF2|ADC|TB0|TB1|INT1|SIM|UART void f()`
  (or `interrupt(0x04)`); the compiler saves/restores ACC, STATUS and, when
  needed, MP0, TBLP/TBHP and the runtime scratch registers.
* Not supported: recursion (the hardware stack holds return addresses
  only), pointers, structs, floating point, bank-1 RAM.

The full reference is in [docs/language.md](docs/language.md).

## Memory model

Every function owns a statically allocated *frame* (parameters, locals and
expression temporaries) in bank-0 RAM.  Frames are overlaid using the call
graph: a callee's frame starts after the frames of all of its callers, and
interrupt handlers are placed after everything reachable from `main`.  This
gives C-like semantics for local variables and arguments without a data
stack.  Two 128-byte banks exist on the device, but the compiler currently
uses bank 0 only (bank 1 is reachable through `MP1`/`IAR1` from inline
assembly).

## Assembler

Holtek syntax, case-insensitive; `[m]` for data memory, bare numbers for
immediates, `x.i` for bits, `low()`/`high()`/`offset`, `$`, `equ`, `org`,
`dc`/`dw`/`db`, `ds`, `include`, and `.section 'data'`/`.section 'code'`
with `db ?`-style RAM reservation.  All SFR names (`acc`, `pcl`, `tblp`,
`pa`, `intc0`, …) and bit names (`c`, `z`, `emi`, …) are predefined.  See
[examples/blink.asm](examples/blink.asm).

## Layout

```
src/isa.rs        instruction set: opcode table, encoder, decoder
src/device.rs     HT66F0185 memory map, SFR/bit names, interrupt vectors
src/asm/          assembler (lexer, expression evaluator, two-pass core)
src/disasm.rs     disassembler
src/hex.rs        program image, Intel HEX and binary I/O
src/sim.rs        core simulator
src/lang/         compiler: lexer, parser, AST, code generator, runtime library
src/main.rs       command-line front end
tests/            feature programs and randomized differential tests, all
                  executed in the simulator
examples/         blink, Time Base interrupt, UART echo, lookup tables
docs/             language reference and instruction-set notes
```

## Status and caveats

* The core (instruction semantics, encodings) is well covered by tests.
  Peripherals are **not** simulated; their registers behave like RAM in
  `htc run`.
* Generated code is straightforward accumulator code with a small peephole
  pass; it is compact but not heavily optimised.
* `TABRD` and `TABRDC` share one encoding (as in Holtek's assembler).

## License

`htc` is released under the MIT license (see `LICENSE` in this directory).

## Programming with an e-Link8 Lite (`host/elink8.py`)

`host/elink8.py` is a Python 3 script (no Rust needed) for Holtek's e-Link8
Lite / e-Link / e-Writer programming dongles.  Holtek does not publish the
dongle's USB wire protocol, so the script has two layers:

* **`WCMD` layer (Windows, works today).** Drives `WCMD.exe`, the *DOS
  Command Mode* programmer that ships with HOPE3000 (the same package that
  programs the e-Link8 Lite).  `flash` runs download → erase → program →
  verify (→ lock); every WCMD command (`-T -D -U -P -V -B -E -L -R -W -C -K
  -S -A -CON`) is also exposed individually and as a library class.
  WCMD programs `.MTP` files (from HT-IDE3000 / HOPE3000 "Save") or `.MEM`
  EEPROM images; the `.MTP` container format is proprietary, so Intel HEX
  produced by `htc` has to be imported through HOPE3000 or HT-IDE3000 first.

  ```sh
  pip install pyusb                          # only for the raw layer
  python host/elink8.py flash firmware.MTP --lock --writer 1
  python host/elink8.py wcmd -- -T /W1       # any WCMD command
  ```

* **Raw USB layer (any OS).** Finds the dongle by its USB IDs (vendor
  `04D9`; application PID `801A`, bootloader PIDs `800E/8030/8032`, 8-bit
  family `800C 800D 8013 8014 8016 801A 802B` — from `e-link.ini`), dumps
  descriptors, sends/receives raw packets and replays packet scripts
  captured with USBPcap/Wireshark.  Use it to probe the device and to
  reverse-engineer the protocol; there are no protocol commands built in.

  ```sh
  python host/elink8.py probe
  python host/elink8.py raw --pid 0x801a --send "01 00 00" --read 64
  python host/elink8.py replay capture.txt
  ```
