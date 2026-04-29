/**
 * @file numerics.h
 * @brief Numerical methods with LaTeX math in doc comments.
 *
 * This header demonstrates that doc-comment bodies can contain:
 *
 * - **Markdown** formatting (bold, italic, lists, code spans, tables)
 * - LaTeX inline math: $f(x) = x^2$
 * - LaTeX display math: $$\int_a^b f(x)\,dx \approx \sum_{i=0}^{n} w_i f(x_i)$$
 *
 * All three forms survive extraction and render correctly in HTML (via MathJax),
 * Markdown (pass-through for MathJax/KaTeX), and LaTeX/PDF output.
 */

#ifndef NUMERICS_H
#define NUMERICS_H

/**
 * @brief Solve $f(x) = 0$ in $[a, b]$ via bisection.
 *
 * Requires $f(a) \cdot f(b) < 0$ (sign change guarantees a root by
 * the **Intermediate Value Theorem**).
 *
 * Convergence rate: the interval width halves each iteration, so after
 * $n$ steps the error satisfies:
 *
 * $$|e_n| \leq \frac{b - a}{2^{n+1}}$$
 *
 * @param f    Continuous function $f : \mathbb{R} \to \mathbb{R}$.
 * @param a    Left endpoint.
 * @param b    Right endpoint ($b > a$, $f(a) \cdot f(b) < 0$).
 * @param tol  Absolute tolerance; stops when $|b - a| < \text{tol}$.
 * @return     Approximate root $x^* \approx f^{-1}(0)$.
 */
double bisect(double (*f)(double), double a, double b, double tol);

/**
 * @brief Adaptive Simpson's rule for $\int_a^b f(x)\,dx$.
 *
 * Uses recursive subdivision until the local error estimate satisfies
 * $|\Delta| < \varepsilon / (b - a)$.  The composite rule on a single
 * panel $[c, d]$ is:
 *
 * $$S(c,d) = \frac{d-c}{6}\left[f(c) + 4f\!\left(\frac{c+d}{2}\right) + f(d)\right]$$
 *
 * | Parameter | Meaning |
 * |-----------|---------|
 * | `f`   | Integrand (must be smooth on $[a,b]$) |
 * | `a`   | Lower limit |
 * | `b`   | Upper limit |
 * | `eps` | Absolute error tolerance $\varepsilon > 0$ |
 *
 * @return Approximation of $\int_a^b f(x)\,dx$ with error $< \varepsilon$.
 * @see    bisect
 */
double integrate(double (*f)(double), double a, double b, double eps);

/**
 * @brief LU decomposition of an $n \times n$ matrix $A$.
 *
 * Factors $A = P L U$ where:
 * - $P$ is a permutation matrix (partial pivoting)
 * - $L$ is unit lower-triangular: $L_{ii} = 1$, $L_{ij} = 0$ for $j > i$
 * - $U$ is upper-triangular
 *
 * The factorisation is stored **in-place** in `A`; the strictly lower part
 * holds $L$ (without the diagonal ones) and the upper part holds $U$.
 *
 * Complexity: $\mathcal{O}(n^3)$ flops.
 *
 * @param A    Input matrix ($n \times n$), overwritten with $L$ and $U$.
 * @param piv  Output pivot index array of length $n$.
 * @param n    Matrix dimension.
 * @return     0 on success, -1 if $A$ is singular.
 * @warning    `A` must be stored in **row-major** order.
 */
int lu_decompose(double *A, int *piv, int n);

/**
 * @brief Solve $Ax = b$ using a pre-computed LU factorisation.
 *
 * Given the output of `lu_decompose`, solves $PLUx = b$ in two steps:
 *
 * 1. Forward substitution: $Ly = Pb$ — $\mathcal{O}(n^2)$
 * 2. Back substitution:    $Ux = y$  — $\mathcal{O}(n^2)$
 *
 * @param LU   Combined $L/U$ factors from `lu_decompose`.
 * @param piv  Pivot array from `lu_decompose`.
 * @param b    Right-hand side vector (length $n$), overwritten with $x$.
 * @param n    System dimension.
 */
void lu_solve(const double *LU, const int *piv, double *b, int n);

#endif /* NUMERICS_H */
