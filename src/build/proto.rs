//! protobuf code generation via `protoc`.
//!
//! When `[language.proto]` is declared in `freight.toml`, freight discovers
//! `.proto` files under `src/`, runs `protoc --cpp_out=<dest>` on each, and
//! injects the generated `.pb.cc` sources into the compile list for the normal
//! C++ compilation step.
//!
//! # Manifest example
//!
//! ```toml
//! [language.proto]
//! # Directory for generated C++ files.  Default: target/<profile>/proto-gen/
//! # dest = "src/generated"
//!
//! # Extra --proto_path roots beyond src/ and the project root (comma-separated).
//! # srcs = "proto/"
//!
//! # Extra flags forwarded verbatim to protoc (whitespace-separated).
//! # extra_flags = "--experimental_allow_proto3_optional"
//! ```
//!
//! `protoc` is resolved from `tool_paths` first (populated by `[build-dependencies]`
//! entries like `protoc = { url = "…", type = "none" }`), then from the system PATH.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use crate::build::discover::SourceFile;
use crate::error::FreightError;
use crate::event::{BuildEvent, Progress};
use crate::manifest::types::LanguageSettings;

// ── Public types ──────────────────────────────────────────────────────────────

/// Output of the proto codegen step.
pub struct ProtoGenResult {
    /// Generated `.pb.cc` source files to be compiled as C++.
    pub generated_sources: Vec<SourceFile>,
    /// Directory containing generated `.pb.h` headers (added to include path).
    /// Empty `PathBuf` when no proto files were found / nothing was generated.
    pub generated_include_dir: PathBuf,
}

// ── Entry point ───────────────────────────────────────────────────────────────

/// Run `protoc` on all `.proto` files found under `src/`, emitting C++ sources
/// into `target/<profile>/proto-gen/` (or the `dest =` directory).
///
/// Returns the generated `.pb.cc` source files and the output directory.
/// Files whose output is already up-to-date (output newer than source) are skipped.
///
/// `tool_paths` is prepended to PATH so a build-dep `protoc` takes precedence
/// over any system installation.
pub fn run_protoc(
    project_dir: &Path,
    profile: &str,
    settings: &LanguageSettings,
    tool_paths: &[PathBuf],
    progress: &Progress,
) -> Result<ProtoGenResult, FreightError> {
    let proto_files = discover_proto_files(project_dir);
    if proto_files.is_empty() {
        return Ok(ProtoGenResult {
            generated_sources: vec![],
            generated_include_dir: PathBuf::new(),
        });
    }

    // ── Output directory ──────────────────────────────────────────────────────

    let out_dir = {
        let rel = settings.extra.get("dest")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("target").join(profile).join("proto-gen"));
        if rel.is_absolute() { rel } else { project_dir.join(rel) }
    };
    std::fs::create_dir_all(&out_dir)?;

    // ── --proto_path roots ────────────────────────────────────────────────────
    //
    // Default roots: src/ and project root.
    // Extra roots come from `srcs = "proto/"` (comma-separated).

    let mut proto_paths: Vec<PathBuf> = vec![
        project_dir.join("src"),
        project_dir.to_path_buf(),
    ];
    if let Some(extra) = settings.extra.get("srcs") {
        for p in extra.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            proto_paths.push(project_dir.join(p));
        }
    }

    // ── Resolve protoc binary ─────────────────────────────────────────────────

    let protoc_bin = resolve_bin("protoc", tool_paths);

    let cpp_out_arg = format!("--cpp_out={}", out_dir.display());
    let proto_path_flags: Vec<String> = proto_paths.iter()
        .filter(|p| p.is_dir())
        .map(|p| format!("--proto_path={}", p.display()))
        .collect();

    // ── Run protoc per file (incremental) ─────────────────────────────────────

    for proto_rel in &proto_files {
        let abs = project_dir.join(proto_rel);

        let stem = abs.file_stem().and_then(|s| s.to_str()).unwrap_or("unknown");
        let pb_cc = out_dir.join(format!("{stem}.pb.cc"));
        let pb_h  = out_dir.join(format!("{stem}.pb.h"));

        if is_up_to_date(&abs, &pb_cc, &pb_h) {
            progress(BuildEvent::Fresh { path: proto_rel.clone() });
            continue;
        }

        progress(BuildEvent::Compiling { path: proto_rel.clone() });

        let mut cmd = Command::new(&protoc_bin);
        cmd.arg(&cpp_out_arg);
        for flag in &proto_path_flags {
            cmd.arg(flag);
        }
        if let Some(extra) = settings.extra.get("extra_flags") {
            for f in extra.split_whitespace() {
                cmd.arg(f);
            }
        }
        cmd.arg(&abs);

        prepend_tool_paths_to_env(&mut cmd, tool_paths);

        let status = cmd
            .status()
            .map_err(|e| FreightError::CompilerNotFound(format!("protoc not found: {e}")))?;

        if !status.success() {
            return Err(FreightError::CompileFailed(
                proto_rel.display().to_string(),
                format!("protoc exited with status {}", status.code().unwrap_or(-1)),
            ));
        }
    }

    // ── Collect generated sources ─────────────────────────────────────────────

    let generated_sources = collect_generated_cc_files(&out_dir, project_dir);

    Ok(ProtoGenResult {
        generated_sources,
        generated_include_dir: out_dir,
    })
}

