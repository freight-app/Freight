#include "vecmath/vec2.h"
#include "vecmath/vec3.h"
#include "vecmath/mat3.h"
#include "mathlib/mathlib.h"

namespace vm {

/// Clamp each component of v to [lo, hi] using mathlib.
Vec3 clamp(const Vec3& v, double lo, double hi) {
    return { ml_clamp(v.x, lo, hi), ml_clamp(v.y, lo, hi), ml_clamp(v.z, lo, hi) };
}

/// Linear interpolation between two Vec3 values.
Vec3 lerp(const Vec3& a, const Vec3& b, double t) {
    return { ml_lerp(a.x, b.x, t), ml_lerp(a.y, b.y, t), ml_lerp(a.z, b.z, t) };
}

} // namespace vm
