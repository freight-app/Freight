#include "stats.hpp"
#include <cassert>
#include <cmath>
#include <vector>

int main() {
    std::vector<double> data = {2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0};

    double m = mean(data);
    assert(std::abs(m - 5.0) < 1e-9 && "mean should be 5.0");

    double v = variance(data);
    assert(std::abs(v - 4.0) < 1e-9 && "variance should be 4.0");

    double sd = std::sqrt(v);
    assert(std::abs(sd - 2.0) < 1e-9 && "std dev should be 2.0");

    return 0;
}
