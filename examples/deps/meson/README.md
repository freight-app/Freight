# Meson dependency

Links a local **Meson** library (`vendor/mathlib`) as a path dependency.
freight auto-detects `meson.build`, runs `meson setup` + `ninja`, and links the
`.a` it finds in the build directory; the `include/` dir is auto-detected.

```sh
freight run        # → "7 squared is 49"
```

Requires `meson` and `ninja` on `$PATH`. The companion `deps/cmake` and
`deps/make` examples show the same pattern for those build systems.
