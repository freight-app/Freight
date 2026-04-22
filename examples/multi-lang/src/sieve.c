#include "sieve.h"
#include <stdbool.h>
#include <stdlib.h>
#include <string.h>

size_t sieve_primes(int *out, size_t count, int limit) {
    bool *composite = calloc((size_t)(limit + 1), sizeof(bool));
    if (!composite) return 0;

    size_t found = 0;
    for (int i = 2; i <= limit && found < count; i++) {
        if (!composite[i]) {
            out[found++] = i;
            for (int j = 2 * i; j <= limit; j += i)
                composite[j] = true;
        }
    }

    free(composite);
    return found;
}
