#include "mathlib/mathlib.h"
#include <math.h>
#include <stddef.h>

double ml_clamp(double v, double lo, double hi) {
    return v < lo ? lo : v > hi ? hi : v;
}

double ml_lerp(double a, double b, double t) {
    return a + t * (b - a);
}

double ml_mean(const double *xs, int n) {
    if (n <= 0) return 0.0;
    double s = 0.0;
    for (int i = 0; i < n; i++) s += xs[i];
    return s / n;
}

double ml_stddev(const double *xs, int n) {
    if (n <= 1) return 0.0;
    double m = ml_mean(xs, n);
    double sq = 0.0;
    for (int i = 0; i < n; i++) { double d = xs[i] - m; sq += d * d; }
    return sqrt(sq / n);
}

long ml_gcd(long a, long b) {
    while (b) { long t = b; b = a % b; a = t; }
    return a < 0 ? -a : a;
}

long ml_lcm(long a, long b) {
    if (a == 0 || b == 0) return 0;
    long g = ml_gcd(a, b);
    long q = a / g;
    if (__builtin_mul_overflow(q, b, &q)) return 0;
    return q < 0 ? -q : q;
}
