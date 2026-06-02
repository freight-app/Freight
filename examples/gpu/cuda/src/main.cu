#include <cstdio>
#include <cmath>
#include <cuda_runtime.h>

__global__ void vec_add(const float *a, const float *b, float *c, int n) {
    int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i < n) c[i] = a[i] + b[i];
}

__global__ void vec_scale(const float *a, float s, float *c, int n) {
    int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i < n) c[i] = a[i] * s;
}

static void check(cudaError_t e, const char *ctx) {
    if (e != cudaSuccess) {
        std::fprintf(stderr, "CUDA error in %s: %s\n", ctx, cudaGetErrorString(e));
        std::exit(1);
    }
}

int main() {
    constexpr int N = 16;
    constexpr int BLOCK = 256;

    float h_a[N], h_b[N], h_c[N];
    for (int i = 0; i < N; i++) { h_a[i] = float(i + 1); h_b[i] = float(i + 1) * 2.0f; }

    float *d_a, *d_b, *d_c;
    check(cudaMalloc(&d_a, N * sizeof(float)), "malloc a");
    check(cudaMalloc(&d_b, N * sizeof(float)), "malloc b");
    check(cudaMalloc(&d_c, N * sizeof(float)), "malloc c");

    check(cudaMemcpy(d_a, h_a, N * sizeof(float), cudaMemcpyHostToDevice), "H2D a");
    check(cudaMemcpy(d_b, h_b, N * sizeof(float), cudaMemcpyHostToDevice), "H2D b");

    // ── kernel 1: element-wise addition ──────────────────────────────
    vec_add<<<(N + BLOCK - 1) / BLOCK, BLOCK>>>(d_a, d_b, d_c, N);
    check(cudaGetLastError(), "vec_add launch");
    check(cudaDeviceSynchronize(), "sync");
    check(cudaMemcpy(h_c, d_c, N * sizeof(float), cudaMemcpyDeviceToHost), "D2H c");

    std::printf("vec_add  (a[i] + b[i]):\n");
    for (int i = 0; i < N; i++)
        std::printf("  [%2d]  %.0f + %.0f = %.0f\n", i, h_a[i], h_b[i], h_c[i]);

    // ── kernel 2: scale ───────────────────────────────────────────────
    vec_scale<<<(N + BLOCK - 1) / BLOCK, BLOCK>>>(d_a, 3.0f, d_c, N);
    check(cudaGetLastError(), "vec_scale launch");
    check(cudaDeviceSynchronize(), "sync");
    check(cudaMemcpy(h_c, d_c, N * sizeof(float), cudaMemcpyDeviceToHost), "D2H c");

    std::printf("vec_scale(a[i] * 3):\n");
    for (int i = 0; i < N; i++)
        std::printf("  [%2d]  %.0f * 3 = %.0f\n", i, h_a[i], h_c[i]);

    cudaFree(d_a); cudaFree(d_b); cudaFree(d_c);
    return 0;
}
