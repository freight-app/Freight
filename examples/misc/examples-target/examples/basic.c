/* Auto-discovered example: any compilable file under examples/ becomes an
 * example target named after its file stem ("basic"). It links against the
 * project's library. */
#include <stdio.h>
#include "mathx.h"

int main(void) {
    printf("2 + 3 = %d\n", mathx_add(2, 3));
    return 0;
}
