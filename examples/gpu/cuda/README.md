# cuda-hello

A minimal CUDA example: two kernels (`vec_add`, `vec_scale`) operating on a
small float array, with proper CUDA error checking on every API call.

**Prerequisites:** CUDA toolkit (`nvcc`) on `$PATH` and a CUDA-capable GPU with
an up-to-date driver.

```sh
freight build
freight run
```

Expected output (with a real GPU):

```
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
| `[language.cuda] std` | Device-code C++ standard passed to nvcc as `-std=c++17` |
| `__global__` kernel launch | `vec_add<<<grid, block>>>(...)` |
| CUDA error checking | `check(cudaError_t, const char*)` wrapper |
| `cudaMalloc` / `cudaMemcpy` / `cudaFree` | host↔device data transfer |
| `cudaDeviceSynchronize` | barrier after async kernel launch |
| Release profile | `freight build --release` → `-O3` + stripped binary |

## Targeting a specific GPU architecture

To generate optimised code for a specific compute capability, pass the arch
flag through `[language.cuda] extra_flags`:

```toml
[language.cuda]
std        = "c++17"
extra_flags = "-arch=sm_89"   # e.g. Ada Lovelace (RTX 40xx)
```

Common targets:

| Architecture | `sm_XX` | GPUs |
|---|---|---|
| Pascal | `sm_60` | GTX 10xx, Tesla P100 |
| Volta | `sm_70` | Tesla V100 |
| Turing | `sm_75` | RTX 20xx, GTX 16xx |
| Ampere | `sm_80` / `sm_86` | RTX 30xx, A100 |
| Ada Lovelace | `sm_89` | RTX 40xx |
| Hopper | `sm_90` | H100 |

Without `-arch`, nvcc defaults to `sm_52` (Maxwell) for maximum compatibility.
