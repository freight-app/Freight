use std::io::Write as _;
use std::path::Path;

use freight::doc::extract_dir;
use sha2::{Digest, Sha256};

use freight::build::{build_project_with, test_project_with};
use freight::event::silent;
use freight::manifest::types::Manifest;
use freight::manifest::{find_manifest_dir, load_manifest};
use freight::registry::freight_registry::FreightRegistry;
use freight::registry::host_triple;
use freight::toolchain::cache::GlobalConfig;

use crate::output::{print_error, print_status, print_success, print_warning};

// ── CLI args ──────────────────────────────────────────────────────────────────

#[derive(clap::Args)]
pub struct Args {
    /// Dry-run: show what would be published without sending anything.
    #[arg(long)]
    pub dry_run: bool,

    /// Skip the confirmation prompt.
    #[arg(long, short = 'y')]
    pub yes: bool,

    /// Skip the pre-publish pipeline (build + test + scan).  Not recommended.
    #[arg(long)]
    pub no_verify: bool,

    /// Registry to publish to (default: first configured registry).
    #[arg(long, short = 'r', value_name = "REGISTRY")]
    pub registry: Option<String>,

    /// Upload a prebuilt binary tarball for the given triple instead of source.
    /// Omit the triple to use the detected host triple (e.g. x86_64-linux-gnu).
    #[arg(long, value_name = "TRIPLE")]
    pub prebuilt: Option<Option<String>>,
}

impl Args {
    pub fn run(self) {
        if let Some(triple_opt) = self.prebuilt {
            cmd_publish_prebuilt(
                triple_opt.as_deref(),
                self.registry.as_deref(),
                self.dry_run,
                self.yes,
                self.no_verify,
            );
        } else {
            cmd_publish(
                self.dry_run,
                self.yes,
                self.no_verify,
                self.registry.as_deref(),
            );
        }
    }
}

// ── Source publish ────────────────────────────────────────────────────────────

fn cmd_publish(dry_run: bool, yes: bool, no_verify: bool, repo: Option<&str>) {
    let project_dir = match super::common::locate_project_dir() {
        Some(d) => d,
        None => return,
    };
    let manifest = match load_manifest(&project_dir) {
        Ok(m) => m,
        Err(e) => {
            print_error(&e.to_string());
            return;
        }
    };

    let name = manifest.package.name.clone();
    let version = manifest.package.version.clone();

    // ── dirty-tree warning ───────────────────────────────────────────────────

    if let Some(msg) = dirty_git_message(&project_dir) {
        print_warning(&msg);
    }

    let (registry, registry_name) = match resolve_registry(repo, &project_dir) {
        Some(r) => r,
        None => return,
    };

    // ── version already published? ───────────────────────────────────────────

    match registry.package_exists(&name, &version) {
        Ok(true) => {
            print_error(&format!(
                "`{name}@{version}` is already published on `{registry_name}` — \
                 bump the version in freight.toml first"
            ));
            return;
        }
        Ok(false) => {}
        Err(e) => print_warning(&format!(
            "could not check registry for existing version: {e}"
        )),
    }

    // ── pre-publish pipeline: build → test → scan ────────────────────────────

    if !no_verify {
        if !run_pre_publish_pipeline(&name) {
            return;
        }
    } else {
        print_warning("--no-verify: skipping build, test, and security scan");
    }

    // ── show plan, ask for confirmation ──────────────────────────────────────

    let description = non_empty(manifest.package.description.as_str());
    let license = non_empty(manifest.package.license.as_str());

    print_status(
        "publishing",
        &format!("`{name}@{version}` to `{registry_name}`"),
    );
    if let Some(d) = description {
        print_status("description", d);
    }
    if let Some(l) = license {
        print_status("license", l);
    }

    if dry_run {
        print_status("dry-run", "no files were uploaded");
        return;
    }

    if !yes && !confirm(&format!("Publish `{name}@{version}` to `{registry_name}`?")) {
        print_status("cancelled", "nothing was published");
        return;
    }

    // ── package ───────────────────────────────────────────────────────────────

    print_status("packaging", &format!("{name}@{version}"));

    let tarball = match build_source_tarball(&project_dir, &manifest) {
        Ok(b) => b,
        Err(e) => {
            print_error(&format!("packaging failed: {e}"));
            return;
        }
    };

    let checksum = hex_sha256(&tarball);
    print_status(
        "packaged",
        &format!("{} bytes  sha256:{}", tarball.len(), &checksum[..16]),
    );

    // ── upload ────────────────────────────────────────────────────────────────

    match registry.publish_package(
        &name,
        &version,
        None,
        description,
        license,
        &tarball,
        None,
        None,
    ) {
        Ok(()) => print_success(&format!("published `{name}@{version}`")),
        Err(e) => {
            print_error(&e.to_string());
            return;
        }
    }

    // ── docs (non-fatal) ─────────────────────────────────────────────────────

    upload_docs(&registry, &name, &version, &project_dir);
}

