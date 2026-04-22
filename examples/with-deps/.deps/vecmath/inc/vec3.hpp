#pragma once
#include <cmath>

struct Vec3 {
    double x, y, z;
    Vec3(double x, double y, double z) : x(x), y(y), z(z) {}
    double length() const;
    Vec3 normalized() const;
    double dot(const Vec3& o) const;
};
