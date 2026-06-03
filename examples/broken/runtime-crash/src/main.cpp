#include <cstdlib>
#include <iostream>
#include <stdexcept>
#include <vector>

// Scenario 1: explicit abort — always terminates with non-zero exit.
static void crash_abort() {
    std::cerr << "aborting intentionally\n";
    std::abort();
}

// Scenario 2: unhandled exception — terminates via std::terminate().
static void crash_exception() {
    throw std::runtime_error("unhandled runtime error");
}

// Scenario 3: out-of-bounds via .at() — throws std::out_of_range.
static void crash_oob() {
    std::vector<int> v = {1, 2, 3};
    std::cout << v.at(100) << "\n";
}

int main(int argc, char**) {
    // No args → abort (default for automated tests).
    // One arg  → unhandled exception.
    // Two args → out-of-bounds.
    switch (argc) {
        case 1: crash_abort();     break;
        case 2: crash_exception(); break;
        case 3: crash_oob();       break;
    }
    return 0;
}
