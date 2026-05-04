#include <stdio.h>

/*
 * Build with different feature combinations to see which blocks are compiled in:
 *
 *   freight run                              # default features: logging
 *   freight run --features tls              # logging + tls + net (tls implies net)
 *   freight run --features json,net         # logging + json + net
 *   freight run --no-default-features       # no features active
 *   freight run --no-default-features --features tls   # tls + net only
 */

int main(void) {
    int features = 0;

#ifdef LOGGING
    printf("  [on]  logging\n");
    features++;
#else
    printf("  [off] logging\n");
#endif

#ifdef NET
    printf("  [on]  net\n");
    features++;
#else
    printf("  [off] net\n");
#endif

#ifdef TLS
    printf("  [on]  tls\n");
    features++;
#else
    printf("  [off] tls\n");
#endif

#ifdef JSON
    printf("  [on]  json\n");
    features++;
#else
    printf("  [off] json\n");
#endif

    printf("\n%d feature(s) active\n", features);
    return 0;
}
