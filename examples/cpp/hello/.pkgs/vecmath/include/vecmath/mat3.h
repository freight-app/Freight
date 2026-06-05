#pragma once
#include "vec3.h"

namespace vm {

/// Column-major 3×3 matrix.
struct Mat3 {
    double m[3][3] = {};

    static Mat3 identity() {
        Mat3 r; r.m[0][0] = r.m[1][1] = r.m[2][2] = 1.0; return r;
    }

    Vec3 operator*(const Vec3& v) const {
        return {
            m[0][0]*v.x + m[1][0]*v.y + m[2][0]*v.z,
            m[0][1]*v.x + m[1][1]*v.y + m[2][1]*v.z,
            m[0][2]*v.x + m[1][2]*v.y + m[2][2]*v.z,
        };
    }

    Mat3 operator*(const Mat3& o) const {
        Mat3 r;
        for (int i = 0; i < 3; i++)
            for (int j = 0; j < 3; j++)
                for (int k = 0; k < 3; k++)
                    r.m[i][j] += m[k][j] * o.m[i][k];
        return r;
    }
};

} // namespace vm
