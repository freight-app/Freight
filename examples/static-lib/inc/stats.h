#pragma once
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

double stats_mean(const double *xs, size_t n);
double stats_variance(const double *xs, size_t n);
double stats_stddev(const double *xs, size_t n);
double stats_median(double *xs, size_t n); /* sorts xs in place */

#ifdef __cplusplus
}
#endif
