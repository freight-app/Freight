#include <stdio.h>
#include <string.h>
#include "strutil.h"

int main(void) {
    const char *sentence = "the quick brown fox jumps";

    printf("input:       \"%s\"\n", sentence);
    printf("word count:  %d\n", str_count_words(sentence));
    printf("has 'fox':   %s\n", str_contains(sentence, "fox")  ? "yes" : "no");
    printf("has 'cat':   %s\n", str_contains(sentence, "cat")  ? "yes" : "no");

    char buf[64];
    strncpy(buf, sentence, sizeof(buf) - 1);
    buf[sizeof(buf) - 1] = '\0';
    str_reverse(buf);
    printf("reversed:    \"%s\"\n", buf);

    return 0;
}
