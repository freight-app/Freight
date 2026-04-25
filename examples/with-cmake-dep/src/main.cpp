#include <iostream>
#include "mathlib.h"

int main() {
    std::cout << "add(3, 4)        = " << mathlib::add(3, 4)        << "\n";
    std::cout << "multiply(6, 7)   = " << mathlib::multiply(6, 7)   << "\n";
    std::cout << "sqrt_approx(2.0) = " << mathlib::sqrt_approx(2.0) << "\n";
    return 0;
}
