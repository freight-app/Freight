#include "workspace_core.h"
#include <stdio.h>

int main(void) {
    printf("%s; answer=%d\n",
           freight_workspace_message(),
           freight_workspace_answer());
    return 0;
}
