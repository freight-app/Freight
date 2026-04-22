#include <stdio.h>
#include <stdint.h>

/* Collatz conjecture: count steps to reach 1. */
static uint64_t collatz(uint64_t n) {
    uint64_t steps = 0;
    while (n != 1) {
        n = (n % 2 == 0) ? n / 2 : 3 * n + 1;
        steps++;
    }
    return steps;
}

int main(void) {
    uint64_t max_steps = 0;
    uint64_t max_n     = 0;

    for (uint64_t n = 1; n <= 1000000; n++) {
        uint64_t s = collatz(n);
        if (s > max_steps) {
            max_steps = s;
            max_n     = n;
        }
    }

    printf("Collatz: n=%llu reaches 1 in %llu steps\n",
           (unsigned long long)max_n,
           (unsigned long long)max_steps);
    return 0;
}
