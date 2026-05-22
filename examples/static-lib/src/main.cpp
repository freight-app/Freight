#include "stats.h"
#include <cstdio>
#include <vector>
#include <algorithm>
#include <numeric>

int main() {
    std::vector<double> data = { 4, 8, 15, 16, 23, 42, 3, 7, 1, 9 };

    double mean = stats_mean(data.data(), data.size());
    double sd   = stats_stddev(data.data(), data.size());

    std::vector<double> copy = data;
    double med = stats_median(copy.data(), copy.size());

    std::printf("data:   ");
    for (double x : data) std::printf("%.0f ", x);
    std::printf("\n");
    std::printf("mean:   %.4f\n", mean);
    std::printf("stddev: %.4f\n", sd);
    std::printf("median: %.1f\n", med);
}
