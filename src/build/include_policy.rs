//! Include hygiene: classify each `#include` as belonging to the project, a
//! declared dependency, the language standard library, or nothing the project
//! declared (an *undeclared* include).
//!
//! See `docs/include-hygiene.md`. The standard library is the only thing allowed
//! without a declaration, and it is recognised by header **name** (a fixed set
//! per language) rather than by directory — glibc's `stdio.h` and POSIX's
//! `unistd.h` share `/usr/include`, so only the standard header *names* can tell
//! them apart, and the name set is identical on every platform.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Why a header is (or is not) allowed under the include policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IncludeClass {
    /// Resolves inside the project's own sources / include dirs.
    Project,
    /// Resolves inside a declared dependency's exported include dirs.
    Dependency(String),
    /// A language standard-library header (matched by name).
    Stdlib,
    /// Resolves from outside any declared package and is not a stdlib header.
    Undeclared,
}

/// Source language of the translation unit being checked. Determines which
/// standard-header table is consulted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    C,
    Cxx,
}

impl Language {
    /// Best-effort language from a file extension. Defaults to C++ for ambiguous
    /// headers (`.h`) since the C++ table is a superset of the C one.
    pub fn from_path(path: &Path) -> Language {
        match path.extension().and_then(|e| e.to_str()) {
            Some("c") => Language::C,
            // C++, CUDA, ObjC++ and bare headers all use the C++ (superset) table.
            _ => Language::Cxx,
        }
    }
}

/// The set of include roots and standard headers a project is allowed to reach.
pub struct IncludeAllowlist {
    /// Canonicalised project-owned include roots (src/, include/, generated…).
    project_roots: Vec<PathBuf>,
    /// Canonicalised dependency include roots, paired with the dep name.
    dep_roots: Vec<(String, PathBuf)>,
    /// Standard-library header names for the active language.
    std_headers: &'static HashSet<&'static str>,
}

impl IncludeAllowlist {
    /// Construct from already-resolved roots (the wiring layer canonicalises and
    /// supplies these; kept separate so the classification logic is unit-testable
    /// without a real resolver).
    pub fn new(
        language: Language,
        project_roots: Vec<PathBuf>,
        dep_roots: Vec<(String, PathBuf)>,
    ) -> Self {
        let project_roots = project_roots.iter().map(|p| canon(p)).collect();
        let dep_roots = dep_roots.into_iter().map(|(n, p)| (n, canon(&p))).collect();
        IncludeAllowlist {
            project_roots,
            dep_roots,
            std_headers: std_header_set(language),
        }
    }

    /// Classify an include.
    ///
    /// * `header_name` — the spelling **without** delimiters, e.g. `"stdio.h"`,
    ///   `"vector"`, `"foo/bar.h"`.
    /// * `resolved_abs` — the absolute path the include resolved to.
    ///
    /// Directory ownership wins over the name check, so a dependency (or the
    /// project) that legitimately ships a file named like a standard header is
    /// still attributed to it rather than mistaken for the stdlib.
    pub fn classify(&self, header_name: &str, resolved_abs: &Path) -> IncludeClass {
        let resolved = canon(resolved_abs);
        if self.project_roots.iter().any(|r| resolved.starts_with(r)) {
            return IncludeClass::Project;
        }
        for (name, root) in &self.dep_roots {
            if resolved.starts_with(root) {
                return IncludeClass::Dependency(name.clone());
            }
        }
        // Match the standard library by name (the trailing path component for
        // `<sys/types.h>`-style spellings is intentionally *not* used here — POSIX
        // sub-paths are not standard headers).
        if self.std_headers.contains(header_name) {
            return IncludeClass::Stdlib;
        }
        IncludeClass::Undeclared
    }
}

/// Canonicalise a path, falling back to the input on error so classification is
/// still best-effort for not-yet-existing paths.
fn canon(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

// ── #include directive parsing & resolution ──────────────────────────────────

/// What an `import`/`#include` directive brings in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectiveKind {
    /// `#include <h>` / `import "h";` etc. — resolves to a header file.
    Header,
    /// `import foo;` — a C++20 named module. No header to resolve; classified
    /// against the declared module set rather than the include search path.
    Module,
}

