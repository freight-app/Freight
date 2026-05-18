/**
 * @file mathlib.h
 * @brief Portable math utilities — Doxygen-style C header.
 *
 * Demonstrates the three most common Doxygen block styles:
 *   - Block comment with asterisk leader
 *   - Triple-slash line comments
 *   - Exclamation block comments
 */

#ifndef MATHLIB_H
#define MATHLIB_H

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

/**
 * @brief Compute the factorial of n.
 *
 * Returns 1 for n == 0 (by convention).
 *
 * @param n  Non-negative integer.
 * @return   n! as an unsigned 64-bit integer.
 * @warning  Overflows for n > 20.
 */
unsigned long long factorial(unsigned int n);

/*!
 * @brief Clamp a value to [lo, hi].
 *
 * @param v   Input value.
 * @param lo  Lower bound (inclusive).
 * @param hi  Upper bound (inclusive).
 * @return    v clamped to [lo, hi].
 */
double clamp(double v, double lo, double hi);

/// Greatest common divisor of two non-negative integers.
/// Uses the iterative Euclidean algorithm.
/// @param a  First operand.
/// @param b  Second operand.
/// @return   GCD(a, b).
unsigned int gcd(unsigned int a, unsigned int b);

/*!
 * @brief Linear interpolation between two values.
 *
 * @param a  Start value (t == 0).
 * @param b  End value (t == 1).
 * @param t  Interpolation parameter in [0, 1].
 * @return   a + t * (b - a)
 */
double lerp(double a, double b, double t);

/**
 * @brief Fixed-size 2-D vector.
 *
 * Coordinates are stored as 64-bit floats.
 */
typedef struct {
    double x; /**< X component. */
    double y; /**< Y component. */
} Vec2;

/// Compute the Euclidean length of v.
/// @param v  Input vector.
/// @return   ||v||₂
double vec2_length(Vec2 v);

/// Add two vectors component-wise.
/// @param a  Left operand.
/// @param b  Right operand.
/// @return   a + b
Vec2 vec2_add(Vec2 a, Vec2 b);

#ifdef __cplusplus
}
#endif

#endif /* MATHLIB_H */
