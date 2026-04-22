#pragma once
#include <stddef.h>

/* Sieve of Eratosthenes. Fills `out` with the first `count` primes.
   Returns the number of primes written (may be < count if limit is low). */
size_t sieve_primes(int *out, size_t count, int limit);
