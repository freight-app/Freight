# [patch] — override a dependency's source

The app declares a dependency on `upstream/greeter`, but a root-level `[patch]`
redirects `greeter` to `local/greeter`. `[patch]` applies across the whole
dependency graph, including transitive deps, and is read only from the root
project's manifest.

```sh
freight run
# → "hello from the PATCHED greeter"   (not "UPSTREAM")
```

Patches must be **path** overrides; version/git/archive overrides are rejected at
validation. Patched deps are skipped by `freight fetch` since the source is
already local. Delete the `[patch]` section and re-run to see the upstream
greeting instead.
