#include <stdio.h>
#include <stdint.h>
#include <string.h>

/* Print build-time architecture info injected by zig cc. */
static void print_target(void) {
#if defined(__aarch64__)
    printf("arch: aarch64\n");
#elif defined(__x86_64__)
    printf("arch: x86_64\n");
#elif defined(__riscv) && __riscv_xlen == 64
    printf("arch: riscv64\n");
#elif defined(__arm__)
    printf("arch: arm\n");
#else
    printf("arch: unknown\n");
#endif

#if defined(__linux__)
    printf("os:   linux\n");
#elif defined(_WIN32)
    printf("os:   windows\n");
#elif defined(__APPLE__)
    printf("os:   macos\n");
#else
    printf("os:   unknown\n");
#endif

    printf("ptr:  %zu bytes\n", sizeof(void *));
}

/* Simple checksum — same result regardless of endianness/arch. */
static uint32_t checksum(const char *s) {
    uint32_t h = 2166136261u;
    while (*s) {
        h ^= (uint8_t)*s++;
        h *= 16777619u;
    }
    return h;
}

int main(void) {
    printf("zigcc-cross example\n");
    printf("===================\n");
    print_target();

    const char *words[] = { "freight", "zig", "cross-compile", NULL };
    printf("\nFNV-1a checksums:\n");
    for (int i = 0; words[i]; i++) {
        printf("  %-16s  0x%08x\n", words[i], checksum(words[i]));
    }
    return 0;
}
