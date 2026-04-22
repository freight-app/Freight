module;
#include <cmath>
export module vec3;

export struct Vec3 {
    double x, y, z;

    constexpr Vec3(double x, double y, double z) : x(x), y(y), z(z) {}

    constexpr Vec3 operator+(const Vec3& o) const { return {x+o.x, y+o.y, z+o.z}; }
    constexpr Vec3 operator-(const Vec3& o) const { return {x-o.x, y-o.y, z-o.z}; }
    constexpr Vec3 operator*(double s)      const { return {x*s,   y*s,   z*s};   }

    constexpr double dot(const Vec3& o) const { return x*o.x + y*o.y + z*o.z; }
    constexpr Vec3   cross(const Vec3& o) const {
        return { y*o.z - z*o.y, z*o.x - x*o.z, x*o.y - y*o.x };
    }
    double length() const { return std::sqrt(dot(*this)); }
    Vec3   normalise() const { return *this * (1.0 / length()); }
};
