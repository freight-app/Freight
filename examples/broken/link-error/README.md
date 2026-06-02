# broken/link-error

This example **compiles but fails to link**. `compute_answer` is declared with
`extern` but has no definition anywhere in the project.

## Expected output

```
$ freight build
error: linking link-error
  undefined reference to `compute_answer(int)'
```

This is a common error when a source file is missing, a library is not listed
in `[dependencies]`, or a `[[lib]]` target is not declared.
