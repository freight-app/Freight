# required-features + default-run

Shows two Cargo-style binary-target controls:

- **`required-features`** — `diag` is only linked when its features are active.
- **`default-run`** — `freight run` picks `toolkit` without needing `--bin`.

```sh
freight build                    # → target/debug/toolkit only
freight build --features extras  # → target/debug/toolkit AND target/debug/diag
freight run                      # runs toolkit (default-run)
freight run --bin diag           # needs the diag binary to exist first
```

A target whose `required-features` aren't met is *silently skipped*, not an
error — so optional tooling can come and go without breaking a plain build.
