# Reference build plugins

Reference [build plugins](../docs/manifest-reference.md#plugin-and-build-plugins):
versioned dependencies that run a Rhai script during a consuming project's build
to generate sources. Each is an ordinary package with a `[plugin]` section.

| Plugin | Section | Tool | Output |
|---|---|---|---|
| [`proto`](proto) | `[proto]` | `protoc` | C++ `.pb.cc` + headers from `.proto` |
| [`flatbuffers`](flatbuffers) | `[flatbuffers]` | `flatc` | header-only `*_generated.h` from `.fbs` |
| [`bison`](bison) | `[bison]` | `bison` | C parser `<stem>.tab.c` + `.tab.h` from `.y` |
| [`flex`](flex) | `[flex]` | `flex` | C lexer `<stem>.yy.c` from `.l` |
| [`cmake`](cmake) | `[cmake]` | `cmake` | builds an `external = true` dependency with CMake and links it |

The `cmake` plugin is a different shape from the codegen ones: rather than
generating sources, it builds an `external = true` dependency (`[cmake] build =
"libfoo"`), reading `PKGS["libfoo"].dir` for the source and wiring the installed
headers + libraries back in. It's the first step toward expressing freight's
foreign build-system support as plugins instead of builtin adaptors.

## Using one

```toml
# your freight.toml
[dependencies]
bison = { path = "../path/to/plugins/bison" }   # or a registry version

[bison]   # presence of the handled section activates the plugin
```

Now any `.y` grammar in your project is compiled into the build automatically.
The tool the plugin needs (`bison` here) must be on `PATH`, or pinned via
`[build-dependencies]`.

## Writing your own

A plugin script runs with the project path constants (`PROJECT_DIR`, `SRC_DIR`,
`OUT_DIR`, …), the section config as `CFG`, and functions `glob`, `run`
(restricted to the plugin's declared `tools`), `add_source`, `add_include_dir`,
and `define`. See the scripts here for the pattern; full reference in
[`docs/manifest-reference.md`](../docs/manifest-reference.md).
