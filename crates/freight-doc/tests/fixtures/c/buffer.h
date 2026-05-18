/**
 * @file buffer.h
 * @brief Fixed-capacity byte buffer with overflow detection.
 */
#pragma once
#include <stddef.h>

/** @brief Buffer capacity in bytes. */
#define BUFFER_CAP 1024

/**
 * @brief Fixed-capacity byte buffer.
 */
typedef struct Buffer {
    unsigned char data[BUFFER_CAP];
    size_t        len;
} Buffer;

/**
 * @brief Initialise a buffer to empty.
 * @param b Buffer to initialise.
 */
void buffer_init(Buffer *b);

/**
 * @brief Append bytes to a buffer.
 * @param b    Target buffer.
 * @param src  Source data.
 * @param n    Number of bytes.
 * @return     0 on success, -1 if capacity exceeded.
 */
int buffer_push(Buffer *b, const unsigned char *src, size_t n);

/**
 * @brief Reset a buffer without freeing memory.
 * @param b Buffer to reset.
 */
void buffer_clear(Buffer *b);

/// Compute the number of free bytes remaining.
/// @param b Buffer to query.
/// @return   BUFFER_CAP - b->len
size_t buffer_remaining(const Buffer *b);