// ── Pre-publish pipeline ──────────────────────────────────────────────────────

/// Runs the full pre-publish pipeline: build → test → security scan.
/// Returns `true` if all steps pass, `false` if any step failed.
fn run_pre_publish_pipeline(name: &str) -> bool {
    // Step 1: build (dev profile — fast, includes debug info for scanning)
    print_status("verifying", &format!("[1/3] building `{name}`…"));
    match build_project_with("dev", &[], true, &[], &silent()) {
        Ok(output) => {
            print_status(
                "ok",
                &format!("[1/3] build passed ({} compiled)", output.compiled),
            );

            // Step 2: run tests
            print_status("verifying", &format!("[2/3] running tests for `{name}`…"));
            match test_project_with("dev", None, &[], true, &[], &silent()) {
                Ok(summary) if summary.failed == 0 => {
                    print_status(
                        "ok",
                        &format!("[2/3] tests passed ({} passed)", summary.passed),
                    );
                }
                Ok(summary) => {
                    print_error(&format!(
                        "[2/3] {} test{} failed — fix failing tests before publishing \
                         (or pass --no-verify)",
                        summary.failed,
                        if summary.failed == 1 { "" } else { "s" }
                    ));
                    return false;
                }
                Err(e) => {
                    print_warning(&format!("[2/3] could not run tests: {e} — continuing"));
                }
            }

            // Step 3: security scan built binaries
            print_status(
                "verifying",
                &format!("[3/3] scanning binaries for `{name}`…"),
            );
            let scan_result = scan_binaries(&output.binaries);
            match scan_result {
                ScanResult::Clean(n) => {
                    print_status("ok", &format!("[3/3] {n} binary scanned, no threats found"));
                }
                ScanResult::Threats(findings) => {
                    print_error(&format!(
                        "[3/3] security scan found {} potential threat{}:",
                        findings.len(),
                        if findings.len() == 1 { "" } else { "s" }
                    ));
                    for f in &findings {
                        eprintln!("       {f}");
                    }
                    print_error(
                        "publishing aborted — investigate the findings or pass --no-verify \
                         to override",
                    );
                    return false;
                }
                ScanResult::Unavailable(reason) => {
                    print_warning(&format!("[3/3] security scan skipped: {reason}"));
                    print_warning(
                        "install ClamAV (`clamd`/`clamscan`) for binary malware detection",
                    );
                }
            }
        }
        Err(e) => {
            print_error(&format!(
                "[1/3] build failed — fix errors before publishing (or pass --no-verify):\n  {e}"
            ));
            return false;
        }
    }

    true
}

// ── Security scan ─────────────────────────────────────────────────────────────

enum ScanResult {
    /// All binaries scanned cleanly.  Carries the number of binaries checked.
    Clean(usize),
    /// One or more threats found.  Carries human-readable finding strings.
    Threats(Vec<String>),
    /// Scanner not available on this system.
    Unavailable(String),
}

/// Scan built binaries for malware using ClamAV (`clamscan`) if available.
fn scan_binaries(binaries: &[std::path::PathBuf]) -> ScanResult {
    let bins: Vec<_> = binaries.iter().filter(|p| p.is_file()).collect();
    if bins.is_empty() {
        return ScanResult::Clean(0);
    }

    // Try clamscan first (most widely available AV on Linux).
    if let Some(scanner) = find_executable("clamscan") {
        return run_clamscan(&scanner, &bins);
    }

    // Fallback: basic static heuristics (no external tool required).
    run_heuristic_scan(&bins)
}

fn run_clamscan(clamscan: &str, bins: &[&std::path::PathBuf]) -> ScanResult {
    let mut cmd = std::process::Command::new(clamscan);
    cmd.args(["--no-summary", "--infected"]);
    for b in bins {
        cmd.arg(b);
    }

    match cmd.output() {
        Ok(out) => {
            if out.status.success() {
                ScanResult::Clean(bins.len())
            } else {
                let findings: Vec<String> = String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .filter(|l| l.contains("FOUND"))
                    .map(str::to_string)
                    .collect();
                if findings.is_empty() {
                    // Non-zero exit with no FOUND lines = scan error (DB stale, etc.)
                    ScanResult::Unavailable(String::from_utf8_lossy(&out.stderr).trim().to_string())
                } else {
                    ScanResult::Threats(findings)
                }
            }
        }
        Err(e) => ScanResult::Unavailable(e.to_string()),
    }
}

