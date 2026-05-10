# Ideas

## Replace dependency types with a ranked based lookup
The current setup is tat the user can specify backend and path of the dependencies.
At some point it's better to have just the version number and let Freight decide wether to use system libraries or download them from a repository.

an order in which this can be done: 
```
logical dependency
    ↓
registry entry
    ↓
pkg-config / cmake metadata
    ↓
package manager ownership lookup
    ↓
raw probing fallback
```

## System cache registry
Whenever new packages are installed, update an internal registry with the new packages. Only stores libraries and headers.

## Add `support` option to package header in .toml
instead of placing arch and os together, packages can set rules on how packages are supported.

Example (from vcpkg):
```
(windows & !uwp & (x86 | x64)) | (!windows & !osx)
```

This prevents the user from downloading packags that are only meant for a specific architecture or os. This will be usefull for compiling certain triplets.
