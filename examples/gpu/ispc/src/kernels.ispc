/*
 * kernels.ispc — SPMD vector kernels compiled by ISPC.
 *
 * Each `export` function is callable from C/C++ via extern "C" linkage.
 * The `foreach` loop maps work over the implicit SIMD gang width so the
 * compiler can emit AVX2, AVX-512, NEON, etc. without manual intrinsics.
 */

/* Element-wise addition: c[i] = a[i] + b[i] */
export void vec_add(uniform float vin_a[],
                    uniform float vin_b[],
                    uniform float vout[],
                    uniform int   n) {
    foreach (i = 0 ... n) {
        vout[i] = vin_a[i] + vin_b[i];
    }
}

/* Element-wise scale: vout[i] = vin[i] * scale */
export void vec_scale(uniform float vin[],
                      uniform float vout[],
                      uniform int   n,
                      uniform float scale) {
    foreach (i = 0 ... n) {
        vout[i] = vin[i] * scale;
    }
}

/* Dot product: returns sum(a[i] * b[i]) */
export uniform float dot_product(uniform float a[],
                                 uniform float b[],
                                 uniform int   n) {
    varying float acc = 0.0f;
    foreach (i = 0 ... n) {
        acc += a[i] * b[i];
    }
    return reduce_add(acc);
}
