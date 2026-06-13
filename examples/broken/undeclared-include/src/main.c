#include <stdio.h>   /* stdlib — allowed */
#include <pthread.h> /* undeclared platform dependency — must be rejected */

int main(void) {
    printf("unreachable: the build is blocked before compiling\n");
    return 0;
}
