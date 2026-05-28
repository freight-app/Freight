use std::path::PathBuf;
use std::collections::{BTreeSet, HashMap};

#[derive(clap::Args)]
pub struct Args {
    #[arg(long)]
    pub release: bool,
    /// Activate specific features (comma-separated or repeated)
    #[arg(long, value_name = "FEATURES", value_delimiter = ',')]
    pub features: Vec<String>,
    /// Do not activate default features
    #[arg(long)]
    pub no_default_features: bool,
    /// Enable sanitizers for this run (e.g. address,undefined). Overrides the profile setting.
    #[arg(long, value_name = "LIST", value_delimiter = ',')]
    pub sanitize: Vec<String>,
    /// Select a specific workspace member to build
    #[arg(long, short = 'p', value_name = "PACKAGE")]
    pub package: Option<String>,
    /// Extra outputs to emit alongside object files. Accepted value: `asm`
    /// (writes `.s` files to `target/{profile}/asm/`).
    #[arg(long, value_name = "FORMAT", value_delimiter = ',')]
    pub emit: Vec<String>,
    /// Print a per-file compilation time table sorted by slowest first.
    #[arg(long)]
    pub time_passes: bool,
    /// Print the build graph (compilation stages and link step) instead of building.
    #[arg(long)]
    pub graph: bool,
    /// Output format for --graph: text (default), mermaid, dot
    #[arg(long, default_value = "text", value_name = "FORMAT", requires = "graph")]
    pub graph_format: String,
    /// Show a live TUI build-progress panel instead of plain output.
    #[arg(long)]
    pub panel: bool,
    #[command(flatten)]
    pub build: super::common::BuildFlags,
}

impl Args {
    pub fn run(self) {
        self.build.apply();
        if self.graph {
            cmd_build_graph(self.release, self.package.as_deref(), &self.features, !self.no_default_features, &self.graph_format);
        } else if self.panel {
            let profile = if self.release { "release" } else { "dev" };
            let ws = super::build::at_workspace_root();
            let code = crate::tui::build_panel::run(
                profile,
                self.features,
                !self.no_default_features,
                self.sanitize,
                ws,
                self.package,
            );
            std::process::exit(code);
        } else {
            cmd_build(self.release, self.package.as_deref(), &self.features, !self.no_default_features, &self.sanitize, &self.emit, self.time_passes);
        }
    }
}

use freight_core::build::{
    build_project_with, build_workspace_with,
    emit_asm_project_with,
    resolve_dep_graph, ResolvedDep,
};
use freight_core::event::{BuildEvent, Progress};
use freight_core::manifest::{find_manifest_dir, load_manifest, load_workspace_manifest};

use crate::output::{
    print_error, print_status, print_success, print_warning,
    GraphEdge, GraphCluster, GraphFormat, render_mermaid_graph, render_dot_graph,
};

// ── Progress ──────────────────────────────────────────────────────────────────

