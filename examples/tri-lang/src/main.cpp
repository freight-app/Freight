#include <cstdio>
#include <cmath>
#include <vector>
#include <numeric>

extern "C" {
#include "timer.h"
// Fortran subroutine exported via bind(C, name="gravity")
void gravity(int n,
             const double *x, const double *y, const double *mass,
             double *fx, double *fy);
}

int main() {
    constexpr int N = 200;

    // Place particles on a 2D grid, all with unit mass.
    std::vector<double> x(N), y(N), mass(N, 1.0), fx(N), fy(N);
    for (int i = 0; i < N; i++) {
        x[i] = static_cast<double>(i % 20);
        y[i] = static_cast<double>(i / 20);
    }

    struct timespec t;
    timer_start(&t);

    gravity(N, x.data(), y.data(), mass.data(), fx.data(), fy.data());

    double ms = timer_elapsed_ms(&t);

    // Compute total force magnitude as a sanity check (should be near zero by symmetry).
    double sum_fx = std::accumulate(fx.begin(), fx.end(), 0.0);
    double sum_fy = std::accumulate(fy.begin(), fy.end(), 0.0);

    std::printf("N-body gravity  (%d particles)\n", N);
    std::printf("  time:         %.3f ms\n", ms);
    std::printf("  sum(fx):      %+.6e  (should be ~0 by symmetry)\n", sum_fx);
    std::printf("  sum(fy):      %+.6e  (should be ~0 by symmetry)\n", sum_fy);
    std::printf("  particle[0]:  fx=%+.6f  fy=%+.6f\n", fx[0], fy[0]);
    std::printf("  particle[N/2]: fx=%+.6f  fy=%+.6f\n", fx[N/2], fy[N/2]);

    return 0;
}
