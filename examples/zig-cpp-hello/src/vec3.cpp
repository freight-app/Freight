#include <cmath>

// Zig 0.16 misclassifies extern struct { double x, y, z } (24 bytes) as three
// SSE eightbytes rather than MEMORY class (SysV: >2 eightbytes → MEMORY).
// Symptom: passing Vec3 alongside a double arg aliases xmm0 with v.x.
// Rule: pass Vec3 by value only when ALL other params are also Vec3 (pure
// MEMORY-class call, no xmm register competition). Use const* for Vec3 args
// that appear alongside scalar params, and for single-Vec3 functions.

extern "C" {

struct Vec3 { double x, y, z; };

Vec3   vec3_add(Vec3 a, Vec3 b)           { return {a.x+b.x, a.y+b.y, a.z+b.z}; }
Vec3   vec3_sub(Vec3 a, Vec3 b)           { return {a.x-b.x, a.y-b.y, a.z-b.z}; }
Vec3   vec3_scale(const Vec3* v, double s){ return {v->x*s, v->y*s, v->z*s}; }
Vec3   vec3_cross(Vec3 a, Vec3 b)         { return {a.y*b.z - a.z*b.y,
                                                    a.z*b.x - a.x*b.z,
                                                    a.x*b.y - a.y*b.x}; }
double vec3_dot(Vec3 a, Vec3 b)           { return a.x*b.x + a.y*b.y + a.z*b.z; }
double vec3_length(const Vec3* v)         { return std::sqrt(v->x*v->x + v->y*v->y + v->z*v->z); }
Vec3   vec3_normalize(const Vec3* v)      { double l = vec3_length(v);
                                            return {v->x/l, v->y/l, v->z/l}; }

} // extern "C"
