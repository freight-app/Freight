#include <stdint.h>
#include <stdio.h>

// Sum eight 32-bit integers. With AVX2 enabled (via `[arch.*] features =
// ["avx2"]`) the SIMD path compiles because freight passed -mavx2, which both
// defines __AVX2__ and makes <immintrin.h> usable without it being a declared
// dependency. On any other target the scalar fallback is built instead.
#ifdef __AVX2__
#include <immintrin.h>

static int sum8(const int32_t v[8]) {
    __m256i acc = _mm256_loadu_si256((const __m256i *)v);
    __m128i lo = _mm256_castsi256_si128(acc);
    __m128i hi = _mm256_extracti128_si256(acc, 1);
    __m128i s = _mm_add_epi32(lo, hi);
    s = _mm_hadd_epi32(s, s);
    s = _mm_hadd_epi32(s, s);
    return _mm_cvtsi128_si32(s);
}
static const char *backend = "AVX2";
#else
static int sum8(const int32_t v[8]) {
    int total = 0;
    for (int i = 0; i < 8; i++) total += v[i];
    return total;
}
static const char *backend = "scalar";
#endif

int main(void) {
    int32_t v[8] = {1, 2, 3, 4, 5, 6, 7, 8};
    printf("sum (%s) = %d\n", backend, sum8(v));
    return 0;
}
