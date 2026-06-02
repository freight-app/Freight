/**
 * opencl-hello — minimal OpenCL example.
 *
 * Runs two kernels on a small float array:
 *   vec_add  : c[i] = a[i] + b[i]
 *   vec_scale: c[i] = a[i] * k
 *
 * All OpenCL API calls are wrapped with CHECK() for clean error reporting.
 * Kernels are compiled at run-time from embedded source strings (the
 * standard "JIT" model; no .cl files on disk are required).
 */

#define CL_TARGET_OPENCL_VERSION 120
#include <CL/cl.h>

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <vector>

// ── Error checking ─────────────────────────────────────────────────────────────

static const char* cl_err_str(cl_int err) {
    switch (err) {
    case CL_SUCCESS:                         return "CL_SUCCESS";
    case CL_DEVICE_NOT_FOUND:                return "CL_DEVICE_NOT_FOUND";
    case CL_DEVICE_NOT_AVAILABLE:            return "CL_DEVICE_NOT_AVAILABLE";
    case CL_BUILD_PROGRAM_FAILURE:           return "CL_BUILD_PROGRAM_FAILURE";
    case CL_INVALID_VALUE:                   return "CL_INVALID_VALUE";
    case CL_INVALID_DEVICE:                  return "CL_INVALID_DEVICE";
    case CL_INVALID_CONTEXT:                 return "CL_INVALID_CONTEXT";
    case CL_INVALID_PLATFORM:                return "CL_INVALID_PLATFORM";
    case CL_INVALID_KERNEL_NAME:             return "CL_INVALID_KERNEL_NAME";
    case CL_INVALID_MEM_OBJECT:              return "CL_INVALID_MEM_OBJECT";
    case CL_INVALID_ARG_INDEX:               return "CL_INVALID_ARG_INDEX";
    case CL_INVALID_ARG_VALUE:               return "CL_INVALID_ARG_VALUE";
    case CL_INVALID_ARG_SIZE:                return "CL_INVALID_ARG_SIZE";
    case CL_INVALID_KERNEL:                  return "CL_INVALID_KERNEL";
    case CL_INVALID_WORK_GROUP_SIZE:         return "CL_INVALID_WORK_GROUP_SIZE";
    case CL_INVALID_GLOBAL_WORK_SIZE:        return "CL_INVALID_GLOBAL_WORK_SIZE";
    case CL_OUT_OF_RESOURCES:                return "CL_OUT_OF_RESOURCES";
    case CL_OUT_OF_HOST_MEMORY:              return "CL_OUT_OF_HOST_MEMORY";
    default:                                 return "<unknown CL error>";
    }
}

static void check(cl_int err, const char* where) {
    if (err != CL_SUCCESS) {
        std::fprintf(stderr, "OpenCL error %d (%s) at %s\n", err, cl_err_str(err), where);
        std::exit(1);
    }
}

#define CHECK(expr) check((expr), #expr)

// ── Kernel source (embedded; no .cl file needed at run time) ──────────────────

static const char* KERNEL_SRC = R"cl(
__kernel void vec_add(__global const float* a,
                      __global const float* b,
                      __global       float* c,
                      int n) {
    int i = get_global_id(0);
    if (i < n) c[i] = a[i] + b[i];
}

__kernel void vec_scale(__global const float* a,
                        __global       float* c,
                        float k,
                        int n) {
    int i = get_global_id(0);
    if (i < n) c[i] = a[i] * k;
}
)cl";

// ── Main ──────────────────────────────────────────────────────────────────────

