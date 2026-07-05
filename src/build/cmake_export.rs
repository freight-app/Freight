//! Export a freight-built library as a **CMake-discoverable package**.
//!
//! When freight builds a dependency natively but an *external* CMake project
//! needs to `find_package()` it, freight's raw `.a`/`.so` + headers aren't
//! enough — CMake's config mode looks for a `<Name>Config.cmake`, and
//! `pkg_check_modules` looks for a `.pc`. This module writes both into an install
//! prefix so the external build resolves freight's copy:
//!
//! ```text
//! <prefix>/
//!   include/                         (headers — placed by the caller)
//!   lib/                             (.a/.so — placed by the caller)
//!     pkgconfig/<pc_name>.pc
//!     cmake/<CMakeName>/<CMakeName>Config.cmake
//!     cmake/<CMakeName>/<CMakeName>ConfigVersion.cmake
//! ```
//!
//! The `cmake/<CMakeName>/` directory and the config file stem must match the
//! consumer's `find_package(<CMakeName>)` argument (case-sensitive) — that's how
//! CMake locates a config package on `CMAKE_PREFIX_PATH`.

use std::io;
use std::path::Path;

/// What to advertise for a freight-built package.
pub struct ExportSpec<'a> {
    /// The `find_package(<CMakeName>)` name — drives the config dir/file + the
    /// imported target name (`<CMakeName>::<CMakeName>`).
    pub cmake_name: &'a str,
    /// The pkg-config module name (`.pc` file stem). Often the lowercase / distro
    /// name (`zlib`, `libpng`); defaults can pass the same as `cmake_name`.
    pub pc_name: &'a str,
    /// Package version (`"*"`/empty becomes `0`).
    pub version: &'a str,
    /// Names of *other freight-exported* packages this one links (its own
    /// `[dependencies]` that freight provides, not system/pkg-config libs). Each
    /// becomes a `find_dependency(<name>)` in the generated config and, when the
    /// resulting `<name>::<name>` target exists, is linked into this package's
    /// interface — so a static freight lib pulls in its freight deps' archives
    /// at the downstream link. System deps are omitted (their CMake casing is
    /// unknown; the consumer finds them the usual way).
    pub dependencies: &'a [String],
}

/// Write a `.pc` and a `<CMakeName>Config.cmake` (+ version file) into `prefix`,
/// which is expected to already contain `include/` and `lib/` with the built
/// artifacts. Idempotent (overwrites).
pub fn export_cmake_package(prefix: &Path, spec: &ExportSpec) -> io::Result<()> {
    write_pkg_config(prefix, spec)?;
    write_cmake_config(prefix, spec)?;
    Ok(())
}

/// Assemble a complete install prefix for a freight-built dependency and export
/// it: copy each header tree in `include_dirs` into `<prefix>/include/`, copy
/// each built library in `lib_files` into `<prefix>/lib/`, then write the `.pc` +
/// `Config.cmake`. Returns the prefix, ready to drop onto `CMAKE_PREFIX_PATH`.
///
/// This is the bridge from a native freight build (raw `.a`/`.so` + a source
/// `include/` tree) to a CMake/pkg-config-discoverable package an external parent
/// can `find_package()`.
pub fn assemble_export_prefix(
    prefix: &Path,
    include_dirs: &[std::path::PathBuf],
    lib_files: &[std::path::PathBuf],
    spec: &ExportSpec,
) -> io::Result<()> {
    let inc = prefix.join("include");
    std::fs::create_dir_all(&inc)?;
    for dir in include_dirs {
        copy_tree(dir, &inc)?;
    }
    let lib = prefix.join("lib");
    std::fs::create_dir_all(&lib)?;
    for f in lib_files {
        if let Some(name) = f.file_name() {
            std::fs::copy(f, lib.join(name))?;
        }
    }
    export_cmake_package(prefix, spec)
}

