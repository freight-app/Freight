# broken/undeclared-include

`src/main.c` includes `<pthread.h>`, a platform header that **no declared
dependency provides**. With `[lints].undeclared-include = "deny"` freight's
include-hygiene pass (Phase 2) blocks the build before invoking the compiler.

`<stdio.h>` is a standard-library header and is intentionally *not* flagged.

To build it, either declare the dependency that provides `<pthread.h>` (e.g. a
`system`/pkg-config dep) or relax the lint to `"warn"` / `"allow"`.
