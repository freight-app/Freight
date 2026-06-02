# objc-hello

Minimal Objective-C binary built by Freight through the clang template.

**Prerequisites:** macOS with Xcode Command Line Tools. The example links the
Foundation framework through a macOS platform dependency.

```sh
freight check
freight run
```

The source uses `.m`, so Freight detects the `objc` language key automatically.
The manifest pins `backend = "clang"` to avoid selecting a C compiler without
Objective-C runtime support.