/// Recursively copy the contents of `src` into `dst` (merging into existing dirs).
fn copy_tree(src: &Path, dst: &Path) -> io::Result<()> {
    if src.is_file() {
        if let Some(name) = src.file_name() {
            std::fs::copy(src, dst.join(name))?;
        }
        return Ok(());
    }
    if !src.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            std::fs::create_dir_all(&to)?;
            copy_tree(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

fn version_or_zero(v: &str) -> &str {
    let v = v.trim();
    if v.is_empty() || v == "*" {
        "0"
    } else {
        v
    }
}

/// `<prefix>/lib/pkgconfig/<pc_name>.pc`
fn write_pkg_config(prefix: &Path, spec: &ExportSpec) -> io::Result<()> {
    let dir = prefix.join("lib").join("pkgconfig");
    std::fs::create_dir_all(&dir)?;
    let body = format!(
        "prefix={prefix}\n\
         exec_prefix=${{prefix}}\n\
         includedir=${{prefix}}/include\n\
         libdir=${{prefix}}/lib\n\
         \n\
         Name: {name}\n\
         Description: {name} (exported by freight)\n\
         Version: {version}\n\
         Cflags: -I${{includedir}}\n\
         Libs: -L${{libdir}} -l{pc}\n",
        prefix = prefix.display(),
        name = spec.cmake_name,
        version = version_or_zero(spec.version),
        pc = spec.pc_name,
    );
    std::fs::write(dir.join(format!("{}.pc", spec.pc_name)), body)
}

/// `<prefix>/lib/cmake/<CMakeName>/<CMakeName>Config.cmake` (+ version file).
fn write_cmake_config(prefix: &Path, spec: &ExportSpec) -> io::Result<()> {
    let name = spec.cmake_name;
    let dir = prefix.join("lib").join("cmake").join(name);
    std::fs::create_dir_all(&dir)?;

    // Locate each freight dependency before defining the target, so its imported
    // target exists when we link it below. `find_dependency` propagates the
    // outer `find_package`'s REQUIRED/QUIET.
    let mut find_deps = String::new();
    if !spec.dependencies.is_empty() {
        find_deps.push_str("include(CMakeFindDependencyMacro)\n");
        for dep in spec.dependencies {
            find_deps.push_str(&format!("find_dependency({dep})\n"));
        }
    }
    // Link each freight dep's imported target when it materialised (guarded, so a
    // dep that resolved to a differently-cased/system target doesn't error).
    let mut link_deps = String::new();
    for dep in spec.dependencies {
        link_deps.push_str(&format!(
            "\u{20}\u{20}if(TARGET {dep}::{dep})\n\
             \u{20}\u{20}\u{20}\u{20}target_link_libraries({name}::{name} INTERFACE {dep}::{dep})\n\
             \u{20}\u{20}endif()\n",
        ));
    }

    // Resolve the prefix relative to the config file so the package is
    // relocatable: <prefix>/lib/cmake/<name>/ → up three to <prefix>.
    let config = format!(
        "# Generated by freight — exports a freight-built library to CMake.\n\
         {find_deps}\
         get_filename_component(_{name}_prefix \"${{CMAKE_CURRENT_LIST_DIR}}/../../..\" ABSOLUTE)\n\
         set({name}_VERSION \"{version}\")\n\
         set({name}_INCLUDE_DIRS \"${{_{name}_prefix}}/include\")\n\
         file(GLOB {name}_LIBRARIES \"${{_{name}_prefix}}/lib/*.a\" \"${{_{name}_prefix}}/lib/*.so\" \"${{_{name}_prefix}}/lib/*.dylib\")\n\
         if(NOT TARGET {name}::{name})\n\
         \u{20}\u{20}add_library({name}::{name} INTERFACE IMPORTED)\n\
         \u{20}\u{20}target_include_directories({name}::{name} INTERFACE \"${{{name}_INCLUDE_DIRS}}\")\n\
         \u{20}\u{20}target_link_libraries({name}::{name} INTERFACE ${{{name}_LIBRARIES}})\n\
         {link_deps}\
         endif()\n\
         set({name}_FOUND TRUE)\n",
        name = name,
        version = version_or_zero(spec.version),
    );
    std::fs::write(dir.join(format!("{name}Config.cmake")), config)?;

    // A permissive version file so `find_package(<name> <ver>)` is satisfied.
    let version_file = format!(
        "set(PACKAGE_VERSION \"{version}\")\n\
         set(PACKAGE_VERSION_COMPATIBLE TRUE)\n\
         if(\"${{PACKAGE_VERSION}}\" VERSION_EQUAL \"${{PACKAGE_FIND_VERSION}}\")\n\
         \u{20}\u{20}set(PACKAGE_VERSION_EXACT TRUE)\n\
         endif()\n",
        version = version_or_zero(spec.version),
    );
    std::fs::write(dir.join(format!("{name}ConfigVersion.cmake")), version_file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_pc_and_config_keyed_by_cmake_name() {
        let tmp = tempfile::tempdir().unwrap();
        let prefix = tmp.path();
        let spec = ExportSpec {
            cmake_name: "ZLIB",
            pc_name: "zlib",
            version: "1.3.2",
            dependencies: &[],
        };
        export_cmake_package(prefix, &spec).unwrap();

        // pkg-config file is keyed by the pc name.
        let pc = std::fs::read_to_string(prefix.join("lib/pkgconfig/zlib.pc")).unwrap();
        assert!(pc.contains("Name: ZLIB"), "{pc}");
        assert!(pc.contains("Version: 1.3.2"), "{pc}");
        assert!(pc.contains("Libs: -L${libdir} -lzlib"), "{pc}");

        // CMake config is keyed by the find_package name (case-sensitive).
        let cfg = std::fs::read_to_string(prefix.join("lib/cmake/ZLIB/ZLIBConfig.cmake")).unwrap();
        assert!(
            cfg.contains("add_library(ZLIB::ZLIB INTERFACE IMPORTED)"),
            "{cfg}"
        );
        assert!(cfg.contains("set(ZLIB_FOUND TRUE)"), "{cfg}");
        assert!(prefix
            .join("lib/cmake/ZLIB/ZLIBConfigVersion.cmake")
            .is_file());
    }

    #[test]
    fn assemble_copies_headers_and_libs_then_exports() {
        let tmp = tempfile::tempdir().unwrap();
        // A built dep: a header tree + a static lib elsewhere on disk.
        let dep_inc = tmp.path().join("src/include");
        std::fs::create_dir_all(dep_inc.join("bar")).unwrap();
        std::fs::write(dep_inc.join("bar/bar.h"), "int bar(void);\n").unwrap();
        let built_lib = tmp.path().join("target/libbar.a");
        std::fs::create_dir_all(built_lib.parent().unwrap()).unwrap();
        std::fs::write(&built_lib, b"").unwrap();

        let prefix = tmp.path().join("export/bar");
        let spec = ExportSpec {
            cmake_name: "bar",
            pc_name: "bar",
            version: "2.0",
            dependencies: &[],
        };
        assemble_export_prefix(&prefix, &[dep_inc], &[built_lib], &spec).unwrap();

        assert!(prefix.join("include/bar/bar.h").is_file(), "header copied");
        assert!(prefix.join("lib/libbar.a").is_file(), "lib copied");
        assert!(
            prefix.join("lib/cmake/bar/barConfig.cmake").is_file(),
            "config written"
        );
        assert!(prefix.join("lib/pkgconfig/bar.pc").is_file(), "pc written");
    }

    #[test]
    fn dependencies_emit_find_dependency_and_guarded_link() {
        let tmp = tempfile::tempdir().unwrap();
        let spec = ExportSpec {
            cmake_name: "alpha",
            pc_name: "alpha",
            version: "1.0",
            dependencies: &["beta".to_string()],
        };
        export_cmake_package(tmp.path(), &spec).unwrap();
        let cfg =
            std::fs::read_to_string(tmp.path().join("lib/cmake/alpha/alphaConfig.cmake")).unwrap();
        assert!(cfg.contains("include(CMakeFindDependencyMacro)"), "{cfg}");
        assert!(cfg.contains("find_dependency(beta)"), "{cfg}");
        // The dep is linked only when its target materialised.
        assert!(cfg.contains("if(TARGET beta::beta)"), "{cfg}");
        assert!(
            cfg.contains("target_link_libraries(alpha::alpha INTERFACE beta::beta)"),
            "{cfg}"
        );
        // find_dependency must run before the target is defined.
        let find_idx = cfg.find("find_dependency(beta)").unwrap();
        let target_idx = cfg.find("add_library(alpha::alpha").unwrap();
        assert!(
            find_idx < target_idx,
            "find_dependency should precede the target\n{cfg}"
        );
    }

    #[test]
    fn no_dependencies_omits_find_dependency() {
        let tmp = tempfile::tempdir().unwrap();
        let spec = ExportSpec {
            cmake_name: "solo",
            pc_name: "solo",
            version: "1.0",
            dependencies: &[],
        };
        export_cmake_package(tmp.path(), &spec).unwrap();
        let cfg =
            std::fs::read_to_string(tmp.path().join("lib/cmake/solo/soloConfig.cmake")).unwrap();
        assert!(!cfg.contains("find_dependency"), "{cfg}");
        assert!(!cfg.contains("CMakeFindDependencyMacro"), "{cfg}");
    }

    #[test]
    fn empty_version_becomes_zero() {
        let tmp = tempfile::tempdir().unwrap();
        let spec = ExportSpec {
            cmake_name: "foo",
            pc_name: "foo",
            version: "*",
            dependencies: &[],
        };
        export_cmake_package(tmp.path(), &spec).unwrap();
        let cfg =
            std::fs::read_to_string(tmp.path().join("lib/cmake/foo/fooConfig.cmake")).unwrap();
        assert!(cfg.contains("set(foo_VERSION \"0\")"), "{cfg}");
    }
}
