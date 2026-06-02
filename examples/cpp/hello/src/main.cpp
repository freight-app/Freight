#include <iostream>
#include <vector>
#include <cmath>
#include "stats.hpp"

int main() {
    std::vector<double> data = {2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0};

    // double m = mean(data);
    // double v = variance(data);

    std::pair tada(mean(data), variance(data));

    auto [m,v] = tada;

    std::cout << "data:     ";
    for (double x : data) std::cout << x << " ";
    std::cout << "\n";
    std::cout << "mean:     " << m << "\n";
    std::cout << "variance: " << v << "\n";
    std::cout << "std dev:  " << std::sqrt(v) << "\n";

    return 0;
}
