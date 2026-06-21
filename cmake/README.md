# Consuming Freight packages from other build systems

`freight install --prefix <P>` lays out a standard prefix and now also emits a
**pkg-config descriptor** (`<P>/lib/pkgconfig/<name>.pc`). That single file makes
a Freight library consumable by essentially every build system, and this
directory ships a `Freight.cmake` helper for the idiomatic CMake path.

```
<P>/
├── include/<name>/…        # public headers
├── lib/lib<name>.a|.so     # the library (+ SONAME symlinks for shared)
└── lib/pkgconfig/<name>.pc # pkg-config descriptor (Name/Version/Cflags/Libs)
```

## CMake — `Freight.cmake`

```cmake
list(APPEND CMAKE_MODULE_PATH "/path/to/freight/cmake")
include(Freight)

# Build a local Freight project on the fly and import it:
freight_dependency(mylib SOURCE_DIR ${CMAKE_SOURCE_DIR}/../mylib REQUIRED)

# …or import an already-installed package (found via pkg-config / a prefix):
freight_dependency(otherlib PREFIX /opt/freight)

target_link_libraries(myapp PRIVATE freight::mylib)
```

`freight_dependency(<name> …)` options:

| Option | Meaning |
|---|---|
| `SOURCE_DIR <dir>` | A Freight project to `freight install` into the build tree at configure time. |
| `PREFIX <dir>` | Install / lookup prefix (default `${CMAKE_BINARY_DIR}/_freight/<name>`). |
| `RELEASE` / `DEBUG` | Profile to install (default: follows `CMAKE_BUILD_TYPE`, else release). |
| `FEATURES <f…>` | Freight features to activate (`freight install --features`). Only applies when building (`SOURCE_DIR`). |
| `NO_DEFAULT_FEATURES` | Pass `--no-default-features` to the build. |
| `STATIC` / `SHARED` | Link preference for the direct-import fallback. |
| `ALIAS <target>` | Extra alias in addition to `freight::<name>`. |
| `REQUIRED` | `FATAL_ERROR` if the package can't be imported. |

Enable features on a built dependency:

```cmake
freight_dependency(mylib SOURCE_DIR ../mylib FEATURES tls zlib NO_DEFAULT_FEATURES REQUIRED)
```

It prefers the emitted `.pc` (via `pkg_check_modules`, so transitive
`Requires.private` and flags come along); if pkg-config is unavailable it falls
back to importing the artifact directly from the installed layout. The `freight`
CLI is found on `PATH` (or set `-DFREIGHT_EXECUTABLE=/path/to/freight`).

## Meson

```meson
mylib = dependency('mylib')   # set PKG_CONFIG_PATH to <P>/lib/pkgconfig
executable('app', 'main.c', dependencies: mylib)
```

## Autotools

```m4
PKG_CHECK_MODULES([MYLIB], [mylib >= 1.2])
# AM_CFLAGS += $(MYLIB_CFLAGS) ;  app_LDADD = $(MYLIB_LIBS)
```

## Plain Makefile

```make
CFLAGS  += $(shell pkg-config --cflags mylib)
LDLIBS  += $(shell pkg-config --libs mylib)
```

In every case, point pkg-config at the install:

```sh
export PKG_CONFIG_PATH=<P>/lib/pkgconfig:$PKG_CONFIG_PATH
```

(If you installed to the default `/usr/local`, pkg-config usually finds it
without `PKG_CONFIG_PATH`.)
