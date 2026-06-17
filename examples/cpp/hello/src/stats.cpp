#include "stats.hpp"

#include <numeric>

double mean(std::span<const double> values) {
    return std::reduce(values.begin(), values.end()) / values.size();
}

double variance(std::span<const double> values) {
    double m = mean(values);
    double sum = 0.0;
    for (double v : values) {
        double d = v - m;
        sum += d * d;
    }
    return sum / values.size();
}
