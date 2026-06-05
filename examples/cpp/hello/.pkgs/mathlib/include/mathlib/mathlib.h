#pragma once

#ifdef __cplusplus
extern "C" {
#endif

/** Clamp `v` to `[lo, hi]`. */
double ml_clamp(double v, double lo, double hi);

/** Linear interpolation: `a + t*(b-a)`. */
double ml_lerp(double a, double b, double t);

/** Arithmetic mean of `n` values. */
double ml_mean(const double *xs, int n);

/** Population standard deviation of `n` values. */
double ml_stddev(const double *xs, int n);

/** Greatest common divisor (non-negative integers). */
long   ml_gcd(long a, long b);

/** Least common multiple (non-negative integers). Returns 0 on overflow. */
long   ml_lcm(long a, long b);

#ifdef __cplusplus
}
#endif
