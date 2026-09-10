//! Runtime support routines emitted (on demand) as assembly text.
//!
//! Calling convention: arguments in the scratch bytes `__r0..__r5`, results
//! in ACC (8-bit) or `__r4:__r5` (16-bit, little endian).  The routines are
//! not re-entrant; an interrupt handler whose call tree uses them saves and
//! restores the scratch bytes on entry/exit.

/// `__mul8`: ACC = __r0 * __r1 (low 8 bits).  Clobbers __r2.
pub const MUL8: &str = "
__mul8:
        clr [__r2]
__mul8_loop:
        sz [__r1]
        jmp __mul8_body
        mov a,[__r2]
        ret
__mul8_body:
        clr c
        rrc [__r1]
        snz c
        jmp __mul8_skip
        mov a,[__r0]
        addm a,[__r2]
__mul8_skip:
        mov a,[__r0]
        add a,[__r0]
        mov [__r0],a
        jmp __mul8_loop
";

/// `__mul16`: (__r5:__r4) = (__r1:__r0) * (__r3:__r2), low 16 bits.
pub const MUL16: &str = "
__mul16:
        clr [__r4]
        clr [__r5]
__mul16_loop:
        mov a,[__r2]
        or a,[__r3]
        snz z
        jmp __mul16_body
        ret
__mul16_body:
        clr c
        rrc [__r3]
        rrc [__r2]
        snz c
        jmp __mul16_skip
        mov a,[__r0]
        addm a,[__r4]
        mov a,[__r1]
        adcm a,[__r5]
__mul16_skip:
        clr c
        rlc [__r0]
        rlc [__r1]
        jmp __mul16_loop
";

/// `__divu8`: ACC = __r0 / __r1, __r2 = __r0 % __r1.  Clobbers __r3.
/// Division by zero yields quotient 0FFh and remainder __r0.
pub const DIVU8: &str = "
__divu8:
        clr [__r2]
        mov a,8
        mov [__r3],a
__divu8_loop:
        clr c
        rlc [__r0]
        rlc [__r2]
        mov a,[__r2]
        sub a,[__r1]
        snz c
        jmp __divu8_next
        mov [__r2],a
        set [__r0].0
__divu8_next:
        sdz [__r3]
        jmp __divu8_loop
        mov a,[__r0]
        ret
";

/// `__divu16`: (__r5:__r4) = (__r1:__r0) / (__r3:__r2); remainder in (__r7:__r6).
/// Clobbers __r0..__r8.
pub const DIVU16: &str = "
__divu16:
        clr [__r6]
        clr [__r7]
        mov a,16
        mov [__r8],a
__divu16_loop:
        clr c
        rlc [__r0]
        rlc [__r1]
        rlc [__r6]
        rlc [__r7]
        mov a,[__r6]
        sub a,[__r2]
        mov [__r4],a
        mov a,[__r7]
        sbc a,[__r3]
        snz c
        jmp __divu16_next
        mov [__r7],a
        mov a,[__r4]
        mov [__r6],a
        set [__r0].0
__divu16_next:
        sdz [__r8]
        jmp __divu16_loop
        mov a,[__r0]
        mov [__r4],a
        mov a,[__r1]
        mov [__r5],a
        ret
";

/// `__divs8`: signed ACC = __r0 / __r1, __r2 = remainder (sign of dividend).
pub const DIVS8: &str = "
__divs8:
        clr [__r4]
        snz [__r0].7
        jmp __divs8_b
        cpl [__r0]
        inc [__r0]
        set [__r4].0
        set [__r4].1
__divs8_b:
        snz [__r1].7
        jmp __divs8_go
        cpl [__r1]
        inc [__r1]
        mov a,1
        xorm a,[__r4]
__divs8_go:
        call __divu8
        snz [__r4].0
        jmp __divs8_rem
        cpla acc
        add a,1
__divs8_rem:
        mov [__r5],a
        snz [__r4].1
        jmp __divs8_done
        cpl [__r2]
        inc [__r2]
__divs8_done:
        mov a,[__r5]
        ret
";

/// `__divs16`: signed (__r5:__r4) = (__r1:__r0) / (__r3:__r2); remainder (__r7:__r6).
pub const DIVS16: &str = "
__divs16:
        clr [__r9]
        snz [__r1].7
        jmp __divs16_b
        cpl [__r0]
        cpl [__r1]
        inc [__r0]
        sz z
        inc [__r1]
        set [__r9].0
        set [__r9].1
__divs16_b:
        snz [__r3].7
        jmp __divs16_go
        cpl [__r2]
        cpl [__r3]
        inc [__r2]
        sz z
        inc [__r3]
        mov a,1
        xorm a,[__r9]
__divs16_go:
        call __divu16
        snz [__r9].0
        jmp __divs16_rem
        cpl [__r4]
        cpl [__r5]
        inc [__r4]
        sz z
        inc [__r5]
__divs16_rem:
        snz [__r9].1
        jmp __divs16_done
        cpl [__r6]
        cpl [__r7]
        inc [__r6]
        sz z
        inc [__r7]
__divs16_done:
        ret
";

/// Number of scratch bytes each routine needs (`__r0..__r{n-1}`).
pub fn scratch_bytes(routine: &str) -> usize {
    match routine {
        "__mul8" => 3,
        "__mul16" => 6,
        "__divu8" => 4,
        "__divu16" => 9,
        "__divs8" => 6,
        "__divs16" => 10,
        _ => 0,
    }
}

/// Source text of a routine plus the routines it depends on.
pub fn routine_source(name: &str) -> Vec<(&'static str, &'static str)> {
    match name {
        "__mul8" => vec![("__mul8", MUL8)],
        "__mul16" => vec![("__mul16", MUL16)],
        "__divu8" => vec![("__divu8", DIVU8)],
        "__divu16" => vec![("__divu16", DIVU16)],
        "__divs8" => vec![("__divs8", DIVS8), ("__divu8", DIVU8)],
        "__divs16" => vec![("__divs16", DIVS16), ("__divu16", DIVU16)],
        _ => vec![],
    }
}