/// Lightweight heuristic scan: flag binaries that look structurally suspicious
/// (e.g. contain known bad string patterns or import sets associated with
/// trojans/backdoors in native binaries on this platform).
///
/// This is a best-effort check, not a replacement for a proper AV scanner.
fn run_heuristic_scan(bins: &[&std::path::PathBuf]) -> ScanResult {
    // Known malicious / highly suspicious string patterns in binary payloads.
    // These are deliberately conservative — only flag things that are extremely
    // unlikely to appear in legitimate compiled C/C++/Fortran/etc. programs.
    const SUSPICIOUS_STRINGS: &[&[u8]] = &[
        b"/dev/tcp/",             // bash reverse shell fragment
        b"$IFS&&",                // shell injection fragment
        b"cmd.exe /c powershell", // PowerShell dropper fragment
        b"TVqQAAMAAAAEAAA",       // base64-encoded MZ header (EXE embedded in binary)
        b"eval(base64_decode",    // PHP webshell — unlikely in a compiled binary but red flag
    ];

    let mut findings = Vec::new();
    for bin in bins {
        let Ok(data) = std::fs::read(bin) else {
            continue;
        };
        for pattern in SUSPICIOUS_STRINGS {
            if data.windows(pattern.len()).any(|w| w == *pattern) {
                findings.push(format!(
                    "{}: contains suspicious pattern `{}`",
                    bin.display(),
                    String::from_utf8_lossy(pattern).trim()
                ));
            }
        }
    }

    if findings.is_empty() {
        // We did a heuristic pass but not a full AV scan.
        ScanResult::Unavailable("ClamAV not found — only heuristic checks were run".to_string())
    } else {
        ScanResult::Threats(findings)
    }
}

// ── Prebuilt publish ──────────────────────────────────────────────────────────

fn cmd_publish_prebuilt(
    triple: Option<&str>,
    repo: Option<&str>,
    dry_run: bool,
    yes: bool,
    no_verify: bool,
) {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            print_error(&format!("cannot read cwd: {e}"));
            return;
        }
    };
    let project_dir = match find_manifest_dir(&cwd) {
        Some(d) => d,
        None => {
            print_error("no freight.toml found");
            return;
        }
    };
    let manifest = match load_manifest(&project_dir) {
        Ok(m) => m,
        Err(e) => {
            print_error(&e.to_string());
            return;
        }
    };

    let name = manifest.package.name.clone();
    let version = manifest.package.version.clone();
    let triple = triple.map(str::to_string).unwrap_or_else(host_triple);

    let (registry, registry_name) = match resolve_registry(repo, &project_dir) {
        Some(r) => r,
        None => return,
    };

    // Pre-publish pipeline for prebuilts: build release + test + scan.
    if !no_verify {
        // Use release for prebuilt so the packaged binary IS what was tested.
        print_status("verifying", &format!("[1/3] building `{name}` (release)…"));
        let output = match build_project_with("release", &[], true, &[], &silent()) {
            Ok(o) => {
                print_status("ok", "[1/3] release build passed");
                o
            }
            Err(e) => {
                print_error(&format!("[1/3] release build failed: {e}"));
                return;
            }
        };

        print_status("verifying", &format!("[2/3] running tests for `{name}`…"));
        match test_project_with("dev", None, &[], true, &[], &silent()) {
            Ok(s) if s.failed == 0 => {
                print_status("ok", &format!("[2/3] {} tests passed", s.passed))
            }
            Ok(s) => {
                print_error(&format!("[2/3] {} tests failed", s.failed));
                return;
            }
            Err(e) => print_warning(&format!("[2/3] could not run tests: {e}")),
        }

        print_status("verifying", "[3/3] scanning release binaries…");
        match scan_binaries(&output.binaries) {
            ScanResult::Clean(n) => print_status("ok", &format!("[3/3] {n} binary scanned")),
            ScanResult::Threats(t) => {
                for f in &t {
                    eprintln!("       {f}");
                }
                print_error("[3/3] threats found — aborting");
                return;
            }
            ScanResult::Unavailable(r) => print_warning(&format!("[3/3] scan skipped: {r}")),
        }
    }

    print_status(
        "prebuilt",
        &format!("packaging `{name}@{version}` for {triple}"),
    );

    if dry_run {
        print_status("dry-run", "no files were uploaded");
        return;
    }

    if !yes
        && !confirm(&format!(
            "Publish prebuilt `{name}@{version}` ({triple}) to `{registry_name}`?"
        ))
    {
        print_status("cancelled", "nothing was published");
        return;
    }

    let tarball = match build_prebuilt_tarball(&project_dir, &manifest, &triple) {
        Ok(t) => t,
        Err(e) => {
            print_error(&format!("packaging failed: {e}"));
            return;
        }
    };

    let checksum = hex_sha256(&tarball);
    print_status(
        "packaged",
        &format!("{} bytes  sha256:{}", tarball.len(), &checksum[..16]),
    );

    let channel: Option<&str> = None;
    match registry.upload_prebuilt(&name, &version, channel, &triple, &tarball) {
        Ok(()) => print_success(&format!(
            "published prebuilt `{name}@{version}` for {triple}"
        )),
        Err(e) => print_error(&format!("upload failed: {e}")),
    }
}

