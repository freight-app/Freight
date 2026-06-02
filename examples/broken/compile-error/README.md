# broken/compile-error

This example **intentionally does not compile**. It demonstrates the error output
freight produces when a C++ source file has syntax errors.

## Expected output

```
$ freight build
error: building compile-error
  --> src/main.cpp:9:1
   |  missing ';' after struct definition
  --> src/main.cpp:15:1
   |  use of undeclared identifier 'undefined_z'
```

Run `freight build` and observe that freight reports each compiler diagnostic with
file name, line number, and message.
