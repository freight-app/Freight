/// Documentation extraction and rendering.
///
/// Supports C/C++ (Doxygen `/** */`, `/*! */`, `///`), Rust (`///`, `/** */`),
/// Fortran (`!>` / `!!`), D (`/++`, `/**`, `///`) and Ada (`--!` / `---`).
pub mod extract;
pub mod render;
