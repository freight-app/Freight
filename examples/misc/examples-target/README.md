# [[example]] targets

A static library plus runnable example programs, mirroring Cargo's `examples/`.

- `examples/basic.c` is **auto-discovered** — example name is the file stem `basic`.
- `examples/fancy_demo.c` is named `fancy` via an explicit `[[example]]` section.

Examples are **not** built by a plain `freight build`; build/run them explicitly:

```sh
freight build --examples        # build all → target/dev/examples/{basic,fancy}
freight build --example fancy    # build just one
freight run   --example basic    # build + run → "2 + 3 = 5"
freight run   --example fancy    # → "(2 + 3) * 4 = 20"
```

Each example links against the project library (`mathx_add` / `mathx_mul`),
just like a test or benchmark.
