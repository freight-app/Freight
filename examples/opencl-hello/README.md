# opencl-hello

A minimal OpenCL example: two kernels (`vec_add`, `vec_scale`) operating on a
small float array, with CUDA-style error checking on every API call.

**Prerequisites:** an OpenCL ICD loader + at least one platform driver.

| Package | Provides |
|---|---|
| `nvidia-opencl-icd` | NVIDIA GPUs |
| `intel-opencl-icd` | Intel CPUs/GPUs |
| `mesa-opencl-icd` | AMD (Clover) |
| `pocl` | Software (CPU-only, no GPU required) |

```sh
freight build
freight run
```

Expected output (with a real OpenCL device):

```
Platform : NVIDIA CUDA
Device   : Tesla V100

vec_add  (a[i] + b[i]):
  [ 0]  1 + 2 = 3
  [ 1]  2 + 4 = 6
  ...
vec_scale(a[i] * 3):
  [ 0]  1 * 3 = 3
  [ 1]  2 * 3 = 6
  ...
```

## What it demonstrates

| Feature | Where |
|---|---|
| `OpenCL = "*"` in `[dependencies]` | Resolved via pkg-config (`-lOpenCL`) |
| `[language.cpp] std = "c++17"` | C++ standard passed to the host compiler |
| Runtime kernel compilation | `clCreateProgramWithSource` + `clBuildProgram` |
| Kernel source as an embedded string | `R"cl( ... )cl"` literal in `main.cpp` |
| Host↔device data transfer | `clCreateBuffer` + `CL_MEM_COPY_HOST_PTR` / `clEnqueueReadBuffer` |
| Error checking | `check(cl_int, const char*)` + `CHECK(expr)` macro |
| Platform/device selection | GPU preferred; CPU fallback |
| Build log on compile failure | `clGetProgramBuildInfo(CL_PROGRAM_BUILD_LOG)` |
| Release profile | `freight build --release` → `-O3` + stripped binary |

## How the kernels work

The kernels are **embedded in the host source** as a raw string literal and
compiled at run-time by the OpenCL JIT. No separate `.cl` files need to be
distributed with the binary:

```cpp
static const char* KERNEL_SRC = R"cl(
    __kernel void vec_add(__global const float* a,
                          __global const float* b,
                          __global       float* c, int n) {
        int i = get_global_id(0);
        if (i < n) c[i] = a[i] + b[i];
    }
    ...
)cl";
```

## Dependency declaration

OpenCL is declared as a regular version dep; freight resolves it via
`pkg-config --libs OpenCL` which returns `-lOpenCL`:

```toml
[dependencies]
OpenCL = "*"
```

On systems without pkg-config, install `ocl-icd-opencl-dev` (Debian/Ubuntu)
or `ocl-icd-devel` (Fedora) to get both the headers and the `.pc` file.
