# stats

Descriptive statistics library for C++17.  All symbols live in the `stats`
namespace; regression utilities are in the nested `stats::algo` namespace.

## Features

- Measures of **central tendency**: `stats::mean`
- Measures of **spread**: `stats::variance`, `stats::stddev`
- **Correlation**: `stats::pearson`
- Order-statistic queries via `stats::OrderStatistics` (median, percentiles)
- Linear regression and covariance in `stats::algo`

## Example

```cpp
#include "stats.h"
#include <vector>

std::vector<double> xs = {1.2, 3.4, 2.1, 5.6, 4.8};
std::vector<double> ys = {0.1, 1.4, 0.8, 2.1, 1.9};

double m  = stats::mean(xs);        // 3.42
double sd = stats::stddev(xs);      // ~1.69

auto [slope, intercept] = stats::algo::linreg(xs, ys);
```

## Summary statistics

Given a sample $x_1, \ldots, x_n$, the sample mean is:

$$\bar{x} = \frac{1}{n}\sum_{i=1}^{n} x_i$$

and the **unbiased** sample variance is:

$$s^2 = \frac{1}{n-1}\sum_{i=1}^{n}(x_i - \bar{x})^2$$

## Namespaces

| Namespace | Contents |
|-----------|----------|
| `stats` | `mean`, `variance`, `stddev`, `pearson`, `OrderStatistics`, `LinRegResult` |
| `stats::algo` | `linreg`, `covariance` |

## API overview

| Symbol | Returns | Description |
|--------|---------|-------------|
| `stats::mean` | `double` | Arithmetic mean |
| `stats::variance` | `double` | Unbiased sample variance $s^2$ |
| `stats::stddev` | `double` | Sample standard deviation $s$ |
| `stats::pearson` | `double` | Pearson $r \in [-1, 1]$ |
| `stats::OrderStatistics` | class | Sorted sample view |
| `stats::algo::linreg` | `LinRegResult` | Least-squares slope and intercept |
| `stats::algo::covariance` | `double` | Sample covariance |

## Notes

> All functions throw `std::invalid_argument` when passed an empty vector.
> `variance` and `stddev` additionally require at least two elements.

## License

Apache-2.0