// ── Tarball builders ──────────────────────────────────────────────────────────

/// Patterns always excluded from source tarballs regardless of `.freightignore`.
const DEFAULT_EXCLUDES: &[&str] = &[
    // build artifacts
    "target/",
    ".freight-build/",
    ".pkgs/",
    // version control
    ".git/",
    ".hg/",
    ".svn/",
    // IDE / editor
    ".vscode/",
    ".idea/",
    "*.swp",
    "*.swo",
    ".DS_Store",
    "Thumbs.db",
    // generated by freight / local config
    "compile_commands.json",
    "freight.lock",
    ".freight/",
    // credentials / secrets
    ".env",
    ".env.*",
    "*.pem",
    "*.key",
    "*.p12",
    "*.pfx",
    "*.crt",
    // databases
    "*.sqlite",
    "*.sqlite3",
    "*.db",
];

/// Read `.freightignore` from `project_dir` and return the patterns, or an
/// empty vec if the file doesn't exist.  The format is identical to
/// `.gitignore`: one glob pattern per line, `#` for comments.
/// Read `freight.toml` and return a cleaned version with `registry` stripped
/// from every dependency entry. Consumers resolve deps via their own config.
fn strip_registry_from_manifest(path: &Path) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let src = std::fs::read_to_string(path)?;
    let mut doc: toml_edit::DocumentMut = src.parse()?;
    for section in &["dependencies", "dev-dependencies", "build-dependencies"] {
        if let Some(item) = doc.get_mut(section) {
            if let Some(table) = item.as_table_mut() {
                for (_, dep) in table.iter_mut() {
                    if let Some(inline) = dep.as_value_mut().and_then(|v| v.as_inline_table_mut()) {
                        inline.remove("registry");
                    } else if let Some(t) = dep.as_table_mut() {
                        t.remove("registry");
                    }
                }
            }
        }
    }
    Ok(doc.to_string().into_bytes())
}

fn load_freightignore(project_dir: &Path) -> Vec<String> {
    let path = project_dir.join(".freightignore");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return vec![];
    };
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
        .collect()
}

fn build_source_tarball(
    project_dir: &Path,
    manifest: &Manifest,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let name = &manifest.package.name;
    let version = &manifest.package.version;
    let prefix = format!("{name}-{version}");
    let user_ignores = load_freightignore(project_dir);
    let enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut ar = tar::Builder::new(enc);

    for entry in walkdir::WalkDir::new(project_dir)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        let rel = match path.strip_prefix(project_dir) {
            Ok(r) if !r.as_os_str().is_empty() => r,
            _ => continue,
        };
        let rel_str = rel.to_string_lossy();

        let excluded = DEFAULT_EXCLUDES
            .iter()
            .copied()
            .chain(user_ignores.iter().map(String::as_str))
            .any(|pat| glob_matches(pat, &rel_str));
        if excluded {
            continue;
        }

        if path.is_file() {
            if rel_str == "freight.toml" {
                let cleaned = strip_registry_from_manifest(path)?;
                let mut header = tar::Header::new_gnu();
                header.set_size(cleaned.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                ar.append_data(
                    &mut header,
                    format!("{prefix}/{rel_str}"),
                    cleaned.as_slice(),
                )?;
            } else {
                ar.append_path_with_name(path, format!("{prefix}/{rel_str}"))?;
            }
        }
    }

    Ok(ar.into_inner()?.finish()?)
}

