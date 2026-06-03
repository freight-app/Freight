# Platform-Conditional Sources

`[os.<name>]` and `[arch.<name>]` sections let you declare source files and
preprocessor defines that are only active on a specific OS or CPU architecture.
Files listed in these sections are excluded from the normal `src/` walk on
non-matching platforms, so the build never sees them.

---

## Manifest syntax

```toml
[os.linux]
sources = ["src/os/linux/**"]
defines = ["POSIX_BUILD", "HAS_EPOLL"]

[os.windows]
sources = ["src/os/windows/**"]
defines = ["WIN32_LEAN_AND_MEAN", "NOMINMAX"]

[os.macos]
sources = ["src/os/macos/**"]
defines = ["TARGET_MACOS"]

[arch.x86_64]
sources = ["src/arch/x86_64/**", "src/asm/*.asm"]
defines = ["HAVE_SSE2"]

[arch.aarch64]
sources = ["src/arch/aarch64/**", "src/asm/*.s"]
defines = ["HAVE_NEON"]
```

`sources` entries are glob patterns resolved relative to the project root.
Both `sources` and `defines` are optional — a section can have either or both.

---

## How it works

### Exclusion set

Before walking `src/`, freight expands every glob from **all** `[os.*]` and
`[arch.*]` sections into a set of concrete file paths. Any file that appears
in this set is excluded from the unconditional walk — regardless of whether
the current platform matches.

### Conditional addition

After the walk, freight expands globs from the **matching** sections only
(`[os.<current_os>]` and `[arch.<current_arch>]`) and appends those files to
the source list. The current arch is `manifest.target.arch` if set, otherwise
`std::env::consts::ARCH`.

```
exclusion_set = expand_globs(all [os.*] and [arch.*] sources)

sources = walk(src/) \ exclusion_set
        + expand_globs([os.<current_os>].sources)
        + expand_globs([arch.<current_arch>].sources)
```

Files not listed in any conditional section pass through the walk unchanged.

### Defines

`defines` from matching sections are merged into the compile invocation
alongside feature defines and profile defines. Non-matching section defines
are never applied.

---

## Recommended layout

```
src/
  main.cpp          ← always compiled
  core.cpp          ← always compiled
  os/
    linux/
      ipc.c
      signal.c
    windows/
      ipc.c
      iocp.c
    macos/
      dispatch.c
  arch/
    x86_64/
      memcpy.cpp
      crc32.cpp
    aarch64/
      memcpy.cpp
      crc32.cpp
  asm/
    memcpy_avx2.asm  ← [arch.x86_64] sources
    memcpy_neon.s    ← [arch.aarch64] sources
```

---

## OS name matching

OS keys are matched case-insensitively against `std::env::consts::OS`. The
values Rust uses: `linux`, `windows`, `macos`, `freebsd`, `openbsd`,
`netbsd`, `dragonfly`, `solaris`, `illumos`, `android`, `ios`.

## Arch name matching

Arch keys are matched case-insensitively against `manifest.target.arch` (if
set) or `std::env::consts::ARCH`. Common values: `x86_64`, `aarch64`, `arm`,
`riscv64`, `wasm32`.

---

## Files touched

| File | Change |
|---|---|
| `Cargo.toml` | Add `glob = "0.3"` workspace dep |
| `crates/freight/Cargo.toml` | Add `glob` |
| `crates/freight/src/manifest/types.rs` | Add `ConditionalSources`; add `os`/`arch` fields to `Manifest` |
| `crates/freight/src/build/discover.rs` | Exclusion set + conditional source addition |
| `crates/freight/src/build/mod.rs` | Collect and inject platform defines |
