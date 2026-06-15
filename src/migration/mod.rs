pub mod autotools;
pub mod cmake;
pub mod make;

use std::collections::BTreeMap;

/// Libraries the compiler driver links automatically — never emitted.
pub(crate) const DRIVER_LINKED: &[&str] = &["c", "gcc", "gcc_s", "stdc++", "c++", "supc++"];

/// Known OS system libraries → the `[os.<os>]` section they belong under. These
/// become `features = [...]` (linked via `-l`), not dependency entries. Shared by
/// all three migrators so they classify link libraries identically.
pub(crate) fn system_lib_os(lib: &str) -> Option<&'static str> {
    match lib {
        "m" | "pthread" | "dl" | "rt" | "atomic" | "util" | "resolv" | "execinfo" | "nsl"
        | "crypt" | "socket" => Some("unix"),
        "ws2_32" | "kernel32" | "user32" | "gdi32" | "shell32" | "ole32" | "oleaut32"
        | "advapi32" | "iphlpapi" | "ntdll" | "dbghelp" | "psapi" | "winmm" | "setupapi"
        | "comctl32" | "comdlg32" | "bcrypt" | "uuid" | "crypt32" | "secur32" | "d3d11"
        | "d3d12" | "dxgi" => Some("windows"),
        _ => None,
    }
}

/// Split a list of linked libraries: real dependencies are pushed to `deps`
/// (compiler-driver libs dropped); OS system libraries go to `features` keyed by
/// their natural `[os.<os>]` section. Both accumulate (deduplicated) so callers
/// can fold in several lists.
pub(crate) fn split_link_libs(
    libs: &[String],
    deps: &mut Vec<String>,
    features: &mut BTreeMap<String, Vec<String>>,
) {
    for lib in libs {
        if DRIVER_LINKED.contains(&lib.as_str()) {
            continue;
        }
        if let Some(os) = system_lib_os(lib) {
            let v = features.entry(os.to_string()).or_default();
            if !v.contains(lib) {
                v.push(lib.clone());
            }
        } else if !deps.contains(lib) {
            deps.push(lib.clone());
        }
    }
}

/// TOML version for a discovered system dependency. The source build files carry
/// no version, so freight asks pkg-config for the installed `--modversion` and
/// pins that. When the library isn't found via pkg-config the version is unknown;
/// it falls back to `"*"` as a draft placeholder, which `freight build` then
/// rejects (freight forbids a bare `*`), prompting the user to pin it.
pub(crate) fn system_dep_item(name: &str) -> toml_edit::Item {
    let version = crate::adaptors::pkg_config_version(name);
    let v = if version.is_empty() { "*" } else { &version };
    toml_edit::value(v)
}

/// Normalise a foreign build-system target name into a freight-safe package
/// name: trim leading/trailing non-alphanumerics, then replace every character
/// that isn't `[A-Za-z0-9_-]` with `-`. Shared by the Make and autotools
/// migrators. (The CMake migrator additionally lower-cases — see
/// `cmake::sanitize_name`.)
pub(crate) fn sanitize_name(s: &str) -> String {
    s.trim_matches(|c: char| !c.is_ascii_alphanumeric())
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}
