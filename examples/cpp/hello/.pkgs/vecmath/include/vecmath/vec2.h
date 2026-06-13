/**
 * @file vec2.h
 * @brief A simple 2D vector class for demonstration purposes.
 */

#pragma once
#include <cmath>

namespace vm {

struct Vec2 {
    double x, y;

    Vec2() : x(0), y(0) {}
    Vec2(double x, double y) : x(x), y(y) {}

    Vec2 operator+(const Vec2& o) const { return {x + o.x, y + o.y}; }
    Vec2 operator-(const Vec2& o) const { return {x - o.x, y - o.y}; }
    Vec2 operator*(double s)      const { return {x * s,   y * s};   }

    double dot(const Vec2& o) const { return x * o.x + y * o.y; }
    double length()           const { return std::sqrt(dot(*this)); }
    Vec2   normalized()       const { double l = length(); return l > 0 ? *this * (1.0/l) : *this; }
};

} // namespace vm