/// A parsed `#include` / `import` directive. Positions are 0-based (LSP
/// convention) and span the full token: `<...>` / `"..."` including delimiters
/// for headers, or the module name for a named-module import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncludeDirective {
    /// Header name (without delimiters, e.g. `"stdio.h"`, `"foo/bar.h"`) or the
    /// module name for a `Module` directive (e.g. `"std"`, `"mylib.core"`).
    pub name: String,
    /// `true` for `<...>`, `false` for `"..."`. Always `false` for modules.
    pub angled: bool,
    pub line: u32,
    pub start_col: u32,
    pub end_col: u32,
    /// Whether this directive brings in a header file or a named module.
    pub kind: DirectiveKind,
}

/// Extract `#include` directives from source text, skipping ones inside `//`
/// line comments and `/* … */` block comments (so commented-out includes are
/// not flagged). String-literal edge cases are not handled — `#include` only has
/// meaning at the start of a logical line, which this approximates well enough.
pub fn parse_includes(source: &str) -> Vec<IncludeDirective> {
    let mut out = Vec::new();
    let mut in_block_comment = false;
    for (lineno, raw) in source.lines().enumerate() {
        // Strip block comments, tracking state across lines.
        let line = strip_comments(raw, &mut in_block_comment);
        let trimmed = line.trim_start();
        // Header-bringing directives, all resolving to a header file:
        //   #include <h> / #include "h"
        //   #import  <h> / #import  "h"      (Objective-C)
        //   import <h>; / import "h";        (C++20 header unit; optional `export`)
        // A named-module import (`import foo;`) has no header to resolve and is
        // skipped — it carries no `<...>`/`"..."` token.
        let after_keyword = if let Some(after_hash) = trimmed.strip_prefix('#') {
            let h = after_hash.trim_start();
            h.strip_prefix("include")
                .or_else(|| h.strip_prefix("import"))
        } else {
            let e = trimmed
                .strip_prefix("export")
                .map(str::trim_start)
                .unwrap_or(trimmed);
            e.strip_prefix("import")
        };
        let Some(rest) = after_keyword else { continue };
        // Only the keyword-less `import` form (not `#include`/`#import`) can be a
        // named module — preprocessor includes always carry a header token.
        let is_import_keyword = !trimmed.starts_with('#');
        let rest = rest.trim_start();
        let (open, close, angled) = match rest.chars().next() {
            Some('<') => ('<', '>', true),
            Some('"') => ('"', '"', false),
            // `import foo;` — a named C++20 module, no `<...>`/`"..."` token.
            _ if is_import_keyword => {
                if let Some(d) = parse_named_module(raw, rest, lineno as u32) {
                    out.push(d);
                }
                continue;
            }
            _ => continue,
        };
        let _ = open;
        let inner = &rest[1..];
        let Some(end) = inner.find(close) else {
            continue;
        };
        let name = inner[..end].trim().to_string();
        if name.is_empty() {
            continue;
        }
        // Column of the opening delimiter. `rest` is a suffix slice of the
        // comment-stripped `line` (not `raw`), so the offset is computed against
        // `line`; char positions there match `raw` up to the first comment, and
        // the directive always precedes any comment. Using `raw` here would slice
        // at a byte index that can land inside a multi-byte char (e.g. a non-ASCII
        // comment after the include) and panic.
        let delim_byte = line.len() - rest.len();
        let start_col = line[..delim_byte].chars().count() as u32;
        // Full token width: delimiter + name + closing delimiter.
        let token = &rest[..end + 2]; // <...> or "..."
        let end_col = start_col + token.chars().count() as u32;
        out.push(IncludeDirective {
            name,
            angled,
            line: lineno as u32,
            start_col,
            end_col,
            kind: DirectiveKind::Header,
        });
    }
    out
}