pub fn make_progress() -> Progress {
    use std::sync::Arc;
    use owo_colors::OwoColorize;
    Arc::new(|event| match event {
        BuildEvent::BuildStarted { name, profile } => {
            print_status("Building", &format!("{name} [{profile}]"));
        }
        BuildEvent::Compiling { path } => {
            print_status("Compiling", &path.display().to_string());
        }
        BuildEvent::Fresh { path } => {
            println!("{:>12} {}", "Fresh".dimmed(), path.display());
        }
        BuildEvent::Linking { name } => {
            print_status("Linking", &name);
        }
        BuildEvent::Archiving { name } => {
            print_status("Archiving", &name);
        }
        BuildEvent::RunningScript { cached } => {
            if cached {
                println!("{:>12} build script (cached)", "Running".dimmed());
            } else {
                print_status("Running", "build script");
            }
        }
        BuildEvent::FetchingDep { name, source } => {
            print_status("Fetching", &format!("{name} ({source})"));
        }
        BuildEvent::ResolvingDep { name, via } => {
            println!("{:>12} {} ({})", "Resolving".dimmed(), name, via);
        }
        BuildEvent::BuildingForeignDep { name, backend } => {
            print_status("Building", &format!("{name} ({backend})"));
        }
        BuildEvent::Warning(msg) => {
            print_warning(&msg);
        }
        BuildEvent::TestLinking { name } => {
            print_status("Linking", &name);
        }
        BuildEvent::TestRunning { name } => {
            print_status("Running", &name);
        }
        BuildEvent::TestResult { name, passed } => {
            if passed {
                println!("{:>12} {} ... ok", "test".bold(), name);
            } else {
                println!("{:>12} {} ... FAILED", "test".bold().red(), name);
            }
        }
        BuildEvent::BenchLinking { name } => {
            print_status("Linking", &name);
        }
        BuildEvent::BenchRunning { name } => {
            print_status("Benchmarking", &name);
        }
        BuildEvent::BenchResult { name, mean_ns } => {
            println!("{:>12} {} … {}", "bench".bold().cyan(), name, fmt_duration(mean_ns));
        }
        BuildEvent::Timing { .. } => {}   // collected separately; not printed inline
        BuildEvent::EmittedAsm { path } => {
            println!("{:>12} {}", "Emitted".dimmed(), path.display());
        }
    })
}

