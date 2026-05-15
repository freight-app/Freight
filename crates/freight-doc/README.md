# freight-doc

Doc comment extractor and renderer for the freight build tool. Ships as both a library (`freight_doc`) and a standalone binary (`freight-doc`).

## What it does

1. **Extracts** doc comments from source files (C, C++, Fortran, Rust, D, Ada) using a line-scanner in `extract.rs`
2. **Renders** extracted items to Markdown, JSON, or MessagePack via `render_md.rs` / `render_json.rs`
3. **Powers** `freight doc --format …` and the `freight-doc` standalone binary

## Module overview

| File | Responsibility |
|---|---|
| `extract.rs` | Language-aware doc comment scanner; produces `DocItem` structs (name, signature, brief, params, body, returns) |
| `markdown.rs` | Math protection helpers (`$...$`, `$$...$$`) and Markdown utilities |
| `render_md.rs` | GFM Markdown output with per-file pages and an index |
| `render_json.rs` | JSON and MessagePack renderers for tooling/doc apps |
| `lib.rs` | Public API: `extract_docs`, `render_markdown`, `render_json`, `render_msgpack` |
| `main.rs` | `freight-doc` CLI — `--format`, `--out`, `--dry-run` flags |

## Supported doc comment styles

| Language | Styles |
|---|---|
| C / C++ | `/** */`, `/*! */`, `///` — Doxygen `@param`/`@return`/`@brief` |
| Fortran | `!>` block opener, `!!` continuation (FORD conventions) |
| Rust | `///`, `/** */` |
| D | `/++ +/`, `/**`, `///` (DDoc) |
| Ada | `--!`, `---` |

## Standalone usage

```sh
# Extract and render docs from a source tree
freight-doc src/ --format md --out docs/api

# All formats at once
freight-doc src/ --format all

# Dry run — list extracted items without writing
freight-doc src/ --dry-run
```

The `freight doc --format …` command in the main `freight` binary delegates directly to this crate.
