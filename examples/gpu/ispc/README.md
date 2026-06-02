# ispc-hello

ISPC (Intel SPMD Program Compiler) kernels called from a C++ host.
Three kernels (`vec_add`, `vec_scale`, `dot_product`) are auto-vectorised
by ISPC for the target ISA and linked directly into the binary.

**Prerequisites:** `ispc` on `$PATH` (version 1.13+).

```sh
freight build
freight run
```

Expected output:

```
vec_add  (a[i] + b[i]):
  [0]  1 + 2 = 3
  [1]  2 + 4 = 6
  ...
vec_scale(a[i] * 3):
  [0]  1 * 3 = 3
  ...
dot(a, b) = 408  (expected 408)

All checks passed.
```

## What it demonstrates

| Feature | Where |
|---|---|
| Mixed-language project | `src/kernels.ispc` + `src/main.cpp` auto-discovered |
| `[language.ispc] target` | Maps to `--target=avx2-i32x8`; controls SIMD width |
| `[language.cpp] std = "c++17"` | Host-side C++ standard |
| SPMD `foreach` loop | Parallel element iteration without manual intrinsics |
| `reduce_add(acc)` | Cross-lane horizontal reduction |
| `extern "C"` declarations | Link ISPC functions from C++ without a generated header |
| `namespace ispc` | ISPC emits symbols inside `namespace ispc` for C++ |
| Release profile | `freight build --release` → `-O3` + stripped binary |

## How ISPC kernels are called

ISPC `export` functions are emitted as `extern "C"` symbols inside
`namespace ispc`.  The host file declares them manually to avoid needing
the auto-generated `*_ispc.h` header:

```cpp
namespace ispc {
    extern "C" void vec_add(float* a, float* b, float* c, int32_t n);
}
// call:
ispc::vec_add(a, b, c, N);
```

## Selecting a target ISA

The `target` language option maps directly to `ispc --target=<value>`:

```toml
[language.ispc]
target = "avx2-i32x8"    # x86-64, AVX2, 8-wide gang (Haswell+)
```

| `target` | ISA | Gang width | CPU family |
|---|---|---|---|
| `sse2-i32x4` | SSE2 | 4 | Any x86-64 |
| `sse4-i32x4` | SSE4.1 | 4 | Penryn 2007+ |
| `avx2-i32x8` | AVX2 | 8 | Haswell 2013+ |
| `avx512skx-i32x16` | AVX-512 | 16 | Skylake-X 2017+ |
| `neon-i32x4` | NEON | 4 | AArch64 |

Omit `target` to let ISPC auto-detect the host CPU (equivalent to
`--target=host`).
