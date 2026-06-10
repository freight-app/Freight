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

/// A parsed `#include` directive. Positions are 0-based (LSP convention) and
/// span the full `<...>` / `"..."` token including delimiters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncludeDirective {
    /// Header name as written, without delimiters, e.g. `"stdio.h"`, `"foo/bar.h"`.
    pub name: String,
    /// `true` for `<...>`, `false` for `"..."`.
    pub angled: bool,
    pub line: u32,
    pub start_col: u32,
    pub end_col: u32,
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
        if !trimmed.starts_with('#') {
            continue;
        }
        // Allow whitespace between '#' and 'include'.
        let after_hash = trimmed[1..].trim_start();
        let Some(rest) = after_hash.strip_prefix("include") else { continue };
        let rest = rest.trim_start();
        let (open, close, angled) = match rest.chars().next() {
            Some('<') => ('<', '>', true),
            Some('"') => ('"', '"', false),
            _ => continue,
        };
        let _ = open;
        let inner = &rest[1..];
        let Some(end) = inner.find(close) else { continue };
        let name = inner[..end].trim().to_string();
        if name.is_empty() {
            continue;
        }
        // Column of the opening delimiter within the original raw line.
        let delim_byte = raw.len() - rest.len();
        let start_col = raw[..delim_byte].chars().count() as u32;
        // Full token width: delimiter + name + closing delimiter.
        let token = &rest[..end + 2]; // <...> or "..."
        let end_col = start_col + token.chars().count() as u32;
        out.push(IncludeDirective {
            name,
            angled,
            line: lineno as u32,
            start_col,
            end_col,
        });
    }
    out
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

// ── Standard-header tables ───────────────────────────────────────────────────

/// C standard-library headers (C89 … C23). POSIX/OS headers are deliberately
/// excluded.
const C_HEADERS: &[&str] = &[
    "assert.h", "complex.h", "ctype.h", "errno.h", "fenv.h", "float.h",
    "inttypes.h", "iso646.h", "limits.h", "locale.h", "math.h", "setjmp.h",
    "signal.h", "stdalign.h", "stdarg.h", "stdatomic.h", "stdbit.h", "stdbool.h",
    "stdckdint.h", "stddef.h", "stdint.h", "stdio.h", "stdlib.h", "stdnoreturn.h",
    "string.h", "tgmath.h", "threads.h", "time.h", "uchar.h", "wchar.h",
    "wctype.h",
];

/// C++ standard-library headers (C++98 … C++23), excluding the C compatibility
/// headers which are added from `C_HEADERS` and the `c*` list below.
const CXX_HEADERS: &[&str] = &[
    // C library wrappers
    "cassert", "ccomplex", "cctype", "cerrno", "cfenv", "cfloat", "cinttypes",
    "ciso646", "climits", "clocale", "cmath", "csetjmp", "csignal", "cstdalign",
    "cstdarg", "cstdbool", "cstddef", "cstdint", "cstdio", "cstdlib", "cstring",
    "ctgmath", "ctime", "cuchar", "cwchar", "cwctype",
    // Containers / sequences
    "array", "deque", "flat_map", "flat_set", "forward_list", "list", "map",
    "mdspan", "queue", "set", "span", "stack", "unordered_map", "unordered_set",
    "vector",
    // Strings / text
    "charconv", "format", "string", "string_view",
    // Algorithms / ranges / numerics
    "algorithm", "bit", "execution", "numbers", "numeric", "random", "ranges",
    "ratio", "valarray",
    // Utilities
    "any", "bitset", "compare", "concepts", "expected", "functional",
    "initializer_list", "iterator", "memory", "memory_resource", "optional",
    "scoped_allocator", "source_location", "stacktrace", "tuple", "type_traits",
    "typeindex", "typeinfo", "utility", "variant", "version",
    // Time / locale / regex
    "chrono", "codecvt", "locale", "regex",
    // Diagnostics / errors
    "exception", "stdexcept", "system_error",
    // I/O
    "filesystem", "fstream", "iomanip", "ios", "iosfwd", "iostream", "istream",
    "ostream", "print", "spanstream", "sstream", "streambuf", "strstream",
    "syncstream",
    // Concurrency
    "atomic", "barrier", "condition_variable", "coroutine", "future",
    "generator", "latch", "mutex", "semaphore", "shared_mutex", "stop_token",
    "thread",
    // Language support / misc
    "limits", "new", "stdfloat",
];

