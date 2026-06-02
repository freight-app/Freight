/**
 * ispc-hello — host side of an ISPC SPMD example.
 *
 * The ISPC compiler (kernels.ispc) generates object code with SIMD-width
 * vectorisation for the target ISA (SSE2, AVX2, AVX-512, NEON…).  The
 * kernels are linked directly into this binary; no shared library is needed.
 *
 * Prototype note: ISPC `export` functions use `extern "C"` linkage and live
 * in namespace `ispc`.  We declare them manually here so that no generated
 * header file needs to be distributed with the project.
 */

#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cmath>

// ── ISPC kernel declarations ───────────────────────────────────────────────────
//
// These match the `export` functions in kernels.ispc.  ISPC maps
//   uniform float[]  → float*
//   uniform int      → int32_t
//   uniform float    → float
// and wraps everything in `extern "C"` inside `namespace ispc`.

namespace ispc {
    extern "C" {
        void vec_add  (float* a, float* b, float* c,  int32_t n);
        void vec_scale(float* vin, float* vout, int32_t n, float scale);
        float dot_product(float* a, float* b, int32_t n);
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────────

static bool approx(float a, float b, float tol = 1e-4f) {
    return std::fabsf(a - b) <= tol;
}

static void fail(const char* msg) {
    std::fprintf(stderr, "FAIL: %s\n", msg);
    std::exit(1);
}

// ── main ───────────────────────────────────────────────────────────────────────

int main() {
    constexpr int N = 8;

    float a[N], b[N], c[N];
    for (int i = 0; i < N; ++i) { a[i] = float(i + 1); b[i] = float((i + 1) * 2); }

    // ── vec_add ────────────────────────────────────────────────────────────────
    ispc::vec_add(a, b, c, N);
    std::printf("vec_add  (a[i] + b[i]):\n");
    for (int i = 0; i < N; ++i) {
        std::printf("  [%d]  %.0f + %.0f = %.0f\n", i, a[i], b[i], c[i]);
        if (!approx(c[i], a[i] + b[i])) fail("vec_add mismatch");
    }

    // ── vec_scale ──────────────────────────────────────────────────────────────
    constexpr float K = 3.0f;
    ispc::vec_scale(a, c, N, K);
    std::printf("\nvec_scale(a[i] * %.0f):\n", K);
    for (int i = 0; i < N; ++i) {
        std::printf("  [%d]  %.0f * %.0f = %.0f\n", i, a[i], K, c[i]);
        if (!approx(c[i], a[i] * K)) fail("vec_scale mismatch");
    }

    // ── dot_product ────────────────────────────────────────────────────────────
    float dot = ispc::dot_product(a, b, N);
    // a = {1..8}, b = {2,4,..,16} → sum(i*(2i)) for i=1..8
    float expected = 0.0f;
    for (int i = 0; i < N; ++i) expected += a[i] * b[i];
    std::printf("\ndot(a, b) = %.0f  (expected %.0f)\n", dot, expected);
    if (!approx(dot, expected)) fail("dot_product mismatch");

    std::printf("\nAll checks passed.\n");
    return 0;
}
