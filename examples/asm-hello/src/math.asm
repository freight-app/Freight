; Simple NASM x86-64 routines called from C.
; Follows the System V AMD64 ABI: first arg in rdi, second in rsi, return in rax.

section .text

global asm_add
global asm_max

; long asm_add(long a, long b)
asm_add:
    mov  rax, rdi
    add  rax, rsi
    ret

; long asm_max(long a, long b)
asm_max:
    mov  rax, rdi
    cmp  rdi, rsi
    cmovl rax, rsi
    ret
