/**
 * @file main.cpp
 * @brief Entry point for the doc-example demo program.
 *
 * Exercises mathlib and stats to verify that the functions work
 * together end-to-end.
 */

#include "mathlib.h"
#include "stats.h"
#include <cstdio>
#include <vector>

/// Print a labelled double value to stdout.
/// @param label  Human-readable name for the value.
/// @param v      The value to print.
static void show(const char *label, double v) {
  printf("  %-20s %g\n", label, v);
}

int main() {
  printf("=== mathlib ===\n");
  for (unsigned int n = 0; n <= 5; ++n)
    printf("  %u! = %llu\n", n, factorial(n));

  printf("  gcd(48, 18) = %u\n", gcd(48, 18));
  printf("  clamp(3.5, 0, 1) = %g\n", clamp(3.5, 0.0, 1.0));

  Vec2 a = {3.0, 4.0};
  printf("  ||(3,4)|| = %g\n", vec2_length(a));

  printf("\n=== stats ===\n");
  std::vector<double> xs = {2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0};
  std::vector<double> ys = {1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0};
  show("mean:", stats::mean(xs));
  show("stddev:", stats::stddev(xs));
  auto [slope, intercept] = stats::algo::linreg(xs, ys);
  show("linreg slope:", slope);
  show("linreg intercept:", intercept);

  return 0;
}
