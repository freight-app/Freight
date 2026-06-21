#include "stats.hpp"

#include "vecmath/vec2.h"

#include <pthread.h>
#include <stdio.h>

#include <algorithm>
#include <cmath>
#include <iostream>
#include <map>
#include <optional>
#include <utility>
#include <vector>

int main() {
    std::vector<double> data = {2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0};

    // CTAD: std::pair deduced from constructor args
    std::pair tada = std::pair(mean(data), variance(data));

    // structured bindings — should get `: double` hints
    auto [m, v] = tada;

    // auto iterator — should get `: std::vector<double>::iterator` (or similar)
    auto it = data.begin();
    auto end_it = data.end();

    // auto from algorithm
    auto min_it = std::min_element(data.begin(), data.end());

    // optional
    std::optional opt = 3.012f;
    auto opt_val = opt.value_or(0.0);

    // map + auto iterator
    std::map<int, double> freq;
    for (double x : data) freq[static_cast<int>(x)]++;
    auto map_it = freq.begin();

    // lambda with auto capture
    auto square = [](double x) { return x * x; };
    auto sq = square(v);

    // throw std::runtime_error("yeet");

    printf("mean: %f\n", m);
    printf("variance: %f\n", v);
    printf("std dev: %f\n", std::sqrt(v));  
    
    {
        // std::ostringstream ss;
        std::cout << "data:     ";
        for (double x : data) std::cout << x << " ";
        std::cout << std::endl;
    }
    std::cout << "mean:     " << m << std::endl;
    std::cout << "variance: " << v << std::endl;
    std::cout << "std dev:  " << std::sqrt(v) << std::endl;

    vm::Vec2 g(1, 1);
    g = g * 10;

    std::cout << v << std::endl;
    std::cout << g.length() << std::endl;

    (void)it; (void)end_it; (void)min_it; (void)opt_val;
    (void)map_it; (void)sq;
    return 0;
}
