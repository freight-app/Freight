#include "mathlib.h"

namespace mathlib {

int add(int a, int b) { return a + b; }

int multiply(int a, int b) { return a * b; }

double sqrt_approx(double x) {
    if (x <= 0.0) return 0.0;
    double g = x / 2.0;
    for (int i = 0; i < 20; ++i)
        g = (g + x / g) / 2.0;
    return g;
}

}
