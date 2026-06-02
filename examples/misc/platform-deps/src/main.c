#include <stdio.h>
#include <math.h>

#ifdef PLATFORM_LINUX
#  include <unistd.h>
#  include <sys/utsname.h>
#elif defined(PLATFORM_MACOS)
#  include <unistd.h>
#  include <sys/utsname.h>
#endif

static void print_platform(void) {
#if defined(PLATFORM_LINUX)
    printf("OS: Linux\n");
#elif defined(PLATFORM_MACOS)
    printf("OS: macOS\n");
#elif defined(PLATFORM_WINDOWS)
    printf("OS: Windows\n");
#else
    printf("OS: unknown\n");
#endif

#ifdef PLATFORM_UNIX
    printf("Family: unix\n");
#endif
}

static void print_kernel(void) {
#if defined(PLATFORM_LINUX) || defined(PLATFORM_MACOS)
    struct utsname u;
    if (uname(&u) == 0) {
        printf("Kernel: %s %s\n", u.sysname, u.release);
    }
#endif
}

int main(void) {
    print_platform();
    print_kernel();

    /* libm linked via [os.linux] dependencies */
    double x = 2.0;
    printf("sqrt(%.1f) = %.6f\n", x, sqrt(x));
    return 0;
}
