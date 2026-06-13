# broken/undeclared-include-owned

Demonstrates include-hygiene Phase 3 header ownership. `zlib` is declared, so
`<zlib.h>` (which lives bare in `/usr/include`) is attributed to it and allowed.
`<pthread.h>` is provided by no declared dependency and is still rejected under
`[lints].undeclared-include = "deny"`.
