#include <iostream>

// Deliberate errors:
//   1. missing semicolon after struct definition
//   2. use of undeclared identifier

struct Point {
    int x;
    int y;
}        // ← missing semicolon

int main() {
    Point p = {1, 2};
    std::cout << "x=" << p.x << " y=" << p.y << "\n"

    // This references a variable that was never declared:
    std::cout << "z=" << undefined_z << "\n";
    return 0;
}