/// Parse a named-module import (`import foo.bar;`) from the text after the
/// `import` keyword. `rest` is the comment-stripped remainder; returns `None`
/// for a partition import (`import :part;`) or empty/malformed names.
fn parse_named_module(raw: &str, rest: &str, lineno: u32) -> Option<IncludeDirective> {
    // Require a terminating `;` so a half-typed `import myli` isn't flagged,
    // mirroring how an unterminated `#include <foo` is skipped.
    if !rest.contains(';') {
        return None;
    }
    let name = rest.split([';', ' ', '\t']).next()?.trim();
    // Reject anything that isn't a dotted identifier: partitions (`:part`),
    // global-module noise, or stray braces.
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '.')
    {
        return None;
    }
    // `rest` is a slice of the comment-stripped copy, so byte arithmetic against
    // `raw` is unsafe (see strip_comments). Locate the name in the original line
    // after the `import` keyword to get a column that lines up with the editor.
    let kw_end = raw.find("import").map(|i| i + "import".len()).unwrap_or(0);
    let start_col = raw[kw_end..]
        .find(name)
        .map(|b| raw[..kw_end + b].chars().count() as u32)
        .unwrap_or(0);
    let end_col = start_col + name.chars().count() as u32;
    Some(IncludeDirective {
        name: name.to_string(),
        angled: false,
        line: lineno,
        start_col,
        end_col,
        kind: DirectiveKind::Module,
    })
}

