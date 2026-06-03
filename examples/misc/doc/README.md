# doc-example

A small multi-language project demonstrating **freight-doc** extraction and the
TUI documentation browser.

## Libraries

| Dep | Language | Purpose |
|-----|----------|---------|
| `mathlib` | C17 | Numerical methods (bisection, quadrature, LU) |
| `stats` | C++17 | Descriptive statistics and regression |
| `linalg` | Fortran 2018 | Dense linear algebra (dev-dependency) |

## Rendered doc features

This example exercises every doc-comment feature:

- **Doxygen** `@brief` / `@param` / `@return` / `@see` / `@warning` tags
- **FORD** `!>` and `!!` Fortran inline comments
- Inline math: $f(x) = e^{-x^2}$
- Display math:

$$\int_{-\infty}^{\infty} e^{-x^2}\,dx = \sqrt{\pi}$$

- Markdown tables *inside* doc-comment bodies
- Cross-reference links (`@see bisect` → navigable link to `bisect`)

## Running

```sh
cd examples/misc/doc
freight build
freight doc
```

`freight doc` opens the interactive TUI browser. Press **Enter** on a package
to expand it and load its README and API tree. Use **Tab** to switch focus
between the tree, content, and info panels. Press **q** to quit.

## Key bindings

| Key | Action |
|-----|--------|
| `↑` / `↓` or `k` / `j` | Navigate tree / scroll content |
| `Enter` | Expand dep or open symbol |
| `Tab` | Cycle focus: tree → content → info |
| `Esc` / `Backspace` | Return focus to tree |
| `g` / `G` | Jump to top / bottom |
| `PgUp` / `PgDn` | Page up / down |
| `q` | Quit |
