#include "timer.h"
#include <time.h>

void timer_start(struct timespec *t) {
    clock_gettime(CLOCK_MONOTONIC, t);
}

double timer_elapsed_ms(const struct timespec *start) {
    struct timespec now;
    clock_gettime(CLOCK_MONOTONIC, &now);
    return (now.tv_sec  - start->tv_sec)  * 1000.0
         + (now.tv_nsec - start->tv_nsec) / 1.0e6;
}
