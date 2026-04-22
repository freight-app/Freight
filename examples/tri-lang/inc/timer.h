#pragma once
#define _POSIX_C_SOURCE 200809L
#include <time.h>

void   timer_start(struct timespec *t);
double timer_elapsed_ms(const struct timespec *start);
