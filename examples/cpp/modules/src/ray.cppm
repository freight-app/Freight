module;
#include <cmath>
export module ray;

import vec3;

export struct Ray {
    Vec3 origin;
    Vec3 direction; // assumed normalised

    constexpr Vec3 at(double t) const { return origin + direction * t; }
};

// Returns the parameter t at which the ray hits a sphere, or -1 if no hit.
export double hit_sphere(const Vec3& centre, double radius, const Ray& ray) {
    Vec3   oc = ray.origin - centre;
    double a  = ray.direction.dot(ray.direction);
    double b  = 2.0 * oc.dot(ray.direction);
    double c  = oc.dot(oc) - radius * radius;
    double d  = b * b - 4.0 * a * c;
    if (d < 0.0) return -1.0;
    return (-b - std::sqrt(d)) / (2.0 * a);
}
