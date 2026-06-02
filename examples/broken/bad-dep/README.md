# broken/bad-dep

This example **fails before compilation** because `libdoesnotexist` is not
available via pkg-config, the system stub list, or any configured registry.

## Expected output

```
$ freight build
error: dependency resolution failed
  could not find 'libdoesnotexist': not in pkg-config, stubs, or registry
```

Common fixes:
- Add the package to a freight registry and configure it in `~/.freight/config.toml`
- Replace with a `{ system = "actual_lib_name" }` dep for a system library
- Add a `{ path = "../local-lib" }` dep for a local project
