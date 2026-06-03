use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc::Sender;

use docify::extract::{extract_dir, DocItem};

use super::browser::PackageDoc;

// ── Public message type ────────────────────────────────────────────────────────

pub enum StdlibMsg {
    /// Scanning is underway. `label` is the current dir/step; `done`/`total` are dir counts.
    Progress {
        done: usize,
        total: usize,
        label: String,
    },
    /// All packages have been extracted (or loaded from cache).
    Done(Vec<PackageDoc>),
}

// ── Public entry point ─────────────────────────────────────────────────────────

/// Run stdlib scanning/cache-loading in the current thread, reporting progress
/// via `tx`.  Call from a background thread so the TUI stays responsive.
pub fn collect_stdlib(tx: Sender<StdlibMsg>) {
    let pkgs = load_or_scan(&tx);
    let _ = tx.send(StdlibMsg::Done(pkgs));
}

// ── Cache ─────────────────────────────────────────────────────────────────────

fn cache_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".cache"))
        })?;
    let dir = base.join("freight");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("stdlib-docs.msgpack"))
}

fn cache_key() -> String {
    // Use compiler version + rustc version as a cache invalidation key.
    let cpp = compiler_version_string();
    let rust = rust_version();
    format!("{cpp}|{rust}")
}

fn compiler_version_string() -> String {
    for compiler in ["g++", "clang++"] {
        if let Ok(out) = Command::new(compiler).arg("--version").output() {
            if let Ok(s) = String::from_utf8(out.stdout) {
                return s.lines().next().unwrap_or("").trim().to_string();
            }
        }
    }
    String::new()
}

#[derive(serde::Serialize, serde::Deserialize)]
struct CacheFile {
    key: String,
    packages: Vec<CachedPackage>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct CachedPackage {
    name: String,
    version: String,
    items: Vec<DocItem>,
}

fn try_load_cache() -> Option<Vec<PackageDoc>> {
    let path = cache_path()?;
    let bytes = std::fs::read(&path).ok()?;
    let cache: CacheFile = rmp_serde::from_slice(&bytes).ok()?;
    if cache.key != cache_key() {
        return None;
    }
    Some(
        cache
            .packages
            .into_iter()
            .map(|p| PackageDoc {
                name: p.name,
                version: p.version,
                items: p.items,
                readme: None,
            })
            .collect(),
    )
}

fn save_cache(pkgs: &[PackageDoc]) {
    let Some(path) = cache_path() else { return };
    let cache = CacheFile {
        key: cache_key(),
        packages: pkgs
            .iter()
            .map(|p| CachedPackage {
                name: p.name.clone(),
                version: p.version.clone(),
                items: p.items.clone(),
            })
            .collect(),
    };
    if let Ok(bytes) = rmp_serde::to_vec_named(&cache) {
        let _ = std::fs::write(path, bytes);
    }
}

// ── Scan or cache ─────────────────────────────────────────────────────────────

fn load_or_scan(tx: &Sender<StdlibMsg>) -> Vec<PackageDoc> {
    // Try cache first.
    if let Some(pkgs) = try_load_cache() {
        let _ = tx.send(StdlibMsg::Progress {
            done: 1,
            total: 1,
            label: "loaded from cache".to_string(),
        });
        return pkgs;
    }
    // Full scan.
    let mut pkgs = Vec::new();
    pkgs.extend(cpp_stdlib(tx));
    pkgs.extend(rust_stdlib(tx));
    if !pkgs.is_empty() {
        save_cache(&pkgs);
    }
    pkgs
}

// ── C++ ───────────────────────────────────────────────────────────────────────

fn cpp_stdlib(tx: &Sender<StdlibMsg>) -> Vec<PackageDoc> {
    let dirs = cpp_stdlib_dirs();
    if dirs.is_empty() {
        return Vec::new();
    }
    let total = dirs.len();
    let mut items: Vec<DocItem> = Vec::new();
    for (i, dir) in dirs.iter().enumerate() {
        let label = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        let _ = tx.send(StdlibMsg::Progress {
            done: i,
            total,
            label,
        });
        if dir.is_dir() {
            items.extend(extract_dir(dir).items);
        }
    }
    if items.is_empty() {
        return Vec::new();
    }
    vec![PackageDoc {
        name: "C++ Standard Library".to_string(),
        version: String::new(),
        items,
        readme: None,
    }]
}

fn cpp_stdlib_dirs() -> Vec<PathBuf> {
    probe_cpp_include_paths()
        .into_iter()
        .filter(|p| {
            p.is_dir()
                && (p.join("bits").is_dir()
                    || p.join("experimental").is_dir()
                    || p.join("__config").exists()
                    || p.join("vector").exists())
        })
        .collect()
}

fn probe_cpp_include_paths() -> Vec<PathBuf> {
    for compiler in ["g++", "clang++", "c++"] {
        if let Ok(out) = Command::new(compiler)
            .args(["-v", "-x", "c++", "/dev/null", "-fsyntax-only"])
            .output()
        {
            let stderr = String::from_utf8_lossy(&out.stderr);
            if let Some(paths) = parse_include_paths(&stderr) {
                if !paths.is_empty() {
                    return paths;
                }
            }
        }
    }
    fallback_cpp_paths()
}

fn parse_include_paths(stderr: &str) -> Option<Vec<PathBuf>> {
    let start = stderr.find("#include <...> search starts here:")?;
    let end = stderr.find("End of search list.").unwrap_or(stderr.len());
    let paths = stderr[start..end]
        .lines()
        .skip(1)
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| PathBuf::from(l.split_whitespace().next().unwrap_or(l)))
        .collect();
    Some(paths)
}

fn fallback_cpp_paths() -> Vec<PathBuf> {
    let mut c = Vec::new();
    for ver in ["14", "13", "12", "11", "10", "9"] {
        c.push(PathBuf::from(format!("/usr/include/c++/{ver}")));
        c.push(PathBuf::from(format!("/usr/local/include/c++/{ver}")));
    }
    for ver in ["19", "18", "17", "16", "15", "14"] {
        c.push(PathBuf::from(format!("/usr/lib/llvm-{ver}/include/c++/v1")));
    }
    c.push(PathBuf::from("/usr/include/c++/v1"));
    c.into_iter().filter(|p| p.is_dir()).collect()
}

// ── Rust ──────────────────────────────────────────────────────────────────────

fn rust_stdlib(tx: &Sender<StdlibMsg>) -> Vec<PackageDoc> {
    let Some(src) = rust_stdlib_src() else {
        return Vec::new();
    };
    let libs = ["std", "core", "alloc"];
    let mut items: Vec<DocItem> = Vec::new();
    for (i, lib) in libs.iter().enumerate() {
        let _ = tx.send(StdlibMsg::Progress {
            done: i,
            total: libs.len(),
            label: lib.to_string(),
        });
        let dir = src.join(lib).join("src");
        if dir.is_dir() {
            items.extend(extract_dir(&dir).items);
        }
    }
    if items.is_empty() {
        return Vec::new();
    }
    vec![PackageDoc {
        name: "Rust Standard Library".to_string(),
        version: rust_version(),
        items,
        readme: None,
    }]
}

fn rust_stdlib_src() -> Option<PathBuf> {
    let out = Command::new("rustc")
        .args(["--print", "sysroot"])
        .output()
        .ok()?;
    let sysroot = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    let src = sysroot.join("lib/rustlib/src/rust/library");
    if src.is_dir() {
        Some(src)
    } else {
        None
    }
}

fn rust_version() -> String {
    Command::new("rustc")
        .args(["--version"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}
