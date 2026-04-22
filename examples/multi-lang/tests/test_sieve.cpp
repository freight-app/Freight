#include <cassert>
#include <cstring>

extern "C" {
#include "sieve.h"
}

int main() {
    int out[10];

    // First 5 primes should be 2, 3, 5, 7, 11
    size_t n = sieve_primes(out, 5, 100);
    assert(n == 5);
    assert(out[0] == 2);
    assert(out[1] == 3);
    assert(out[2] == 5);
    assert(out[3] == 7);
    assert(out[4] == 11);

    // Low limit — only 2 fits
    n = sieve_primes(out, 10, 3);
    assert(n == 2);
    assert(out[0] == 2);
    assert(out[1] == 3);

    // count=0 should return 0
    n = sieve_primes(out, 0, 100);
    assert(n == 0);

    return 0;
}
