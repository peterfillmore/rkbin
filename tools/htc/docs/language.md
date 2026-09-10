# The `htc` language reference

`htc` is a small, C-flavoured language designed for the Holtek HT66F0185's
accumulator architecture.  If you know C you know most of it; the
differences are listed at the end.

## Lexical structure

* Comments: `// line` and `/* block */`.
* Integer literals: decimal `123`, hexadecimal `0x7F` or `7Fh`, binary
  `0b1010`, octal `017`, character `'A'` (with `\n \r \t \0 \\ \'` escapes).
  Underscores may separate digits (`1_000`).
* String literals are only allowed as initialisers of `u8` arrays and
  include a terminating `0`.
* Identifiers are case-sensitive.  Upper-case names of special function
  registers (`PA`, `PAC`, `INTC0`, …) and register bits (`C`, `Z`, `EMI`,
  `TB0E`, …) are predefined; run `htc regs` for the list.

## Types

| type             | size | range              |
|------------------|------|--------------------|
| `bool`           | 1    | `false`/`true` (stored as 0/1) |
| `u8` (`uint8_t`, `char`, `unsigned`) | 1 | 0..255 |
| `i8` (`int8_t`)  | 1    | −128..127          |
| `u16` (`uint16_t`)| 2   | 0..65535           |
| `i16` (`int16_t`, `int`, `short`) | 2 | −32768..32767 |
| `void`           | –    | function results only |

16-bit values are stored little-endian.  Arrays hold up to 256 elements of
any scalar type.  There are no pointers, structs or floating point.

### Type rules

* A binary arithmetic or bitwise operation is evaluated in the *common type*
  of its operands: the wider size (8 or 16 bits), signed if either operand
  is signed.  There is no promotion to `int`: `u8 + u8` wraps at 256.
* Shifts have the type of the left operand; the count is taken modulo 256
  and counts of 8 (16) or more shift everything out (sign-filling for
  signed right shifts).
* Comparisons and logical operators yield `bool`.
* Constant expressions are folded at compile time using 64-bit arithmetic;
  a folded constant gets the narrowest type that fits, or the type of an
  enclosing cast.
* Conversions between integer types truncate or zero/sign-extend.
  Converting to `bool` yields 0 or 1.
* Division truncates toward zero; the remainder takes the dividend's sign.
  Division by zero yields an unspecified result (constant divisors of zero
  are compile errors).

## Declarations

```c
u8 x;                        // global, zero-initialised at start-up
u16 total = 1234;            // constant initialiser, applied at start-up
u8 buf[8] = {1, 2, 3};       // remaining elements are zero
u8 msg[] = "hi";             // u8 array of length 3 (includes the 0)
u8 mirror @ 0xF0;            // absolute placement, no allocation
const u8 N = 8;              // compile-time constant, no storage
const u8 TABLE[] = {…};      // table in program memory, read with TABRD
const u16 TABLE16[] = {…};   // one program word per element
static u8 y;                 // `static` is accepted and ignored
```

Globals are allocated in declaration order from data address `80h`.
Locals may be declared anywhere inside a block (also in the `for` header)
and are scoped to that block.  Local `const` scalars are allowed; `const`
arrays must be global.

Variables placed with `@` are not zero-initialised unless they have an
initialiser.  Choose addresses above the compiler-allocated region (see the
frame report printed by `htc build`) or you will overlap it — the compiler
warns when it can tell.

## Functions

```c
u8 add(u8 a, u8 b) { return a + b; }
void nothing(void) { }
u16 wide() { return 40000; }
```

* Parameters and locals live in a static per-function frame; frames are
  overlaid according to the call graph, so **recursion is rejected**.
* 8-bit results are returned in ACC, 16-bit results through a hidden
  `__ret0/__ret1` pair.
* `main` is required; it is called from the start-up code, and if it
  returns the device executes `halt`.
* The hardware stack has 8 levels.  The compiler warns if the static call
  depth (including interrupt handlers and runtime routines) may exceed it.

### Interrupt handlers

```c
interrupt TB0 void every_tick() { ticks++; }
interrupt(0x04) void ext0() { … }
```