/// Remove comment content from a single line, carrying `/* */` state across
/// lines via `in_block`. Returns the code-only portion of the line.
fn strip_comments(line: &str, in_block: &mut bool) -> String {
    let bytes = line.as_bytes();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < bytes.len() {
        if *in_block {
            if i + 1 < bytes.len() && bytes[i] == b'*' && bytes[i + 1] == b'/' {
                *in_block = false;
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }
        if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'/' {
            break; // rest of line is a comment
        }
        if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            *in_block = true;
            i += 2;
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// The compiler's built-in system include directories (e.g. `/usr/include`,
/// the libstdc++ dir). Probed by running the compiler's preprocessor in verbose
/// mode; returns an empty list on any failure (callers degrade gracefully — an
/// undeclared system header simply won't be confirmed to exist, so it isn't
/// flagged, which is the safe direction).
pub fn system_include_dirs(compiler: &Path, language: Language) -> Vec<PathBuf> {
    let lang = match language {
        Language::C => "c",
        Language::Cxx => "c++",
    };
    let output = std::process::Command::new(compiler)
        .args(["-E", "-x", lang, "-", "-v"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output();
    match output {
        Ok(o) => parse_search_dirs(&String::from_utf8_lossy(&o.stderr)),
        Err(_) => Vec::new(),
    }
}

/// Parse the `#include <...>`/`"..."` `search starts here:` block out of a
/// compiler's `-v` preprocessor output. A pure parser (no existence filtering —
/// callers that only want real directories filter on `is_dir` themselves).
/// Shared by the build include-hygiene probe and the LSP header-index probe.
pub(crate) fn parse_search_dirs(stderr: &str) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut in_list = false;
    for line in stderr.lines() {
        let t = line.trim();
        if t.starts_with("#include <...> search starts here")
            || t.starts_with("#include \"...\" search starts here")
        {
            in_list = true;
            continue;
        }
        if t.starts_with("End of search list") {
            break;
        }
        if in_list && !t.is_empty() {
            // macOS appends " (framework directory)"; strip any " (...)" suffix.
            let path = t.split(" (").next().unwrap_or(t).trim();
            if !path.is_empty() {
                dirs.push(PathBuf::from(path));
            }
        }
    }
    dirs
}

/// Resolve a `#include` against a set of search directories, returning the
/// absolute path of the first match. For quote includes the file's own
/// directory is searched first (the usual preprocessor rule).
pub fn resolve_include(
    d: &IncludeDirective,
    file_dir: &Path,
    search_dirs: &[PathBuf],
) -> Option<PathBuf> {
    if !d.angled {
        let candidate = file_dir.join(&d.name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    for dir in search_dirs {
        let candidate = dir.join(&d.name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

// ── Orchestration ────────────────────────────────────────────────────────────

/// One undeclared-include finding, ready to be turned into an LSP diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndeclaredInclude {
    pub line: u32,
    pub start_col: u32,
    pub end_col: u32,
    /// The include spelling with delimiters, e.g. `<pthread.h>`.
    pub spelling: String,
}

/// Find every `#include` in `source` that resolves to a header provided by no
/// declared package and is not a language standard-library header.
///
/// * `declared_dirs` — the file's compile-command include dirs (project + deps).
/// * `system_dirs` — the compiler's built-in include dirs (used only to confirm
///   an undeclared header actually exists; a header found nowhere is skipped, as
///   that is clangd's file-not-found, not ours).
pub fn check_includes(
    source: &str,
    file_dir: &Path,
    declared_dirs: &[PathBuf],
    system_dirs: &[PathBuf],
    language: Language,
) -> Vec<UndeclaredInclude> {
    // Declared dirs are the allowlist; resolution also searches system dirs so we
    // can tell "undeclared but present" from "missing". The including file's own
    // directory is always a project root — a quote include resolving next to the
    // source (`#include "util.h"`) is project-local by definition, even when no
    // compile_commands declared an `-I` for it.
    let mut project_roots = declared_dirs.to_vec();
    project_roots.push(file_dir.to_path_buf());
    let allow = IncludeAllowlist::new(language, project_roots, Vec::new());
    let search: Vec<PathBuf> = declared_dirs
        .iter()
        .chain(system_dirs.iter())
        .cloned()
        .collect();

    let mut out = Vec::new();
    for d in parse_includes(source) {
        // Named modules have no header file; they are classified against the
        // declared module set by the LSP layer, not the include search path.
        if d.kind == DirectiveKind::Module {
            continue;
        }
        let Some(resolved) = resolve_include(&d, file_dir, &search) else {
            continue; // not found anywhere — clangd reports file-not-found
        };
        if allow.classify(&d.name, &resolved) == IncludeClass::Undeclared {
            let (l, r) = if d.angled { ("<", ">") } else { ("\"", "\"") };
            out.push(UndeclaredInclude {
                line: d.line,
                start_col: d.start_col,
                end_col: d.end_col,
                spelling: format!("{l}{}{r}", d.name),
            });
        }
    }
    out
}

// ── Standard-header tables ───────────────────────────────────────────────────

/// C standard-library headers (C89 … C23). POSIX/OS headers are deliberately
/// excluded.
const C_HEADERS: &[&str] = &[
    "assert.h",
    "complex.h",
    "ctype.h",
    "errno.h",
    "fenv.h",
    "float.h",
    "inttypes.h",
    "iso646.h",
    "limits.h",
    "locale.h",
    "math.h",
    "setjmp.h",
    "signal.h",
    "stdalign.h",
    "stdarg.h",
    "stdatomic.h",
    "stdbit.h",
    "stdbool.h",
    "stdckdint.h",
    "stddef.h",
    "stdint.h",
    "stdio.h",
    "stdlib.h",
    "stdnoreturn.h",
    "string.h",
    "tgmath.h",
    "threads.h",
    "time.h",
    "uchar.h",
    "wchar.h",
    "wctype.h",
];

/// C++ standard-library headers (C++98 … C++23), excluding the C compatibility
/// headers which are added from `C_HEADERS` and the `c*` list below.
const CXX_HEADERS: &[&str] = &[
    // C library wrappers
    "cassert",
    "ccomplex",
    "cctype",
    "cerrno",
    "cfenv",
    "cfloat",
    "cinttypes",
    "ciso646",
    "climits",
    "clocale",
    "cmath",
    "csetjmp",
    "csignal",
    "cstdalign",
    "cstdarg",
    "cstdbool",
    "cstddef",
    "cstdint",
    "cstdio",
    "cstdlib",
    "cstring",
    "ctgmath",
    "ctime",
    "cuchar",
    "cwchar",
    "cwctype",
    // Containers / sequences
    "array",
    "deque",
    "flat_map",
    "flat_set",
    "forward_list",
    "list",
    "map",
    "mdspan",
    "queue",
    "set",
    "span",
    "stack",
    "unordered_map",
    "unordered_set",
    "vector",
    // Strings / text
    "charconv",
    "format",
    "string",
    "string_view",
    // Algorithms / ranges / numerics
    "algorithm",
    "bit",
    "execution",
    "numbers",
    "numeric",
    "random",
    "ranges",
    "ratio",
    "valarray",
    // Utilities
    "any",
    "bitset",
    "compare",
    "concepts",
    "expected",
    "functional",
    "initializer_list",
    "iterator",
    "memory",
    "memory_resource",
    "optional",
    "scoped_allocator",
    "source_location",
    "stacktrace",
    "tuple",
    "type_traits",
    "typeindex",
    "typeinfo",
    "utility",
    "variant",
    "version",
    // Time / locale / regex
    "chrono",
    "codecvt",
    "locale",
    "regex",
    // Diagnostics / errors
    "exception",
    "stdexcept",
    "system_error",
    // I/O
    "filesystem",
    "fstream",
    "iomanip",
    "ios",
    "iosfwd",
    "iostream",
    "istream",
    "ostream",
    "print",
    "spanstream",
    "sstream",
    "streambuf",
    "strstream",
    "syncstream",
    // Concurrency
    "atomic",
    "barrier",
    "condition_variable",
    "coroutine",
    "future",
    "generator",
    "latch",
    "mutex",
    "semaphore",
    "shared_mutex",
    "stop_token",
    "thread",
    // Language support / misc
    "limits",
    "new",
    "stdfloat",
];

/// The C standard-library header table, in stable order (for completion lists).
pub fn c_std_headers() -> &'static [&'static str] {
    C_HEADERS
}

/// The C++-only standard-library header table, in stable order (for completion
/// lists). Does not include the C `.h` headers — chain [`c_std_headers`] for
/// the full C++ set.
pub fn cxx_std_headers() -> &'static [&'static str] {
    CXX_HEADERS
}

/// Standard headers for `language`. The C++ set includes the C `.h` headers,
/// which remain valid in C++ translation units.
pub fn std_header_set(language: Language) -> &'static HashSet<&'static str> {
    static C_SET: OnceLock<HashSet<&'static str>> = OnceLock::new();
    static CXX_SET: OnceLock<HashSet<&'static str>> = OnceLock::new();
    match language {
        Language::C => C_SET.get_or_init(|| C_HEADERS.iter().copied().collect()),
        Language::Cxx => CXX_SET.get_or_init(|| {
            C_HEADERS
                .iter()
                .chain(CXX_HEADERS.iter())
                .copied()
                .collect()
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowlist() -> IncludeAllowlist {
        IncludeAllowlist::new(
            Language::Cxx,
            vec![PathBuf::from("/proj/src"), PathBuf::from("/proj/include")],
            vec![("zlib".into(), PathBuf::from("/deps/zlib/include"))],
        )
    }

    #[test]
    fn stdlib_recognised_by_name_regardless_of_path() {
        let a = allowlist();
        // A real toolchain path; only the *name* matters.
        assert_eq!(
            a.classify("vector", Path::new("/usr/include/c++/13/vector")),
            IncludeClass::Stdlib
        );
        assert_eq!(
            a.classify("stdio.h", Path::new("/usr/include/stdio.h")),
            IncludeClass::Stdlib
        );
    }

    #[test]
    fn posix_and_os_headers_are_undeclared() {
        let a = allowlist();
        // POSIX / OS headers are NOT standard-library headers.
        assert_eq!(
            a.classify("pthread.h", Path::new("/usr/include/pthread.h")),
            IncludeClass::Undeclared
        );
        assert_eq!(
            a.classify("unistd.h", Path::new("/usr/include/unistd.h")),
            IncludeClass::Undeclared
        );
    }

    #[test]
    fn undeclared_third_party_header() {
        let a = allowlist();
        assert_eq!(
            a.classify("openssl/ssl.h", Path::new("/usr/include/openssl/ssl.h")),
            IncludeClass::Undeclared
        );
    }

    #[test]
    fn project_and_dependency_paths_win_over_name() {
        let a = allowlist();
        assert_eq!(
            a.classify("widget.h", Path::new("/proj/src/widget.h")),
            IncludeClass::Project
        );
        assert_eq!(
            a.classify("zlib.h", Path::new("/deps/zlib/include/zlib.h")),
            IncludeClass::Dependency("zlib".into())
        );
        // A dependency file named like a std header is still attributed to the dep.
        assert_eq!(
            a.classify("vector", Path::new("/deps/zlib/include/vector")),
            IncludeClass::Dependency("zlib".into())
        );
    }

    #[test]
    fn c_language_excludes_cxx_headers() {
        let a = IncludeAllowlist::new(Language::C, vec![], vec![]);
        assert_eq!(
            a.classify("stdio.h", Path::new("/usr/include/stdio.h")),
            IncludeClass::Stdlib
        );
        // <vector> is not a C standard header.
        assert_eq!(
            a.classify("vector", Path::new("/usr/include/c++/13/vector")),
            IncludeClass::Undeclared
        );
    }

    #[test]
    fn parse_includes_extracts_directives() {
        let src = "\
#include <stdio.h>
#  include \"foo/bar.h\"
int x;
// #include <commented_out.h>
/* #include <block.h> */
#include <zlib.h>  // trailing comment
";
        let incs = parse_includes(src);
        let names: Vec<_> = incs
            .iter()
            .map(|d| (d.name.as_str(), d.angled, d.line))
            .collect();
        assert_eq!(
            names,
            vec![
                ("stdio.h", true, 0),
                ("foo/bar.h", false, 1),
                ("zlib.h", true, 5)
            ]
        );
        // The <stdio.h> token spans columns 9..18 (0-based, end exclusive).
        assert_eq!(incs[0].start_col, 9);
        assert_eq!(incs[0].end_col, 18);
    }

    #[test]
    fn parse_includes_handles_import_and_objc_forms() {
        let src = "\
#import <Foundation/Foundation.h>
import <pthread.h>;
export import \"mylib.h\";
import std;
import mylib.core;
int importance = 5;
";
        let got: Vec<_> = parse_includes(src)
            .iter()
            .map(|d| (d.name.clone(), d.angled, d.line, d.kind))
            .collect();
        assert_eq!(
            got,
            vec![
                (
                    "Foundation/Foundation.h".to_string(),
                    true,
                    0,
                    DirectiveKind::Header
                ),
                ("pthread.h".to_string(), true, 1, DirectiveKind::Header),
                ("mylib.h".to_string(), false, 2, DirectiveKind::Header),
                // Named modules are captured as Module directives (resolved
                // against the declared module set, not the include path).
                ("std".to_string(), false, 3, DirectiveKind::Module),
                ("mylib.core".to_string(), false, 4, DirectiveKind::Module),
            ]
        );
        // `int importance = 5;` is not an import and must be ignored.
        assert!(!got.iter().any(|(n, ..)| n == "importance"));
    }

    #[test]
    fn parse_named_module_rejects_partitions_and_noise() {
        // Partition imports (`import :part;`) and module-declaration lines are
        // not whole-module imports and must not be captured as Module directives.
        let src = "\
import :part;
module;
export module mylib;
import   spaced.mod  ;
";
        let mods: Vec<_> = parse_includes(src)
            .iter()
            .filter(|d| d.kind == DirectiveKind::Module)
            .map(|d| d.name.clone())
            .collect();
        // `module mylib;` after `export` is not an `import`, so it isn't here;
        // only the well-formed `import spaced.mod;` is captured.
        assert_eq!(mods, vec!["spaced.mod".to_string()]);
    }

    #[test]
    fn parse_includes_handles_non_ascii_comment() {
        // A multi-byte char after the directive must not break column math
        // (regression: `raw.len() - rest.len()` sliced inside the em-dash).
        let src = "#include <pthread.h> /* platform — needs a dep */\n";
        let incs = parse_includes(src);
        assert_eq!(incs.len(), 1);
        assert_eq!(incs[0].name, "pthread.h");
        // `<pthread.h>` starts at column 9 and spans 11 chars.
        assert_eq!(incs[0].start_col, 9);
        assert_eq!(incs[0].end_col, 20);
    }

    #[test]
    fn parse_includes_skips_multiline_block_comment() {
        let src = "/* opening\n#include <hidden.h>\nstill comment */\n#include <real.h>\n";
        let incs = parse_includes(src);
        assert_eq!(incs.len(), 1);
        assert_eq!(incs[0].name, "real.h");
    }

    #[test]
    fn check_includes_flags_only_undeclared_present_headers() {
        let tmp = tempfile::tempdir().unwrap();
        let tmp = tmp.path();
        let declared = tmp.join("deps/zlib/include");
        let system = tmp.join("usr/include");
        let filedir = tmp.join("proj/src");
        for d in [&declared, &system, &filedir] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(declared.join("zlib.h"), "").unwrap();
        std::fs::write(system.join("pthread.h"), "").unwrap();
        std::fs::write(system.join("stdio.h"), "").unwrap();
        std::fs::create_dir_all(system.join("c++")).unwrap();
        std::fs::write(system.join("c++/vector"), "").unwrap();

        let src = "\
#include <zlib.h>
#include <pthread.h>
#include <stdio.h>
#include <vector>
#include <does_not_exist.h>
";
        let mut sys_dirs = vec![system.clone()];
        sys_dirs.push(system.join("c++"));
        let found = check_includes(src, &filedir, &[declared.clone()], &sys_dirs, Language::Cxx);

        // Only <pthread.h> is undeclared *and* present: zlib is declared, stdio &
        // vector are stdlib, and does_not_exist.h resolves nowhere.
        assert_eq!(found.len(), 1, "got {found:?}");
        assert_eq!(found[0].spelling, "<pthread.h>");
        assert_eq!(found[0].line, 1);
    }

    #[test]
    fn header_next_to_source_is_project_local() {
        // A quote include resolving to the source file's own directory must not
        // be flagged even with no declared dirs (the no-compile_commands case).
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("util.h"), "").unwrap();
        let found = check_includes("#include \"util.h\"\n", &src, &[], &[], Language::Cxx);
        assert!(found.is_empty(), "header next to source flagged: {found:?}");
    }

    #[test]
    fn parse_search_dirs_extracts_system_block() {
        let stderr = "\
ignored preamble
#include \"...\" search starts here:
 /proj/local
#include <...> search starts here:
 /usr/lib/clang/18/include
 /usr/include/c++/13
 /usr/include (framework directory)
End of search list.
trailing junk
";
        let dirs = parse_search_dirs(stderr);
        assert_eq!(
            dirs,
            vec![
                PathBuf::from("/proj/local"),
                PathBuf::from("/usr/lib/clang/18/include"),
                PathBuf::from("/usr/include/c++/13"),
                PathBuf::from("/usr/include"),
            ]
        );
    }

    #[test]
    fn resolve_include_searches_quote_dir_then_search_path() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("proj");
        let dep = tmp.path().join("dep");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::create_dir_all(&dep).unwrap();
        std::fs::write(proj.join("local.h"), "").unwrap();
        std::fs::write(dep.join("lib.h"), "").unwrap();

        let quote = IncludeDirective {
            name: "local.h".into(),
            angled: false,
            line: 0,
            start_col: 0,
            end_col: 0,
            kind: DirectiveKind::Header,
        };
        assert_eq!(
            resolve_include(&quote, &proj, &[dep.clone()]),
            Some(proj.join("local.h"))
        );

        let angle = IncludeDirective {
            name: "lib.h".into(),
            angled: true,
            line: 0,
            start_col: 0,
            end_col: 0,
            kind: DirectiveKind::Header,
        };
        assert_eq!(
            resolve_include(&angle, &proj, &[dep.clone()]),
            Some(dep.join("lib.h"))
        );

        let missing = IncludeDirective {
            name: "nope.h".into(),
            angled: true,
            line: 0,
            start_col: 0,
            end_col: 0,
            kind: DirectiveKind::Header,
        };
        assert_eq!(resolve_include(&missing, &proj, &[dep]), None);
    }
}
