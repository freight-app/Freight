#include <iostream>
#include <vector>
#include <span>

extern "C" {
#include "sieve.h"
}

int main() {
    constexpr int N = 20;
    constexpr int LIMIT = 200;

    std::vector<int> primes(N);
    size_t found = sieve_primes(primes.data(), N, LIMIT);
    primes.resize(found);

    std::cout << "First " << found << " primes up to " << LIMIT << ":\n";
    for (int p : std::span(primes)) {
        std::cout << "  " << p << "\n";
    }

    return 0;
}
