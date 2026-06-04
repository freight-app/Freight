#pragma once
#include <span>

/// Compute the arithmetic mean of a sequence of values.
/// @param values Input data span.
/// @returns The mean as a double.
double mean(std::span<const double> values);

/// Compute the variance of a sequence of values.
/// @param values Input data span.
/// @returns Population variance as a double.
double variance(std::span<const double> values);