/// Like [`make_progress`] but also collects [`BuildEvent::Timing`] events into
/// the returned `Arc<Mutex<Vec<(PathBuf, u64)>>>` for post-build reporting.
fn make_timed_progress() -> (Progress, std::sync::Arc<std::sync::Mutex<Vec<(PathBuf, u64)>>>) {
    use std::sync::{Arc, Mutex};
    use owo_colors::OwoColorize;

    let timings: Arc<Mutex<Vec<(PathBuf, u64)>>> = Arc::new(Mutex::new(Vec::new()));
    let timings_sink = Arc::clone(&timings);

    let progress: Progress = Arc::new(move |event| match event {
        BuildEvent::Timing { ref path, ns } => {
            timings_sink.lock().unwrap().push((path.clone(), ns));
        }
        BuildEvent::BuildStarted { name, profile } => print_status("Building", &format!("{name} [{profile}]")),
        BuildEvent::Compiling { path } => print_status("Compiling", &path.display().to_string()),
        BuildEvent::Fresh { path } => println!("{:>12} {}", "Fresh".dimmed(), path.display()),
        BuildEvent::Linking { name } => print_status("Linking", &name),
        BuildEvent::Archiving { name } => print_status("Archiving", &name),
        BuildEvent::RunningScript { cached } => {
            if cached { println!("{:>12} build script (cached)", "Running".dimmed()); }
            else { print_status("Running", "build script"); }
        }
        BuildEvent::FetchingDep { name, source } => print_status("Fetching", &format!("{name} ({source})")),
        BuildEvent::ResolvingDep { name, via } => println!("{:>12} {} ({})", "Resolving".dimmed(), name, via),
        BuildEvent::BuildingForeignDep { name, backend } => print_status("Building", &format!("{name} ({backend})")),
        BuildEvent::Warning(msg) => print_warning(&msg),
        BuildEvent::EmittedAsm { path } => println!("{:>12} {}", "Emitted".dimmed(), path.display()),
        _ => {}
    });

    (progress, timings)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Return true when the nearest `freight.toml` (found by walking up from cwd)
/// has a `[workspace]` section. Falls through to the regular project path on
/// any I/O or parse error.
pub fn at_workspace_root() -> bool {
    let Ok(cwd) = std::env::current_dir() else { return false };
    let Some(dir) = find_manifest_dir(&cwd) else { return false };
    load_workspace_manifest(&dir).is_some()
}

// ── Commands ──────────────────────────────────────────────────────────────────

pub fn cmd_build(release: bool, package: Option<&str>, features: &[String], use_defaults: bool, sanitize: &[String], emit: &[String], time_passes: bool) {
    let profile = if release { "release" } else { "dev" };

    if time_passes {
        // Safety: single-threaded here; rayon workers not yet started.
        unsafe { std::env::set_var("FREIGHT_TIME_PASSES", "1"); }
    }

    let (progress, timings) = if time_passes {
        make_timed_progress()
    } else {
        (make_progress(), std::sync::Arc::new(std::sync::Mutex::new(vec![])))
    };

    let build_ok;
    if at_workspace_root() {
        match build_workspace_with(profile, package, features, use_defaults, &progress) {
            Ok(outputs) => {
                println!();
                for o in &outputs {
                    print_success(&format!(
                        "{} ({} compiled, {} up to date)",
                        o.package_name, o.compiled, o.skipped,
                    ));
                    for bin in &o.binaries {
                        println!("    {}", bin.display());
                    }
                }
                build_ok = true;
            }
            Err(e) => { println!(); print_error(&e.to_string()); build_ok = false; }
        }
    } else {
        if package.is_some() {
            print_error("`-p` can only be used at a workspace root");
            return;
        }
        match build_project_with(profile, features, use_defaults, sanitize, &progress) {
            Ok(output) => {
                println!();
                print_success(&format!(
                    "{} ({} compiled, {} up to date)",
                    output.package_name, output.compiled, output.skipped,
                ));
                for bin in &output.binaries {
                    println!("    {}", bin.display());
                }
                build_ok = true;
            }
            Err(e) => { println!(); print_error(&e.to_string()); build_ok = false; }
        }
    }

    if build_ok {
        if emit.iter().any(|e| e.eq_ignore_ascii_case("asm")) {
            if let Err(e) = emit_asm_project_with(profile, &progress) {
                print_error(&format!("--emit asm failed: {e}"));
            }
        }
        if time_passes {
            let mut t = timings.lock().unwrap();
            t.sort_by(|a, b| b.1.cmp(&a.1));
            print_timing_table(&t);
        }
    }
}

// ── freight build --graph ─────────────────────────────────────────────────────

pub fn cmd_build_graph(release: bool, _package: Option<&str>, features: &[String], _use_defaults: bool, format: &str) {
    use owo_colors::OwoColorize;

    let profile = if release { "release" } else { "dev" };

    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => { print_error(&format!("cannot read cwd: {e}")); return; }
    };
    let project_dir = match find_manifest_dir(&cwd) {
        Some(d) => d,
        None => { print_error("no freight.toml found"); return; }
    };
    let manifest = match load_manifest(&project_dir) {
        Ok(m) => m,
        Err(e) => { print_error(&format!("failed to load manifest: {e}")); return; }
    };

    let activated: BTreeSet<String> = features.iter().cloned().collect();

    let resolved = match resolve_dep_graph(&project_dir, &manifest, false, &activated) {
        Ok(r) => r,
        Err(e) => { print_error(&format!("dependency resolution failed: {e}")); return; }
    };

    // Assign a build stage to every resolved dep.
    // resolved is already in topological order (leaves first), so we can
    // compute stage[dep] = max(stage[freight_dep]) + 1 in a single pass.
    let mut stage_of: HashMap<String, usize> = HashMap::new();
    for dep in &resolved {
        // Stage = one above the highest stage of any freight dep this dep needs.
        let max_dep_stage = dep.manifest.dependencies.keys()
            .filter_map(|n| stage_of.get(n).copied())
            .max();
        stage_of.insert(dep.name.clone(), max_dep_stage.map_or(0, |s| s + 1));
    }

    // Group by stage.
    let max_stage = stage_of.values().copied().max().unwrap_or(0);
    let mut stages: Vec<Vec<&ResolvedDep>> = vec![vec![]; max_stage + 1];
    for dep in &resolved {
        let s = stage_of[&dep.name];
        stages[s].push(dep);
    }

    let fmt = GraphFormat::parse(format);

    if fmt != GraphFormat::Text {
        // Build clusters (one per stage) and edges.
        let mut clusters: Vec<GraphCluster> = Vec::new();
        let mut edges: Vec<GraphEdge> = Vec::new();

        for (stage_idx, stage_deps) in stages.iter().enumerate() {
            let label = format!("Stage {stage_idx}");
            let nodes = stage_deps.iter()
                .map(|d| format!("{}\n{}", d.name, d.manifest.package.version))
                .collect();
            clusters.push(GraphCluster { id: format!("stage{stage_idx}"), label, nodes });

            for dep in stage_deps.iter() {
                for needed in dep.manifest.dependencies.keys() {
                    if stage_of.contains_key(needed) {
                        edges.push(GraphEdge {
                            from: format!("{}\n{}", needed, resolved.iter().find(|r| &r.name == needed).map_or("", |r| &r.manifest.package.version)),
                            to:   format!("{}\n{}", dep.name, dep.manifest.package.version),
                        });
                    }
                }
            }
        }

        // Root project node and its edges.
        let root_node = format!("{}\n{}", manifest.package.name, manifest.package.version);
        let mut root_needs: Vec<String> = Vec::new();
        for dep in &resolved {
            if manifest.dependencies.contains_key(&dep.name) {
                edges.push(GraphEdge {
                    from: format!("{}\n{}", dep.name, dep.manifest.package.version),
                    to:   root_node.clone(),
                });
                root_needs.push(dep.name.clone());
            }
        }

        // Link node.
        let bin_names: Vec<String> = if manifest.bins.is_empty() {
            vec![manifest.package.name.clone()]
        } else {
            manifest.bins.iter().map(|b| b.name.clone()).collect()
        };
        let link_node = format!("link: {}", bin_names.join(", "));
        edges.push(GraphEdge { from: root_node.clone(), to: link_node.clone() });

        let ungrouped = vec![root_node, link_node];
        let title = format!("{} build graph [{}]", manifest.package.name, profile);
        match fmt {
            GraphFormat::Mermaid => render_mermaid_graph(&title, &clusters, &edges, &ungrouped),
            GraphFormat::Dot     => render_dot_graph(&title, &clusters, &edges, &ungrouped),
            GraphFormat::Text    => unreachable!(),
        }
        return;
    }

    // Print header.
    println!(
        "{} {}  {}",
        manifest.package.name.bold().bright_blue(),
        manifest.package.version.bright_black(),
        format!("[{profile}]").bright_black()
    );

    let rule = "─".repeat(48).bright_black().to_string();

    // Print each dep stage.
    for (stage_idx, stage_deps) in stages.iter().enumerate() {
        if stage_deps.is_empty() { continue; }

        println!();
        let needs: Vec<String> = stage_deps.iter()
            .flat_map(|d| d.manifest.dependencies.keys()
                .filter(|n| stage_of.get(*n).copied().unwrap_or(usize::MAX) < stage_idx)
                .cloned())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter().collect();

        let label = if needs.is_empty() {
            format!("Stage {stage_idx}  (parallel)")
        } else {
            format!("Stage {stage_idx}  (parallel · needs: {})", needs.join(", "))
        };
        println!("{rule}");
        println!("{}", label.bold());

        for (di, dep) in stage_deps.iter().enumerate() {
            let is_last_dep = di == stage_deps.len() - 1;
            let dep_conn  = if is_last_dep { "└── " } else { "├── " };
            let src_prefix = if is_last_dep { "    " } else { "│   " };

            let origin = dep.dir.strip_prefix(&project_dir)
                .map(|p| format!("({})", p.display()))
                .unwrap_or_else(|_| format!("({})", dep.dir.display()));

            println!(
                "{}{}  {}  {}",
                dep_conn.bright_black(),
                dep.name.bold().bright_blue(),
                dep.manifest.package.version.bright_black(),
                origin.yellow()
            );

            // Collect and print source files for this dep.
            let src_dir = dep.dir.join("src");
            let srcs = collect_graph_sources(&src_dir);
            for (si, src) in srcs.iter().enumerate() {
                let is_last_src = si == srcs.len() - 1;
                let src_conn = if is_last_src { "└── " } else { "├── " };
                let rel = src.strip_prefix(&dep.dir).unwrap_or(src);
                println!("{}{}{}", src_prefix.bright_black(), src_conn.bright_black(), rel.display().to_string().bright_black());
            }
        }
    }

    // Root project sources (final compile stage).
    let root_src_dir = project_dir.join("src");
    let root_srcs = collect_graph_sources(&root_src_dir);
    if !root_srcs.is_empty() {
        println!();
        println!("{rule}");
        println!("{}", format!("Stage {}  (root)", max_stage + 1).bold());
        for (i, src) in root_srcs.iter().enumerate() {
            let is_last = i == root_srcs.len() - 1;
            let conn = if is_last { "└── " } else { "├── " };
            let rel = src.strip_prefix(&project_dir).unwrap_or(src);
            println!("{}{}", conn.bright_black(), rel.display());
        }
    }

    // Link step.
    println!();
    println!("{rule}");
    println!("{}", "Link".bold());
    let target_dir = project_dir.join("target").join(profile);

    // Binaries from [[bin]] targets.
    let bins: Vec<String> = manifest.bins.iter()
        .map(|b| b.name.clone())
        .collect();
    let bin_names = if bins.is_empty() {
        vec![manifest.package.name.clone()]
    } else {
        bins
    };

    for bin in &bin_names {
        let exe = target_dir.join(bin);
        println!("└── {}", exe.display().to_string().bright_blue().bold());
    }

    // List dep libs.
    if !resolved.is_empty() {
        let libs: Vec<String> = resolved.iter()
            .map(|d| format!("lib{}.a", d.name))
            .collect();
        println!("    {}", libs.join("  ").bright_black());
    }

    println!("{rule}");
}

