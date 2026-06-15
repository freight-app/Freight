/* Declared as `[[example]] name = "fancy"` so its target name differs from the
 * file stem. Run with: freight run --example fancy */
#include <stdio.h>
#include "mathx.h"

int main(void) {
    printf("(2 + 3) * 4 = %d\n", mathx_mul(mathx_add(2, 3), 4));
    return 0;
}
