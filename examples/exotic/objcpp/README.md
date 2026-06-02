# objcpp-hello

Objective-C++ example built by Freight through clang++. It mixes Foundation
objects with C++ standard-library containers in one `.mm` translation unit.

**Prerequisites:** macOS with Xcode Command Line Tools. The example links the
Foundation framework through a macOS platform dependency.

```sh
freight check
freight run
```

The source uses `.mm`, so Freight detects the `objcpp` language key
automatically. `[language.objcpp]` demonstrates that Objective-C++ accepts the
same standard strings as the C++ clang template.
