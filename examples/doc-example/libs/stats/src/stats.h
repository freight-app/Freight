/**
 * @file stats.h
 * @brief Descriptive statistics — C++ public API.
 *
 * All functions operate on `std::vector<double>` samples and throw
 * `std::invalid_argument` for degenerate inputs (empty vectors, length
 * mismatches, or samples too small for the requested statistic).
 */

#pragma once
#include <vector>

/**
 * @brief Arithmetic mean of a sample.
 *
 * @param xs  Non-empty input vector.
 * @return    Sample mean $\bar{x} = \frac{1}{n}\sum x_i$.
 * @throws    std::invalid_argument if xs is empty.
 */
double mean(const std::vector<double>& xs);

/**
 * @brief Sample variance (unbiased, Bessel-corrected).
 *
 * Uses $s^2 = \frac{1}{n-1}\sum(x_i - \bar{x})^2$.
 *
 * @param xs  Input sample (≥ 2 elements required).
 * @return    $s^2$
 * @see       mean
 */
double variance(const std::vector<double>& xs);

/// Standard deviation of a sample.
/// @param xs  Input sample (≥ 2 elements required).
/// @return    $\sqrt{s^2}$
double stddev(const std::vector<double>& xs);

/**
 * @brief Pearson correlation coefficient between two equal-length samples.
 *
 * Returns a value in $[-1, 1]$. Returns NaN when either sample has zero
 * variance.
 *
 * @param xs  First sample.
 * @param ys  Second sample (must be the same length as xs).
 * @return    $r \in [-1, 1]$.
 */
double pearson(const std::vector<double>& xs, const std::vector<double>& ys);

/**
 * @brief Immutable sorted view into a sample for order-statistic queries.
 */
class OrderStatistics {
public:
    /**
     * @brief Construct from a sample (makes a sorted copy).
     * @param xs  Input sample.
     */
    explicit OrderStatistics(std::vector<double> xs);

    /// Median of the sample.
    double median() const;

    /**
     * @brief Percentile via linear interpolation.
     * @param p  Percentile in [0, 100].
     * @return   Interpolated value.
     */
    double percentile(double p) const;

private:
    std::vector<double> sorted_;
};
