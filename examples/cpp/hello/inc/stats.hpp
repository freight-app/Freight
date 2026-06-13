/**
 * @file stats.hpp
 * @brief Simple statistical utility functions.
 *
 * This header provides small helpers for computing the mean and
 * variance of a set of double values supplied as a std::span.
 */

#pragma once
#include <span>

/// @brief Compute the arithmetic mean of a sequence of values.
/// @param values Input data span.
/// @returns The mean as a double.
double mean(std::span<const double> values);

/// @brief Compute the variance of a sequence of values.
/// @param values Input data span.
/// @returns Population variance as a double.
double variance(std::span<const double> values);
