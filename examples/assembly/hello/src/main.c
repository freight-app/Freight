#include <stdio.h>

/* Implemented in src/math.asm */
long asm_add(long a, long b);
long asm_max(long a, long b);

int main(void) {
    printf("asm_add(3, 4)    = %ld\n", asm_add(3, 4));
    printf("asm_add(10, -2)  = %ld\n", asm_add(10, -2));
    printf("asm_max(7, 12)   = %ld\n", asm_max(7, 12));
    printf("asm_max(-5, -1)  = %ld\n", asm_max(-5, -1));
    return 0;
}
