# Meson dependency

Builds a local **Meson** library (`vendor/mathlib`) via the `meson` build-system
plugin. The dependency is marked `external = true`, and the `[meson]` section
tells the plugin to build it: it runs `meson setup` / `compile` / `install` and
links the result.

```sh
freight run        # → "7 squared is 49"
```

Requires `meson` and `ninja` on `$PATH`. The companion `deps/cmake` and
`deps/make` examples show the same pattern for those build systems.
