/**
 * @file mathlib.c
 * @brief Implementations for mathlib — C source.
 */

#include "mathlib.h"
#include <math.h>

/**
 * @brief Compute the factorial of n.
 *
 * Iterative implementation to avoid stack overflow for large n.
 *
 * @param n  Non-negative integer (≤ 20).
 * @return   n!
 */
unsigned long long factorial(unsigned int n) {
    unsigned long long r = 1;
    for (unsigned int i = 2; i <= n; ++i) r *= i;
    return r;
}

/*!
 * Clamp value v to the closed interval [lo, hi].
 */
double clamp(double v, double lo, double hi) {
    if (v < lo) return lo;
    if (v > hi) return hi;
    return v;
}

/// Iterative GCD via Euclidean algorithm.
unsigned int gcd(unsigned int a, unsigned int b) {
    while (b) { unsigned int t = b; b = a % b; a = t; }
    return a;
}

/// Euclidean norm of a 2-D vector.
double vec2_length(Vec2 v) {
    return sqrt(v.x * v.x + v.y * v.y);
}

/// Component-wise addition of two Vec2 values.
Vec2 vec2_add(Vec2 a, Vec2 b) {
    return (Vec2){ a.x + b.x, a.y + b.y };
}
