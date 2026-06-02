#include <hip/hip_runtime.h>

#include <cstdio>
#include <cstdlib>

static void check(hipError_t err, const char *context) {
    if (err != hipSuccess) {
        std::fprintf(stderr, "HIP error in %s: %s\n", context, hipGetErrorString(err));
        std::exit(1);
    }
}

__global__ void vec_add(const float *a, const float *b, float *out, int n) {
    int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i < n) {
        out[i] = a[i] + b[i];
    }
}

__global__ void vec_scale(const float *in, float *out, int n, float k) {
    int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i < n) {
        out[i] = in[i] * k;
    }
}

int main() {
    constexpr int n = 8;
    float host_a[n] = {1, 2, 3, 4, 5, 6, 7, 8};
    float host_b[n] = {8, 7, 6, 5, 4, 3, 2, 1};
    float host_c[n] = {};

    float *dev_a = nullptr;
    float *dev_b = nullptr;
    float *dev_c = nullptr;

    check(hipMalloc(&dev_a, n * sizeof(float)), "malloc a");
    check(hipMalloc(&dev_b, n * sizeof(float)), "malloc b");
    check(hipMalloc(&dev_c, n * sizeof(float)), "malloc c");

    check(hipMemcpy(dev_a, host_a, n * sizeof(float), hipMemcpyHostToDevice), "copy a");
    check(hipMemcpy(dev_b, host_b, n * sizeof(float), hipMemcpyHostToDevice), "copy b");

    dim3 block(128);
    dim3 grid((n + block.x - 1) / block.x);

    hipLaunchKernelGGL(vec_add, grid, block, 0, 0, dev_a, dev_b, dev_c, n);
    check(hipGetLastError(), "vec_add launch");
    check(hipDeviceSynchronize(), "vec_add sync");
    check(hipMemcpy(host_c, dev_c, n * sizeof(float), hipMemcpyDeviceToHost), "copy add");

    std::printf("vec_add:");
    for (float value : host_c) {
        std::printf(" %.1f", value);
    }
    std::printf("\n");

    hipLaunchKernelGGL(vec_scale, grid, block, 0, 0, dev_c, dev_c, n, 0.5f);
    check(hipGetLastError(), "vec_scale launch");
    check(hipDeviceSynchronize(), "vec_scale sync");
    check(hipMemcpy(host_c, dev_c, n * sizeof(float), hipMemcpyDeviceToHost), "copy scale");

    std::printf("vec_scale:");
    for (float value : host_c) {
        std::printf(" %.1f", value);
    }
    std::printf("\n");

    hipFree(dev_a);
    hipFree(dev_b);
    hipFree(dev_c);

    return 0;
}
