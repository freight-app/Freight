#include "strutil.h"
#include <ctype.h>
#include <string.h>

int str_count_words(const char *s) {
    int count = 0, in_word = 0;
    while (*s) {
        if (isspace((unsigned char)*s)) { in_word = 0; }
        else if (!in_word)              { in_word = 1; count++; }
        s++;
    }
    return count;
}

int str_contains(const char *haystack, const char *needle) {
    return strstr(haystack, needle) != NULL;
}

void str_reverse(char *s) {
    if (!s) return;
    size_t len = strlen(s);
    for (size_t i = 0, j = len - 1; i < j; i++, j--) {
        char tmp = s[i]; s[i] = s[j]; s[j] = tmp;
    }
}
