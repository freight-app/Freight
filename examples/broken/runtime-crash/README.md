# broken/runtime-crash

This example **builds and links successfully** but crashes at runtime.
It demonstrates that a clean build does not guarantee a correct program, and
shows how to use freight's debug build + GDB/LLDB to diagnose the crash.

## Running

```
$ freight build
$ freight run          # → crashes with SIGSEGV (null dereference)
$ freight run -- x     # → undefined behaviour (out-of-bounds)
$ freight run -- x x   # → runtime_error exception
```

## Debugging with freight

```
$ freight debug        # opens GDB / LLDB, stops at the crash
(gdb) bt               # show backtrace
```

Build with `--release` first to see that the crash still happens with
optimisations, then re-enable debug symbols to get a useful backtrace:

```
$ freight build --release
$ ./target/release/runtime-crash   # still crashes
$ freight debug                    # debug build: full symbols + backtrace
```
