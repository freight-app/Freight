#include "stats.h"
#include <math.h>
#include <stdlib.h>
#include <string.h>

double stats_mean(const double *xs, size_t n) {
    double s = 0;
    for (size_t i = 0; i < n; i++) s += xs[i];
    return s / (double)n;
}

double stats_variance(const double *xs, size_t n) {
    double m = stats_mean(xs, n), s = 0;
    for (size_t i = 0; i < n; i++) { double d = xs[i] - m; s += d * d; }
    return s / (double)n;
}

double stats_stddev(const double *xs, size_t n) {
    return sqrt(stats_variance(xs, n));
}

static int cmp_double(const void *a, const void *b) {
    double x = *(const double *)a, y = *(const double *)b;
    return (x > y) - (x < y);
}

double stats_median(double *xs, size_t n) {
    qsort(xs, n, sizeof(double), cmp_double);
    return (n % 2) ? xs[n / 2] : (xs[n/2 - 1] + xs[n/2]) / 2.0;
}
