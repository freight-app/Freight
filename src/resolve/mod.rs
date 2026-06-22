//! Dependency *resolution* — finding already-installed libraries (never building
//! foreign source). These helpers shell out to `pkg-config` (and consult bundled
//! system-lib stubs), not to make/cmake/etc, so they live in freight's core
//! rather than in a build-system plugin.

pub mod build_deps;
pub mod pkg_config;
pub mod pkg_config_cache;
pub mod system_pm;
