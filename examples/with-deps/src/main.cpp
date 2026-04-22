#include <iostream>
#include "vec3.hpp"

int main() {
    Vec3 a(1.0, 2.0, 3.0);
    Vec3 b(4.0, 5.0, 6.0);

    std::cout << "a = (" << a.x << ", " << a.y << ", " << a.z << ")\n";
    std::cout << "|a| = " << a.length() << "\n";
    std::cout << "a·b = " << a.dot(b) << "\n";

    Vec3 n = a.normalized();
    std::cout << "â = (" << n.x << ", " << n.y << ", " << n.z << ")\n";

    return 0;
}