Vector names: `INT0 CMP MF0 MF1 MF2 ADC TB0 TB1 INT1 SIM UART`.  A handler
takes no parameters, returns `void`, cannot be called directly and ends
with `RETI`.  The prologue/epilogue save and restore `ACC`, `STATUS`, and
(only when the handler's call tree needs them) `MP0`, `TBLP`/`TBHP` and the
runtime-library scratch registers.  Handlers get RAM frames disjoint from
everything `main` can reach.  A function called both from `main` and from
an interrupt handler is not re-entrant; the compiler warns about it.

Enabling an interrupt source is the program's job (e.g. `TB0E = 1; ei();`).

## Statements

```c
if (cond) stmt else stmt
while (cond) stmt
do stmt while (cond);
for (init; cond; step) stmt        // init may declare a variable
switch (expr) { case 1: … break; case 2: case 3: … default: … }
break; continue; return; return expr;
{ … }                               // block with its own scope
asm { … }                           // inline assembly, see below
expr;
```

`switch` accepts 8- or 16-bit subjects and constant case labels; cases
fall through unless terminated by `break`, like C.

## Expressions

Operators, from highest to lowest precedence:

| level | operators                                   |
|-------|---------------------------------------------|
| 1     | `f(args)`  `a[i]`  `x.n` (bit)  `x++` `x--` |
| 2     | `-x` `~x` `!x` `++x` `--x` `(type)x`        |
| 3     | `* / %`                                     |
| 4     | `+ -`                                       |
| 5     | `<< >>`                                     |
| 6     | `< <= > >=`                                 |
| 7     | `== !=`                                     |
| 8     | `&`                                         |
| 9     | `^`                                         |
| 10    | `\|`                                        |
| 11    | `&&`                                        |
| 12    | `\|\|`                                      |
| 13    | `c ? a : b`                                 |
| 14    | `= += -= *= /= %= &= \|= ^= <<= >>=`        |

`&&` and `||` short-circuit.  Assignments are expressions (`a = b = 0`).

### Bit access

`x.n` (n = 0..7) reads or writes bit `n` of any 8-bit variable, array
element or special function register:

```c
PA.3 = 1;                 // SET [14h].3
if (!PB.0) …              // SNZ/SZ
flags.7 = a < b;          // CLR then conditional SET
u8 v = PA.3;              // 0 or 1
```

Named bits (`EMI`, `C`, `Z`, `TB0E`, …) behave the same way.

### Arrays and tables

`a[i]` with a constant index is addressed directly; a variable index uses
`MP0`/`IAR0`.  Indexing is not bounds-checked at run time (constant indices
are checked at compile time).  Elements of `const` tables are read from
program memory with `TABRD`; a `const u8` table stores one element per
16-bit program word.

## Intrinsics

| call                    | effect                    |
|-------------------------|---------------------------|
| `halt()`                | `HALT` (enter sleep/idle) |
| `nop()`                 | `NOP`                     |
| `clr_wdt()`             | `CLR WDT`                 |
| `ei()` / `enable_interrupts()`  | `SET INTC0.0` (EMI) |
| `di()` / `disable_interrupts()` | `CLR INTC0.0`       |

## Inline assembly

```c
asm {
    mov a,[_count]        ; globals are visible as _name
    add a,[F3+1]          ; frames are F<n>, temporaries T<n> (see --asm output)
    mov pa,a
}
```

The text is passed to the assembler verbatim (one instruction or label per
line).  ACC, STATUS, MP0, TBLP/TBHP may be clobbered freely; other frames'
data must not be modified.

## Generated code and memory

`htc build --asm out.asm` writes the intermediate assembly.  The layout of
data memory is:

```
80h..     globals (declaration order)
          runtime scratch (__r0.., only if * / % are used)
          16-bit return slot (__ret0/1, only if needed)
          interrupt save slots (__sv_*)
          function frames: F<n> = base, T<n> = F<n> + locals
```

`htc build` prints the total RAM use and every frame.  Bank 1 (`80h..FFh`
with `BP.0 = 1`) is left to inline assembly.

## Differences from C

* No `int` promotion: arithmetic is done in the operand width (max 16 bits).
* No pointers, structs, unions, enums, typedefs, floating point, `goto`,
  `sizeof`, preprocessor or multiple translation units.
* No recursion; local variables are static and functions are not re-entrant.
* `bool` is a real type; `.n` bit access is an extension.
* Strings exist only as `u8` array initialisers.