int main() {
    // ── 1. Select platform and device ────────────────────────────────────────
    cl_uint num_platforms = 0;
    cl_int rc = clGetPlatformIDs(0, nullptr, &num_platforms);
    if (rc != CL_SUCCESS || num_platforms == 0) {
        std::fprintf(stderr,
            "No OpenCL platforms found (clGetPlatformIDs returned %d).\n"
            "Install an OpenCL ICD loader and a platform driver, e.g.:\n"
            "  • nvidia-opencl-icd   (NVIDIA)\n"
            "  • intel-opencl-icd    (Intel)\n"
            "  • mesa-opencl-icd     (AMD / software via Clover)\n"
            "  • pocl                (CPU-only portable OpenCL)\n",
            rc);
        return 1;
    }

    std::vector<cl_platform_id> platforms(num_platforms);
    CHECK(clGetPlatformIDs(num_platforms, platforms.data(), nullptr));

    // Pick the first GPU; fall back to the first CPU.
    cl_device_id device = nullptr;
    cl_platform_id selected_platform = nullptr;

    for (auto p : platforms) {
        cl_uint ndev = 0;
        if (clGetDeviceIDs(p, CL_DEVICE_TYPE_GPU, 0, nullptr, &ndev) == CL_SUCCESS && ndev > 0) {
            cl_device_id d;
            CHECK(clGetDeviceIDs(p, CL_DEVICE_TYPE_GPU, 1, &d, nullptr));
            device = d;
            selected_platform = p;
            break;
        }
    }
    if (!device) {
        for (auto p : platforms) {
            cl_uint ndev = 0;
            if (clGetDeviceIDs(p, CL_DEVICE_TYPE_CPU, 0, nullptr, &ndev) == CL_SUCCESS && ndev > 0) {
                cl_device_id d;
                CHECK(clGetDeviceIDs(p, CL_DEVICE_TYPE_CPU, 1, &d, nullptr));
                device = d;
                selected_platform = p;
                break;
            }
        }
    }

    if (!device) {
        std::fprintf(stderr, "No usable OpenCL device found on any platform.\n");
        return 1;
    }

    // Print info about the chosen device.
    char buf[256];
    clGetPlatformInfo(selected_platform, CL_PLATFORM_NAME, sizeof(buf), buf, nullptr);
    std::printf("Platform : %s\n", buf);
    clGetDeviceInfo(device, CL_DEVICE_NAME, sizeof(buf), buf, nullptr);
    std::printf("Device   : %s\n\n", buf);

    // ── 2. Create context and command queue ───────────────────────────────────
    cl_int err;
    cl_context ctx = clCreateContext(nullptr, 1, &device, nullptr, nullptr, &err);
    check(err, "clCreateContext");

    cl_command_queue queue = clCreateCommandQueue(ctx, device, 0, &err);
    check(err, "clCreateCommandQueue");

    // ── 3. Compile kernels ────────────────────────────────────────────────────
    cl_program prog = clCreateProgramWithSource(ctx, 1, &KERNEL_SRC, nullptr, &err);
    check(err, "clCreateProgramWithSource");

    err = clBuildProgram(prog, 1, &device, nullptr, nullptr, nullptr);
    if (err != CL_SUCCESS) {
        // Print the build log before aborting.
        size_t log_size = 0;
        clGetProgramBuildInfo(prog, device, CL_PROGRAM_BUILD_LOG, 0, nullptr, &log_size);
        std::vector<char> log(log_size);
        clGetProgramBuildInfo(prog, device, CL_PROGRAM_BUILD_LOG, log_size, log.data(), nullptr);
        std::fprintf(stderr, "Kernel build failed:\n%s\n", log.data());
        check(err, "clBuildProgram");
    }

    cl_kernel k_add   = clCreateKernel(prog, "vec_add",   &err); check(err, "kernel vec_add");
    cl_kernel k_scale = clCreateKernel(prog, "vec_scale", &err); check(err, "kernel vec_scale");

    // ── 4. Set up data ────────────────────────────────────────────────────────
    constexpr int N = 8;
    float h_a[N], h_b[N], h_c[N];
    for (int i = 0; i < N; ++i) { h_a[i] = float(i + 1); h_b[i] = float((i + 1) * 2); }

    cl_mem d_a = clCreateBuffer(ctx, CL_MEM_READ_ONLY  | CL_MEM_COPY_HOST_PTR, N * sizeof(float), h_a, &err); check(err, "buf a");
    cl_mem d_b = clCreateBuffer(ctx, CL_MEM_READ_ONLY  | CL_MEM_COPY_HOST_PTR, N * sizeof(float), h_b, &err); check(err, "buf b");
    cl_mem d_c = clCreateBuffer(ctx, CL_MEM_WRITE_ONLY,                         N * sizeof(float), nullptr, &err); check(err, "buf c");

    // ── 5. Run vec_add ────────────────────────────────────────────────────────
    int n = N;
    CHECK(clSetKernelArg(k_add, 0, sizeof(cl_mem), &d_a));
    CHECK(clSetKernelArg(k_add, 1, sizeof(cl_mem), &d_b));
    CHECK(clSetKernelArg(k_add, 2, sizeof(cl_mem), &d_c));
    CHECK(clSetKernelArg(k_add, 3, sizeof(int),    &n));

    size_t gws = N;
    CHECK(clEnqueueNDRangeKernel(queue, k_add, 1, nullptr, &gws, nullptr, 0, nullptr, nullptr));
    CHECK(clFinish(queue));
    CHECK(clEnqueueReadBuffer(queue, d_c, CL_TRUE, 0, N * sizeof(float), h_c, 0, nullptr, nullptr));

    std::printf("vec_add  (a[i] + b[i]):\n");
    for (int i = 0; i < N; ++i)
        std::printf("  [%2d]  %.0f + %.0f = %.0f\n", i, h_a[i], h_b[i], h_c[i]);

    // ── 6. Run vec_scale ──────────────────────────────────────────────────────
    const float k = 3.0f;
    CHECK(clSetKernelArg(k_scale, 0, sizeof(cl_mem), &d_a));
    CHECK(clSetKernelArg(k_scale, 1, sizeof(cl_mem), &d_c));
    CHECK(clSetKernelArg(k_scale, 2, sizeof(float),  &k));
    CHECK(clSetKernelArg(k_scale, 3, sizeof(int),    &n));

    CHECK(clEnqueueNDRangeKernel(queue, k_scale, 1, nullptr, &gws, nullptr, 0, nullptr, nullptr));
    CHECK(clFinish(queue));
    CHECK(clEnqueueReadBuffer(queue, d_c, CL_TRUE, 0, N * sizeof(float), h_c, 0, nullptr, nullptr));

    std::printf("\nvec_scale(a[i] * %.0f):\n", k);
    for (int i = 0; i < N; ++i)
        std::printf("  [%2d]  %.0f * %.0f = %.0f\n", i, h_a[i], k, h_c[i]);

    // ── 7. Clean up ───────────────────────────────────────────────────────────
    clReleaseMemObject(d_a);
    clReleaseMemObject(d_b);
    clReleaseMemObject(d_c);
    clReleaseKernel(k_add);
    clReleaseKernel(k_scale);
    clReleaseProgram(prog);
    clReleaseCommandQueue(queue);
    clReleaseContext(ctx);

    return 0;
}
