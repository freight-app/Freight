#include <iostream>
#include <vector>
#include <stdexcept>

// Scenario 1: null pointer dereference
static void null_deref() {
    int* p = nullptr;
    std::cout << "value = " << *p << "\n";   // crash here
}

// Scenario 2: out-of-bounds vector access (UB, may or may not crash)
static void oob_access() {
    std::vector<int> v = {1, 2, 3};
    std::cout << "element = " << v[100] << "\n";  // undefined behaviour
}

// Scenario 3: explicit abort
static void explicit_abort() {
    throw std::runtime_error("something went wrong at runtime");
}

int main(int argc, char**) {
    // Change the argument to 2 or 3 to trigger the other scenarios.
    switch (argc) {
        case 1: null_deref();    break;
        case 2: oob_access();    break;
        case 3: explicit_abort(); break;
    }
    return 0;
}
