#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* Merge sort — clean C with no UB, compiled via zig cc */

static void merge(int *arr, int *tmp, int lo, int mid, int hi) {
    memcpy(tmp + lo, arr + lo, (size_t)(hi - lo + 1) * sizeof(int));
    int i = lo, j = mid + 1, k = lo;
    while (i <= mid && j <= hi)
        arr[k++] = (tmp[i] <= tmp[j]) ? tmp[i++] : tmp[j++];
    while (i <= mid) arr[k++] = tmp[i++];
}

static void merge_sort(int *arr, int *tmp, int lo, int hi) {
    if (lo >= hi) return;
    int mid = lo + (hi - lo) / 2;
    merge_sort(arr, tmp, lo,      mid);
    merge_sort(arr, tmp, mid + 1, hi);
    merge(arr, tmp, lo, mid, hi);
}

static void print_array(const char *label, const int *arr, int n) {
    printf("%s [", label);
    for (int i = 0; i < n; i++)
        printf("%s%d", i ? ", " : "", arr[i]);
    printf("]\n");
}

int main(void) {
    int data[] = { 38, 27, 43, 3, 9, 82, 10, -5, 0, 17 };
    int n      = (int)(sizeof data / sizeof data[0]);
    int *tmp   = malloc((size_t)n * sizeof(int));
    if (!tmp) { fputs("out of memory\n", stderr); return 1; }

    print_array("before:", data, n);
    merge_sort(data, tmp, 0, n - 1);
    print_array("after: ", data, n);

    free(tmp);
    return 0;
}
