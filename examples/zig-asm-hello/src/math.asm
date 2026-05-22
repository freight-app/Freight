section .text

; ── int64_t asm_gcd(int64_t a, int64_t b) ────────────────────────────────────
; Euclidean GCD via signed division (idiv).
; Uses the standard SysV x86-64 ABI: rdi=a, rsi=b, return rax.
global asm_gcd
asm_gcd:
        test    rsi, rsi
        jz      .done
.loop:
        mov     rax, rdi
        cqo                     ; sign-extend rax into rdx:rax
        idiv    rsi             ; rdx = rdi % rsi
        mov     rdi, rsi
        mov     rsi, rdx
        test    rsi, rsi
        jnz     .loop
.done:
        mov     rax, rdi
        ret

; ── uint64_t asm_popcount(uint64_t n) ────────────────────────────────────────
; Count set bits using the POPCNT hardware instruction (SSE4.2).
global asm_popcount
asm_popcount:
        popcnt  rax, rdi
        ret

; ── uint64_t asm_bswap64(uint64_t n) ─────────────────────────────────────────
; Reverse byte order (big-endian ↔ little-endian) using BSWAP.
global asm_bswap64
asm_bswap64:
        mov     rax, rdi
        bswap   rax
        ret

; ── uint64_t asm_next_pow2(uint64_t n) ───────────────────────────────────────
; Round up to the next power of two using BSR (bit scan reverse).
; n=0 returns 1. n already a power of two returns n.
global asm_next_pow2
asm_next_pow2:
        test    rdi, rdi
        jz      .zero
        lea     rax, [rdi - 1]
        test    rax, rax
        jz      .one            ; input was 1 → already pow2
        bsr     rcx, rax        ; index of highest set bit of (n-1)
        mov     rax, 2
        shl     rax, cl         ; 2 << bit_index
        ret
.zero:
.one:
        mov     rax, 1
        ret
