/**
 * @file stats.cpp
 * @brief Descriptive statistics — C++ with mixed Doxygen styles.
 */

#include <vector>
#include <numeric>
#include <stdexcept>
#include <cmath>

/**
 * @brief Arithmetic mean of a sample.
 *
 * @param xs  Non-empty input vector.
 * @return    Sample mean.
 * @throws    std::invalid_argument if xs is empty.
 */
double mean(const std::vector<double>& xs) {
    if (xs.empty()) throw std::invalid_argument("empty sample");
    return std::accumulate(xs.begin(), xs.end(), 0.0) / xs.size();
}

/*!
 * @brief Sample variance (unbiased, Bessel-corrected).
 *
 * @param xs  Input sample (≥ 2 elements required).
 * @return    s²
 * @see       mean
 */
double variance(const std::vector<double>& xs) {
    if (xs.size() < 2) throw std::invalid_argument("need at least 2 elements");
    double m = mean(xs);
    double acc = 0.0;
    for (double x : xs) acc += (x - m) * (x - m);
    return acc / (xs.size() - 1);
}

/// Standard deviation of a sample.
/// @param xs  Input sample.
/// @return    sqrt(variance(xs))
double stddev(const std::vector<double>& xs) {
    return std::sqrt(variance(xs));
}

/**
 * @brief Pearson correlation coefficient between two equal-length samples.
 *
 * Returns a value in [-1, 1]. Returns NaN when either sample has zero variance.
 *
 * @param xs  First sample.
 * @param ys  Second sample (must be the same length as xs).
 * @return    r ∈ [-1, 1].
 */
double pearson(const std::vector<double>& xs, const std::vector<double>& ys) {
    if (xs.size() != ys.size()) throw std::invalid_argument("length mismatch");
    double mx = mean(xs), my = mean(ys);
    double num = 0.0, dx2 = 0.0, dy2 = 0.0;
    for (size_t i = 0; i < xs.size(); ++i) {
        double dx = xs[i] - mx, dy = ys[i] - my;
        num += dx * dy;
        dx2 += dx * dx;
        dy2 += dy * dy;
    }
    return num / std::sqrt(dx2 * dy2);
}

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
