#include <iostream>
#include <vector>
#include <cmath>
#include "stats.hpp"

#include "vecmath/vec2.h"

int main() {
    std::vector<double> data = {2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0};


    std::pair tada = std::pair(mean(data), variance(data));

    auto [m,v] = tada;

    throw std::runtime_error("yeet");

    std::cout << "data:     ";
    for (double x : data) std::cout << x << " ";
    std::cout << "\n";
    std::cout << "mean:     " << m << "\n";
    std::cout << "variance: " << v << "\n";
    std::cout << "std dev:  " << std::sqrt(v) << "\n";

    vm::Vec2 g(1,1);

    g = g * 10;

    std::cout << v << std::endl;
    std::cout << g.length() << std::endl;


    return 0;
}