fn build_prebuilt_tarball(
    project_dir: &Path,
    manifest: &Manifest,
    _triple: &str,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let name = &manifest.package.name;
    let version = &manifest.package.version;
    let desc = &manifest.package.description;

    let enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut ar = tar::Builder::new(enc);

    let include_dir = project_dir.join("include");
    if include_dir.is_dir() {
        ar.append_dir_all("include", &include_dir)?;
    }

    let release_dir = project_dir.join("target").join("release");
    for ext in &["a", "so", "dll", "dylib", "lib"] {
        for stem in &[format!("lib{name}"), name.clone()] {
            let candidate = release_dir.join(format!("{stem}.{ext}"));
            if candidate.is_file() {
                ar.append_path_with_name(&candidate, &format!("lib/{stem}.{ext}"))?;
            }
        }
    }
    let bin_candidate = release_dir.join(name.as_str());
    if bin_candidate.is_file() {
        ar.append_path_with_name(&bin_candidate, &format!("bin/{name}"))?;
    }

    let pc = format!(
        "prefix=/usr/local\nlibdir=${{prefix}}/lib\nincludedir=${{prefix}}/include\n\n\
         Name: {name}\nDescription: {desc}\nVersion: {version}\n\
         Cflags: -I${{includedir}}\nLibs: -L${{libdir}} -l{name}\n"
    );
    let pc_bytes = pc.as_bytes();
    let mut header = tar::Header::new_gnu();
    header.set_size(pc_bytes.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    ar.append_data(&mut header, &format!("lib/pkgconfig/{name}.pc"), pc_bytes)?;

    Ok(ar.into_inner()?.finish()?)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn upload_docs(registry: &FreightRegistry, name: &str, version: &str, project_dir: &Path) {
    let src_dir = project_dir.join("src");
    let scan_dir = if src_dir.is_dir() {
        src_dir
    } else {
        project_dir.to_path_buf()
    };
    let items = extract_dir(&scan_dir).items;
    if items.is_empty() {
        print_warning("no doc comments found — skipping docs upload");
        return;
    }
    print_status("uploading", &format!("docs ({} symbols)", items.len()));
    match freight::doc::to_msgpack(&items) {
        Ok(blob) => {
            if let Err(e) = registry.upload_docs(name, version, &blob) {
                print_warning(&format!("docs upload failed (non-fatal): {e}"));
            }
        }
        Err(e) => print_warning(&format!("docs serialization failed: {e}")),
    }
}

fn resolve_registry(repo: Option<&str>, project_dir: &Path) -> Option<(FreightRegistry, String)> {
    let config = {
        let mut cfg = GlobalConfig::load();
        if let Some(local) = GlobalConfig::load_local(project_dir) {
            cfg.apply_local(local);
        }
        cfg
    };
    if let Some(rname) = repo {
        match config.registries.iter().find(|r| r.name == rname) {
            Some(c) => Some((FreightRegistry::from_config(c), rname.to_string())),
            None => {
                print_error(&format!("unknown registry `{rname}`"));
                None
            }
        }
    } else {
        match config.registries.first() {
            Some(c) => Some((FreightRegistry::from_config(c), c.name.clone())),
            None => Some((FreightRegistry::default_registry(), "default".to_string())),
        }
    }
}

fn dirty_git_message(project_dir: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(project_dir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.trim().is_empty() {
        return None;
    }
    let count = stdout.lines().count();
    Some(format!(
        "publishing with {count} uncommitted change{} — consider committing first",
        if count == 1 { "" } else { "s" }
    ))
}

fn confirm(prompt: &str) -> bool {
    print!("{prompt} [y/N] ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

fn glob_matches(pattern: &str, path: &str) -> bool {
    if let Some(dir) = pattern.strip_suffix('/') {
        return path.starts_with(dir) || path == dir;
    }
    glob::Pattern::new(pattern)
        .map(|p| p.matches(path))
        .unwrap_or(false)
}

fn hex_sha256(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    format!("{:x}", h.finalize())
}

fn non_empty(s: &str) -> Option<&str> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn find_executable(name: &str) -> Option<String> {
    std::env::var_os("PATH")?
        .to_string_lossy()
        .split(':')
        .map(|dir| format!("{dir}/{name}"))
        .find(|p| std::path::Path::new(p).is_file())
}
