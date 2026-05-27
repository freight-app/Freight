# d-hello

A pure-D binary demonstrating ranges, UFCS, operator overloading, and C interop.

**Prerequisites:** any of `dmd`, `ldc2`, or `gdc` on `$PATH`.

```sh
freight build
freight run
```

Expected output:

```
─── Range pipeline ───
Even numbers [1,20]:        [2, 4, 6, 8, 10, 12, 14, 16, 18, 20]
Their squares:              [4, 16, 36, 64, 100, 144, 196, 256, 324, 400]
Min square: 4  Max square: 400

─── Vec2 arithmetic ───
a            = (3.000, 4.000)  length = 5.000
b            = (1.000, -2.000)  length = 2.236
a + b        = (4.000, 2.000)
a.normalized = (0.600, 0.800)  (length ≈ 1: 1.000000)
a · b        = -5.000

─── First 12 Fibonacci numbers ───
[0, 1, 1, 2, 3, 5, 8, 13, 21, 34, 55, 89]

─── C interop: qsort ───
before: [3.14, -1, 2.72, 0, 1.41, -0.5]
after:  [-1, -0.5, 0, 1.41, 2.72, 3.14]
```

## What it demonstrates

| Feature | Where |
|---|---|
| `[compiler] backend = "ldc2"` | Prefer LDC over DMD for better optimisation |
| Range pipeline | `iota` + `filter` + `map` + `array` |
| UFCS (Uniform Function Call Syntax) | `evens.map!(n => n * n)` |
| Operator overloading | `Vec2.opBinary!"+"`, `Vec2.opBinary!"*"` |
| `@property` | `Vec2.length`, `Vec2.normalized` |
| Lazy struct range | `Fibonacci` with `empty` / `front` / `popFront` |
| `extern (C)` interop | Call libc `qsort` directly without a binding library |
| Release profile | `freight build --release` → `-O -release` + stripped binary |

## Compiler selection

`[compiler] backend = "ldc2"` makes freight prefer LDC2 when available and fall
back to DMD otherwise. All three D compilers are supported:

| Backend | Notes |
|---|---|
| `ldc2` | LLVM-based; best optimisation; recommended |
| `dmd` | Reference compiler; fastest compilation |
| `gdc` | GCC front-end; integrates with GCC toolchain |

```toml
[compiler]
backend = "gdc"   # or "dmd"
```
