# mathlib

Portable C math utilities — numerical methods and linear algebra primitives.

## Features

- **Bisection** root-finding with guaranteed convergence
- **Adaptive quadrature** via recursive Simpson's rule
- **LU decomposition** with partial pivoting ($\mathcal{O}(n^3)$)

## Quick Start

```c
#include "mathlib.h"
#include "numerics.h"

double root = bisect(sin, 3.0, 4.0, 1e-10);
double area = integrate(cos, 0.0, M_PI_2, 1e-8);
```

## Numerical Methods

The library uses standard floating-point arithmetic. All functions operate on
`double` precision values and are thread-safe (no global state).

### Error bounds

For `bisect`, after $n$ iterations the error satisfies:

$$|e_n| \leq \frac{b - a}{2^{n+1}}$$

For `integrate`, the adaptive rule terminates when the local error estimate
falls below $\varepsilon / (b - a)$ on each sub-interval.

## API reference

| Function | Purpose |
|----------|---------|
| `bisect` | Find $f(x) = 0$ in $[a, b]$ |
| `integrate` | Compute $\int_a^b f(x)\,dx$ |
| `lu_decompose` | Factor $A = PLU$ |
| `lu_solve` | Solve $Ax = b$ from an LU factorisation |
| `clamp` | Clamp a value to $[lo, hi]$ |
| `lerp` | Linear interpolation $a + t(b - a)$ |

## Building

```sh
freight build
```

Requires a C17-capable compiler. No external dependencies.

## License

Apache-2.0