/// Returns `true` if the project's `src/` directory contains at least one `.proto` file.
///
/// Used to skip the "no source files" guard in `load_project_at` for proto-only projects.
pub fn has_proto_files(project_dir: &Path) -> bool {
    let src_dir = project_dir.join("src");
    if !src_dir.is_dir() { return false; }
    walk_has_proto(&src_dir)
}

// ── Internal helpers ──────────────────────────────────────────────────────────

fn discover_proto_files(project_dir: &Path) -> Vec<PathBuf> {
    let src_dir = project_dir.join("src");
    if !src_dir.is_dir() { return vec![]; }
    let mut files = Vec::new();
    walk_proto(&src_dir, project_dir, &mut files);
    files.sort();
    files
}

fn walk_proto(dir: &Path, project_dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() {
            walk_proto(&p, project_dir, out);
        } else if p.extension().and_then(|e| e.to_str()) == Some("proto") {
            if let Ok(rel) = p.strip_prefix(project_dir) {
                out.push(rel.to_path_buf());
            }
        }
    }
}

fn walk_has_proto(dir: &Path) -> bool {
    let Ok(rd) = std::fs::read_dir(dir) else { return false };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() && walk_has_proto(&p) { return true; }
        if p.extension().and_then(|e| e.to_str()) == Some("proto") { return true; }
    }
    false
}

/// Returns `true` when both `pb_cc` and `pb_h` exist and are newer than `proto_src`.
fn is_up_to_date(proto_src: &Path, pb_cc: &Path, pb_h: &Path) -> bool {
    let Some(pm) = mtime(proto_src) else { return false };
    match (mtime(pb_cc), mtime(pb_h)) {
        (Some(cc), Some(h)) => pm < cc && pm < h,
        _ => false,
    }
}

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).ok()?.modified().ok()
}

/// Collect all `.pb.cc` files in `out_dir` as `SourceFile { lang_key: "cpp" }`.
fn collect_generated_cc_files(out_dir: &Path, project_dir: &Path) -> Vec<SourceFile> {
    let Ok(rd) = std::fs::read_dir(out_dir) else { return vec![] };
    let mut sources: Vec<SourceFile> = rd
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            let fname = p.file_name()?.to_str()?;
            if fname.ends_with(".pb.cc") {
                let rel = p.strip_prefix(project_dir).unwrap_or(&p).to_path_buf();
                Some(SourceFile { path: rel, lang_key: "cpp".to_string() })
            } else {
                None
            }
        })
        .collect();
    sources.sort_by(|a, b| a.path.cmp(&b.path));
    sources
}

fn prepend_tool_paths_to_env(cmd: &mut Command, tool_paths: &[PathBuf]) {
    if tool_paths.is_empty() { return; }
    let current = std::env::var_os("PATH").unwrap_or_default();
    let mut parts: Vec<PathBuf> = tool_paths.to_vec();
    parts.extend(std::env::split_paths(&current));
    if let Ok(new_path) = std::env::join_paths(parts) {
        cmd.env("PATH", new_path);
    }
}

/// Resolve a binary name: check `tool_paths` first, then fall back to bare name (PATH).
fn resolve_bin(name: &str, tool_paths: &[PathBuf]) -> String {
    for dir in tool_paths {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return candidate.to_string_lossy().into_owned();
        }
        #[cfg(windows)]
        {
            let exe = dir.join(format!("{name}.exe"));
            if exe.is_file() {
                return exe.to_string_lossy().into_owned();
            }
        }
    }
    name.to_string()
}
