#include "stats.hpp"
#include <cstdio>
#include <vector>

int main() {
    std::vector<double> data;
    data.reserve(10000);
    for (int i = 0; i < 10000; ++i) {
        data.push_back(static_cast<double>(i % 97));
    }

    double checksum = 0.0;
    for (int run = 0; run < 1000; ++run) {
        checksum += mean(data);
        checksum += variance(data);
    }

    std::printf("stats checksum: %.3f\n", checksum);
    return checksum > 0.0 ? 0 : 1;
}