/// Standard headers for `language`. The C++ set includes the C `.h` headers,
/// which remain valid in C++ translation units.
pub fn std_header_set(language: Language) -> &'static HashSet<&'static str> {
    static C_SET: OnceLock<HashSet<&'static str>> = OnceLock::new();
    static CXX_SET: OnceLock<HashSet<&'static str>> = OnceLock::new();
    match language {
        Language::C => C_SET.get_or_init(|| C_HEADERS.iter().copied().collect()),
        Language::Cxx => CXX_SET.get_or_init(|| {
            C_HEADERS.iter().chain(CXX_HEADERS.iter()).copied().collect()
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
        assert_eq!(a.classify("vector", Path::new("/usr/include/c++/13/vector")), IncludeClass::Stdlib);
        assert_eq!(a.classify("stdio.h", Path::new("/usr/include/stdio.h")), IncludeClass::Stdlib);
    }

    #[test]
    fn posix_and_os_headers_are_undeclared() {
        let a = allowlist();
        // POSIX / OS headers are NOT standard-library headers.
        assert_eq!(a.classify("pthread.h", Path::new("/usr/include/pthread.h")), IncludeClass::Undeclared);
        assert_eq!(a.classify("unistd.h", Path::new("/usr/include/unistd.h")), IncludeClass::Undeclared);
    }

    #[test]
    fn undeclared_third_party_header() {
        let a = allowlist();
        assert_eq!(a.classify("openssl/ssl.h", Path::new("/usr/include/openssl/ssl.h")), IncludeClass::Undeclared);
    }

    #[test]
    fn project_and_dependency_paths_win_over_name() {
        let a = allowlist();
        assert_eq!(a.classify("widget.h", Path::new("/proj/src/widget.h")), IncludeClass::Project);
        assert_eq!(a.classify("zlib.h", Path::new("/deps/zlib/include/zlib.h")), IncludeClass::Dependency("zlib".into()));
        // A dependency file named like a std header is still attributed to the dep.
        assert_eq!(a.classify("vector", Path::new("/deps/zlib/include/vector")), IncludeClass::Dependency("zlib".into()));
    }

    #[test]
    fn c_language_excludes_cxx_headers() {
        let a = IncludeAllowlist::new(Language::C, vec![], vec![]);
        assert_eq!(a.classify("stdio.h", Path::new("/usr/include/stdio.h")), IncludeClass::Stdlib);
        // <vector> is not a C standard header.
        assert_eq!(a.classify("vector", Path::new("/usr/include/c++/13/vector")), IncludeClass::Undeclared);
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
        let names: Vec<_> = incs.iter().map(|d| (d.name.as_str(), d.angled, d.line)).collect();
        assert_eq!(
            names,
            vec![("stdio.h", true, 0), ("foo/bar.h", false, 1), ("zlib.h", true, 5)]
        );
        // The <stdio.h> token spans columns 9..18 (0-based, end exclusive).
        assert_eq!(incs[0].start_col, 9);
        assert_eq!(incs[0].end_col, 18);
    }

    #[test]
    fn parse_includes_skips_multiline_block_comment() {
        let src = "/* opening\n#include <hidden.h>\nstill comment */\n#include <real.h>\n";
        let incs = parse_includes(src);
        assert_eq!(incs.len(), 1);
        assert_eq!(incs[0].name, "real.h");
    }

    #[test]
    fn resolve_include_searches_quote_dir_then_search_path() {
        let tmp = std::env::temp_dir().join(format!("inc_policy_{}", std::process::id()));
        let proj = tmp.join("proj");
        let dep = tmp.join("dep");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::create_dir_all(&dep).unwrap();
        std::fs::write(proj.join("local.h"), "").unwrap();
        std::fs::write(dep.join("lib.h"), "").unwrap();

        let quote = IncludeDirective { name: "local.h".into(), angled: false, line: 0, start_col: 0, end_col: 0 };
        assert_eq!(resolve_include(&quote, &proj, &[dep.clone()]), Some(proj.join("local.h")));

        let angle = IncludeDirective { name: "lib.h".into(), angled: true, line: 0, start_col: 0, end_col: 0 };
        assert_eq!(resolve_include(&angle, &proj, &[dep.clone()]), Some(dep.join("lib.h")));

        let missing = IncludeDirective { name: "nope.h".into(), angled: true, line: 0, start_col: 0, end_col: 0 };
        assert_eq!(resolve_include(&missing, &proj, &[dep]), None);

        std::fs::remove_dir_all(&tmp).ok();
    }
}
