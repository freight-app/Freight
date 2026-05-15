# freight

The `freight` CLI binary — the user-facing front-end for the freight build tool and package manager.

All commands delegate to `freight-core` for their heavy lifting. This crate owns:

- **CLI surface** (`clap`) — every subcommand, flag, and dispatch arm in `src/main.rs`
- **Output helpers** (`src/output.rs`) — coloured `✓` / `⚠` / `✗` status lines via `owo-colors`
- **Command implementations** (`src/commands/`) — thin wrappers that call `freight-core` and format results for the terminal
- **Shell completions** (`src/completion.rs`) — dynamic candidates for `freight add`, `freight toolchain use`, etc.

## Commands

| Module | Commands |
|---|---|
| `commands/build.rs` | `build`, `run`, `test`, `bench`, `watch`, `clean` |
| `commands/deps.rs` | `add`, `remove`, `update`, `fetch`, `tree`, `search`, `info`, `login`, `register`, `publish`, `yank` |
| `commands/new.rs` | `new`, `init` |
| `commands/check.rs` | `check` |
| `commands/toolchain.rs` | `toolchain list/add/use` |
| `commands/debug.rs` | `debug` |
| `commands/fmt.rs` | `fmt` |
| `commands/lint.rs` | `lint` |
| `commands/install.rs` | `install`, `package` |
| `commands/doc.rs` | `doc` |
| `commands/compile_commands.rs` | `compile-commands` |

## Building

```sh
cargo build -p freight
cargo install --path .
```

See the [root README](../../README.md) for the full user-facing CLI reference.
