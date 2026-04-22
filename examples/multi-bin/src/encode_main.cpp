#include <iostream>
#include <string>
#include "base64.hpp"

int main() {
    std::string line;
    while (std::getline(std::cin, line)) {
        std::cout << base64_encode(line) << '\n';
    }
    return 0;
}
