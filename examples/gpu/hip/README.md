# hip-hello

HIP example built by Freight through the `hipcc` guest compiler template.

**Prerequisites:** ROCm with `hipcc` and `hipconfig` on `PATH`, plus an
AMD GPU supported by the installed ROCm runtime. The bundled template currently
supports Linux on `x86_64`.

```sh
freight check
freight run
```

The `.hip` source activates the `hip` language key. The `hipcc` template declares
`requires_toolchain = ["cpp"]`, so Freight also needs a working C++ host
toolchain for the final link.
