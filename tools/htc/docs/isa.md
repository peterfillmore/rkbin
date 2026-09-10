# HT66F0185 instruction set and encoding

The HT66F0185 executes a 63-instruction Holtek 8-bit RISC core with a
16-bit instruction word, 4K × 16 program memory (12-bit program counter),
256 × 8 data memory in two banks, an 8-level hardware stack and a
memory-mapped accumulator (`ACC` at data address `05h`).

## Sources

Holtek's datasheets document mnemonics and semantics but not the binary
encoding.  The encoding implemented in `src/isa.rs` comes from:

1. the HT-IDE3000 cross-assembler description file (`*.fmt`) format, whose
   `%mnemonic` table lists `magic, mask, mnemonic` per instruction and whose
   `%operand` table describes how operand bits are packed; and
2. `HGASM` listing files (`*.LST`) produced by Holtek's assembler for real
   projects, used to confirm individual encodings such as
   `sz z` → `3D0A`, `inc tblp` → `1487`, `tabrd mp0` → `1D01`,
   `orm a,[32]` → `05A0`, `clr wdt2` → `0005`.

Both `tabrd` and `tabrdc` assemble to `1Dxx` in Holtek's tools; `tabrdl` is
`1D80 | m`.

## Operand packing

| class            | fixed-bit mask | operand bits                                              |
|------------------|----------------|-----------------------------------------------------------|
| none             | `FFFF`         | –                                                         |
| `[m]` data memory| `BF80`         | `m[6:0]` → bits 6..0, `m[7]` → bit 14                     |
| `x` immediate    | `FF00`         | `x[7:0]` → bits 7..0                                      |
| `addr` program   | `3800`         | `addr[10:0]` → bits 10..0, `addr[11]` → bit 14            |
| `[m].i` bit      | `BC00`         | `m[6:0]` → bits 6..0, `i` → bits 9..7, `m[7]` → bit 14    |

Data addresses `80h..FFh` therefore set bit 14; e.g. `mov [80h],a` is
`4080` and `clr [85h]` is `5F05`.

## Opcode table

| word (base) | mnemonic          | cycles | flags        |
|-------------|-------------------|--------|--------------|
| `0000`      | `nop`             | 1      |              |
| `0001`      | `clr wdt` / `clr wdt1` | 1 | TO, PDF     |
| `0005`      | `clr wdt2`        | 1      | TO, PDF      |
| `0002`      | `halt`            | 1      | TO, PDF      |
| `0003`      | `ret`             | 2      |              |
| `0004`      | `reti`            | 2      |              |
| `0080`      | `mov [m],a`       | 1      |              |
| `0100`      | `cpla [m]`        | 1      | Z            |
| `0180`      | `cpl [m]`         | 1      | Z            |
| `0200`      | `sub a,[m]`       | 1      | Z C AC OV    |
| `0280`      | `subm a,[m]`      | 1      | Z C AC OV    |
| `0300`      | `add a,[m]`       | 1      | Z C AC OV    |
| `0380`      | `addm a,[m]`      | 1      | Z C AC OV    |
| `0400`      | `xor a,[m]`       | 1      | Z            |
| `0480`      | `xorm a,[m]`      | 1      | Z            |
| `0500`      | `or a,[m]`        | 1      | Z            |
| `0580`      | `orm a,[m]`       | 1      | Z            |
| `0600`      | `and a,[m]`       | 1      | Z            |
| `0680`      | `andm a,[m]`      | 1      | Z            |
| `0700`      | `mov a,[m]`       | 1      |              |
| `0900`      | `ret a,x`         | 2      |              |
| `0A00`      | `sub a,x`         | 1      | Z C AC OV    |
| `0B00`      | `add a,x`         | 1      | Z C AC OV    |
| `0C00`      | `xor a,x`         | 1      | Z            |
| `0D00`      | `or a,x`          | 1      | Z            |
| `0E00`      | `and a,x`         | 1      | Z            |
| `0F00`      | `mov a,x`         | 1      |              |
| `1000`      | `sza [m]`         | 1 (+1) |              |
| `1080`      | `sz [m]`          | 1 (+1) |              |
| `1100`      | `swapa [m]`       | 1      |              |
| `1180`      | `swap [m]`        | 1      |              |
| `1200`      | `sbc a,[m]`       | 1      | Z C AC OV    |
| `1280`      | `sbcm a,[m]`      | 1      | Z C AC OV    |
| `1300`      | `adc a,[m]`       | 1      | Z C AC OV    |
| `1380`      | `adcm a,[m]`      | 1      | Z C AC OV    |
| `1400`      | `inca [m]`        | 1      | Z            |
| `1480`      | `inc [m]`         | 1      | Z            |
| `1500`      | `deca [m]`        | 1      | Z            |
| `1580`      | `dec [m]`         | 1      | Z            |
| `1600`      | `siza [m]`        | 1 (+1) |              |
| `1680`      | `siz [m]`         | 1 (+1) |              |
| `1700`      | `sdza [m]`        | 1 (+1) |              |
| `1780`      | `sdz [m]`         | 1 (+1) |              |
| `1800`      | `rla [m]`         | 1      |              |
| `1880`      | `rl [m]`          | 1      |              |
| `1900`      | `rra [m]`         | 1      |              |
| `1980`      | `rr [m]`          | 1      |              |
| `1A00`      | `rlca [m]`        | 1      | C            |
| `1A80`      | `rlc [m]`         | 1      | C            |
| `1B00`      | `rrca [m]`        | 1      | C            |
| `1B80`      | `rrc [m]`         | 1      | C            |
| `1D00`      | `tabrd [m]` / `tabrdc [m]` | 2 |           |
| `1D80`      | `tabrdl [m]`      | 2      |              |
| `1E80`      | `daa [m]`         | 1      | C            |
| `1F00`      | `clr [m]`         | 1      |              |
| `1F80`      | `set [m]`         | 1      |              |
| `2000`      | `call addr`       | 2      |              |
| `2800`      | `jmp addr`        | 2      |              |
| `3000`      | `set [m].i`       | 1      |              |
| `3400`      | `clr [m].i`       | 1      |              |
| `3800`      | `snz [m].i`       | 1 (+1) |              |
| `3C00`      | `sz [m].i`        | 1 (+1) |              |

Skip instructions take one extra cycle when the skip is taken; any
instruction that writes `PCL` takes one extra cycle.

## Semantics worth knowing

* `C` is a *carry* for addition and a *no-borrow* flag for subtraction:
  after `sub a,[m]`, `C = 1` means `ACC ≥ [m]` (unsigned).
* `sbc a,[m]` computes `ACC − [m] − !C`.
* `OV` is the signed overflow flag; `AC` the nibble carry (used by `daa`).
* `sz [m]` skips when the byte is zero; `sz [m].i` / `snz [m].i` test a bit.
  There is no byte-wide `snz`.
* `tabrd [m]` reads the program word at `TBHP:TBLP`, `tabrdl [m]` the word at
  `0Fxx` (last page); the low byte goes to `[m]`, the high byte to `TBLH`.
* Direct addressing always reaches bank 0.  `IAR0/MP0` are bank-0 only,
  `IAR1/MP1` follow `BP.0`.  The `EEC` register (`40h`) exists in bank 1 only.
* Writing `PCL` jumps within the current 256-word page.
* Special vectors: reset `000h`; interrupts `004h` INT0, `008h` comparator,
  `00Ch`/`010h`/`014h` multi-function 0/1/2, `018h` A/D, `01Ch` Time Base 0,
  `020h` Time Base 1, `024h` INT1, `028h` SIM, `02Ch` UART.
