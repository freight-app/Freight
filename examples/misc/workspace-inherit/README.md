# Workspace inheritance

Members inherit shared definitions from the workspace root with the
`workspace = true` marker (mirrors Cargo):

- **`[workspace.package]`** → `version.workspace = true`, `license.workspace = true`
- **`[workspace.dependencies]`** → `greeter = { workspace = true }`

```sh
cd app
freight run                 # → "hello from the workspace greeter library"
freight metadata | grep version   # both packages report the inherited 1.2.0
```

A `path` in `[workspace.dependencies]` is written relative to the **root**;
freight rewrites it per member, so `app` (in a subdirectory) still finds the
sibling `greeter` library. Inheritance is resolved before parsing, so the typed
manifest the build sees already has concrete values.
