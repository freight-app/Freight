// stats.cpp — implementation file; documentation lives in stats.h.

#include "stats.h"
#include <algorithm>
#include <cmath>
#include <limits>
#include <numeric>
#include <stdexcept>

namespace stats {

double mean(const std::vector<double>& xs) {
    if (xs.empty()) throw std::invalid_argument("empty sample");
    return std::accumulate(xs.begin(), xs.end(), 0.0) / static_cast<double>(xs.size());
}

double variance(const std::vector<double>& xs) {
    if (xs.size() < 2) throw std::invalid_argument("need at least 2 elements");
    double m   = mean(xs);
    double acc = 0.0;
    for (double x : xs) acc += (x - m) * (x - m);
    return acc / static_cast<double>(xs.size() - 1);
}

double stddev(const std::vector<double>& xs) {
    return std::sqrt(variance(xs));
}

double pearson(const std::vector<double>& xs, const std::vector<double>& ys) {
    if (xs.size() != ys.size()) throw std::invalid_argument("length mismatch");
    double mx = mean(xs), my = mean(ys);
    double num = 0.0, dx2 = 0.0, dy2 = 0.0;
    for (std::size_t i = 0; i < xs.size(); ++i) {
        double dx = xs[i] - mx, dy = ys[i] - my;
        num += dx * dy; dx2 += dx * dx; dy2 += dy * dy;
    }
    return num / std::sqrt(dx2 * dy2);
}

OrderStatistics::OrderStatistics(std::vector<double> xs) : sorted_(std::move(xs)) {
    std::sort(sorted_.begin(), sorted_.end());
}

double OrderStatistics::median() const {
    std::size_t n = sorted_.size();
    if (n == 0) throw std::invalid_argument("empty sample");
    return (n % 2 == 1) ? sorted_[n / 2]
                        : (sorted_[n / 2 - 1] + sorted_[n / 2]) / 2.0;
}

double OrderStatistics::percentile(double p) const {
    if (sorted_.empty()) throw std::invalid_argument("empty sample");
    double idx = p / 100.0 * static_cast<double>(sorted_.size() - 1);
    std::size_t lo = static_cast<std::size_t>(idx);
    double frac    = idx - static_cast<double>(lo);
    if (lo + 1 >= sorted_.size()) return sorted_.back();
    return sorted_[lo] * (1.0 - frac) + sorted_[lo + 1] * frac;
}

namespace algo {

LinRegResult linreg(const std::vector<double>& xs, const std::vector<double>& ys) {
    if (xs.size() != ys.size()) throw std::invalid_argument("length mismatch");
    if (xs.size() < 2)          throw std::invalid_argument("need at least 2 points");
    double mx  = mean(xs), my = mean(ys);
    double num = 0.0, den = 0.0;
    for (std::size_t i = 0; i < xs.size(); ++i) {
        double dx = xs[i] - mx;
        num += dx * (ys[i] - my);
        den += dx * dx;
    }
    double slope     = (den == 0.0) ? std::numeric_limits<double>::quiet_NaN() : num / den;
    double intercept = my - slope * mx;
    return {slope, intercept};
}

double covariance(const std::vector<double>& xs, const std::vector<double>& ys) {
    if (xs.size() != ys.size()) throw std::invalid_argument("length mismatch");
    if (xs.size() < 2)          throw std::invalid_argument("need at least 2 elements");
    double mx = mean(xs), my = mean(ys);
    double acc = 0.0;
    for (std::size_t i = 0; i < xs.size(); ++i) acc += (xs[i] - mx) * (ys[i] - my);
    return acc / static_cast<double>(xs.size() - 1);
}

} // namespace algo
} // namespace stats