fn collect_graph_sources(src_dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    const SOURCE_EXTS: &[&str] = &["c", "cc", "cpp", "cxx", "c++", "cu", "hip", "m", "mm"];
    let mut files = Vec::new();
    collect_graph_sources_rec(src_dir, SOURCE_EXTS, &mut files);
    files.sort();
    files
}

fn collect_graph_sources_rec(dir: &std::path::Path, exts: &[&str], out: &mut Vec<std::path::PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect_graph_sources_rec(&path, exts, out);
        } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if exts.contains(&ext) {
                out.push(path);
            }
        }
    }
}

fn print_timing_table(timings: &[(PathBuf, u64)]) {
    use owo_colors::OwoColorize;
    if timings.is_empty() { return; }
    println!();
    let name_width = timings.iter()
        .map(|(p, _)| p.display().to_string().len())
        .max().unwrap_or(20).max(20).min(60);
    println!("{:>12}  {:<width$}  {:>10}", "time-passes".bold().yellow(), "file", "time", width = name_width);
    println!("{}", "─".repeat(name_width + 26));
    for (path, ns) in timings {
        println!("{:>12}  {:<width$}  {:>10}",
            "",
            truncate_left(&path.display().to_string(), name_width),
            fmt_duration(*ns),
            width = name_width,
        );
    }
}

fn truncate_left(s: &str, max: usize) -> String {
    if s.len() <= max { s.to_string() }
    else { format!("…{}", &s[s.len() - max + 1..]) }
}

fn fmt_duration(ns: u64) -> String {
    if ns >= 1_000_000_000 {
        format!("{:.3} s ", ns as f64 / 1_000_000_000.0)
    } else if ns >= 1_000_000 {
        format!("{:.3} ms", ns as f64 / 1_000_000.0)
    } else if ns >= 1_000 {
        format!("{:.3} µs", ns as f64 / 1_000.0)
    } else {
        format!("{ns} ns")
    }
}
