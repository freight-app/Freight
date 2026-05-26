use freight_core::build::{bench_project_with, bench_workspace_with, BenchResult};

use crate::output::print_error;

#[derive(clap::Args)]
pub struct Args {
    /// Run only the bench with this name (file stem)
    pub name: Option<String>,
    /// Activate specific features (comma-separated or repeated)
    #[arg(long, value_name = "FEATURES", value_delimiter = ',')]
    pub features: Vec<String>,
    /// Do not activate default features
    #[arg(long)]
    pub no_default_features: bool,
    /// Select a specific workspace member to benchmark
    #[arg(long, short = 'p', value_name = "PACKAGE")]
    pub package: Option<String>,
    #[command(flatten)]
    pub build: super::common::BuildFlags,
}

impl Args {
    pub fn run(self) {
        self.build.apply();
        cmd_bench(
            self.name.as_deref(),
            self.package.as_deref(),
            &self.features,
            !self.no_default_features,
        );
    }
}

pub fn cmd_bench(filter: Option<&str>, package: Option<&str>, features: &[String], use_defaults: bool) {
    let progress = super::build::make_progress();
    if super::build::at_workspace_root() {
        match bench_workspace_with(filter, package, features, use_defaults, &progress) {
            Ok(summary) => {
                println!();
                if summary.results.is_empty() {
                    println!("no bench files found in any workspace member");
                    return;
                }
                print_bench_table(&summary.results);
            }
            Err(e) => { println!(); print_error(&e.to_string()); }
        }
        return;
    }

    if package.is_some() {
        print_error("`-p` can only be used at a workspace root");
        return;
    }

    match bench_project_with(filter, features, use_defaults, &progress) {
        Ok(summary) => {
            println!();
            if summary.results.is_empty() {
                println!("no bench files found under benches/");
                return;
            }
            print_bench_table(&summary.results);
        }
        Err(e) => { println!(); print_error(&e.to_string()); }
    }
}

fn print_bench_table(results: &[BenchResult]) {
    use owo_colors::OwoColorize;
    let name_width = results.iter().map(|r| r.name.len()).max().unwrap_or(10).max(10);
    println!("{:>12}  {:<width$}  {:>12}  {:>12}  {:>12}  {}",
        "bench".bold().cyan(),
        "name", "mean", "min", "max", "runs",
        width = name_width,
    );
    println!("{}", "─".repeat(name_width + 52));
    for r in results {
        println!("{:>12}  {:<width$}  {:>12}  {:>12}  {:>12}  {}",
            "",
            r.name,
            fmt_duration(r.mean_ns),
            fmt_duration(r.min_ns),
            fmt_duration(r.max_ns),
            r.runs,
            width = name_width,
        );
    }
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
