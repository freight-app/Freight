#include "vec3.hpp"
#include <cmath>

double Vec3::length() const {
    return std::sqrt(x*x + y*y + z*z);
}

Vec3 Vec3::normalized() const {
    double l = length();
    return Vec3(x/l, y/l, z/l);
}

double Vec3::dot(const Vec3& o) const {
    return x*o.x + y*o.y + z*o.z;
}
