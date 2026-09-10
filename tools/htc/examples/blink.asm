; Blink PA0 in plain Holtek assembly (HT-IDE compatible syntax).
;
;   htc asm examples/blink.asm -o blink.hex --lst blink.lst

        .section 'data'
count_lo    db ?
count_hi    db ?

        .section 'code'
        org 000h
        jmp start

        org 030h
start:
        mov a, 0abh             ; watchdog off (WE4..WE0 = 10101b)
        mov wdtc, a
        mov a, 0feh
        mov pac, a              ; PA0 output
        clr pa
loop:
        mov a, pa
        xor a, 01h
        mov pa, a               ; toggle PA0
        call delay
        jmp loop

delay:
        clr count_lo
        mov a, 40h
        mov count_hi, a
delay_1:
        sdz count_lo
        jmp delay_1
        sdz count_hi
        jmp delay_1
        ret
