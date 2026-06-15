# Command aliases

`.freight/config.toml` defines `[alias]` shortcuts (mirrors Cargo). Aliases are
read from the global `~/.freight/config.toml` merged with this project-local
file (local wins).

```sh
freight b     # → freight build
freight br    # → freight build --release
freight r     # → freight run   → "built via an alias!"
```

A string alias is split on whitespace; an array is taken verbatim. An alias can
never shadow a built-in subcommand, and expansion is single-pass.
