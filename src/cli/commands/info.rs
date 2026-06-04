use std::path::Path;

use freight::dep_cmds::locate_project;
use freight::manifest::types::{Dependency, Manifest};
use freight::registry::repos::{registries_in_order, repo_by_name};
use freight::toolchain::cache::GlobalConfig;

use crate::output::{print_error, print_status, print_warning};
use owo_colors::OwoColorize;

#[derive(clap::Args)]
pub struct Args {
    pub package: Option<String>,
    /// Registry to query (default: all configured registries in order)
    #[arg(long, value_name = "NAME")]
    pub repo: Option<String>,
}

impl Args {
    pub fn run(self) {
        cmd_info(self.package.as_deref(), self.repo.as_deref());
    }
}

fn cmd_info(package: Option<&str>, repo: Option<&str>) {
    if let Some(package) = package {
        let config = {
            let mut cfg = GlobalConfig::load();
            let cwd = std::env::current_dir().unwrap_or_default();
            if let Some(proj) = freight::manifest::find_manifest_dir(&cwd) {
                if let Some(local) = GlobalConfig::load_local(&proj) {
                    cfg.apply_local(local);
                }
            }
            cfg
        };
        let repos: Vec<Box<dyn freight::registry::PackageRepo>> = if let Some(rname) = repo {
            match repo_by_name(rname, &config) {
                Ok(r) => vec![r],
                Err(e) => {
                    print_error(&e.to_string());
                    return;
                }
            }
        } else {
            registries_in_order(&config)
        };

        for r in &repos {
            let label = if r.repo_key().is_empty() {
                "freight.dev"
            } else {
                r.repo_key()
            };
            match r.lookup(package, None) {
                Ok(Some(info)) => {
                    println!(
                        "{} {}",
                        info.name.bold().bright_blue(),
                        format!("(via {label})").bright_black()
                    );
                    if let Some(desc) = &info.description {
                        println!("  {desc}");
                    }
                    if let Some(readme) = r.fetch_readme(&info.name) {
                        println!();
                        print_readme_excerpt(&readme);
                    }
                    println!();
                    println!("  {:<16}  {}", "version".bold(), "status".bold());
                    println!("  {}", "─".repeat(30).bright_black());
                    for v in &info.versions {
                        let yanked = if v.checksum.is_none() { "" } else { "" };
                        println!("  {:<16}  {yanked}", v.version.bright_blue());
                    }

                    if let Some(latest) = info.versions.first() {
                        if !latest.dependencies.is_empty() {
                            println!();
                            println!("  {}", "dependencies".bold());
                            println!("  {}", "─".repeat(30).bright_black());
                            let mut deps: Vec<_> = latest.dependencies.iter().collect();
                            deps.sort_by_key(|(k, _)| k.as_str());
                            for (name, ver) in deps {
                                println!("  {:<24}  {}", name.bright_blue(), ver.bright_black());
                            }
                        }
                    }
                    return;
                }
                Ok(None) => continue,
                Err(e) => {
                    print_warning(&format!("{label}: {e}"));
                    continue;
                }
            }
        }
        print_error(&format!("`{package}` not found in any configured registry"));
        return;
    }

    let (project_dir, manifest) = match locate_project() {
        Ok(p) => p,
        Err(e) => {
            print_error(&e.to_string());
            return;
        }
    };

    print_current_package_info(&project_dir, &manifest);
}

fn print_readme_excerpt(readme: &str) {
    let mut output = String::new();
    let mut h2_count = 0;

    for line in readme.lines() {
        if line.starts_with("## ") || line.starts_with("## ") {
            h2_count += 1;
            if h2_count > 1 {
                break;
            }
            let title = line.trim_start_matches('#').trim();
            output.push_str(&format!("  {}\n", title.bold()));
            continue;
        }
        if line.starts_with("# ") {
            continue;
        }
        if line.trim_start().starts_with('<')
            || line.contains("shields.io")
            || line.contains("badge")
        {
            continue;
        }
        if line
            .chars()
            .all(|c| c == '-' || c == '=' || c == '*' || c.is_whitespace())
            && !line.is_empty()
        {
            continue;
        }

        let stripped = strip_inline_md(line);
        output.push_str(&format!("  {stripped}\n"));

        if output.len() > 500 {
            output.push_str("  …\n");
            break;
        }
    }

    let trimmed = output.trim_end();
    if !trimmed.is_empty() {
        println!("{trimmed}");
    }
}

fn strip_inline_md(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' | '_' => {
                if chars.peek() == Some(&c) {
                    chars.next();
                }
            }
            '`' => {
                while chars.peek().map(|&x| x != '`').unwrap_or(false) {
                    out.push(chars.next().unwrap());
                }
                chars.next();
            }
            '[' => {
                while let Some(&ch) = chars.peek() {
                    if ch == ']' {
                        chars.next();
                        break;
                    }
                    out.push(chars.next().unwrap());
                }
                if chars.peek() == Some(&'(') {
                    chars.next();
                    while chars.peek().map(|&x| x != ')').unwrap_or(false) {
                        chars.next();
                    }
                    chars.next();
                }
            }
            _ => out.push(c),
        }
    }
    out
}

fn print_current_package_info(project_dir: &Path, manifest: &Manifest) {
    println!("{} {}", manifest.package.name, manifest.package.version);

    print_optional_field("description", non_empty(&manifest.package.description));
    print_optional_list("authors", &manifest.package.authors);
    print_optional_field("license", non_empty(&manifest.package.license));
    print_optional_field("repository", manifest.package.repository.as_deref());
    print_optional_field("readme", manifest.package.readme.as_deref());
    print_optional_list("keywords", &manifest.package.keywords);
    print_optional_field("supports", manifest.package.supports.as_deref());
    print_optional_list("provides", &manifest.package.provides);
    print_status(
        "manifest",
        &project_dir.join("freight.toml").display().to_string(),
    );

    if !manifest.language.is_empty() {
        let mut languages: Vec<_> = manifest.language.keys().map(String::as_str).collect();
        languages.sort_unstable();
        print_status("languages", &languages.join(", "));
    }

    if let Some(lib) = &manifest.lib {
        print_status("library", &format!("{:?}", lib.lib_type).to_lowercase());
    }

    if !manifest.bins.is_empty() {
        let mut bins: Vec<_> = manifest.bins.iter().map(|bin| bin.name.as_str()).collect();
        bins.sort_unstable();
        print_status("binaries", &bins.join(", "));
    }

    print_dependency_summary("dependencies", &manifest.dependencies);
    print_dependency_summary("dev-deps", &manifest.dev_dependencies);

    if !manifest.features.is_empty() {
        let mut features: Vec<_> = manifest.features.keys().map(String::as_str).collect();
        features.sort_unstable();
        print_status("features", &features.join(", "));
    }
}

fn non_empty(value: &str) -> Option<&str> {
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

fn print_optional_field(label: &str, value: Option<&str>) {
    if let Some(value) = value {
        if !value.is_empty() {
            print_status(label, value);
        }
    }
}

fn print_optional_list(label: &str, values: &[String]) {
    if !values.is_empty() {
        print_status(label, &values.join(", "));
    }
}

fn print_dependency_summary(label: &str, deps: &std::collections::HashMap<String, Dependency>) {
    if deps.is_empty() {
        return;
    }

    let mut names: Vec<_> = deps.keys().map(String::as_str).collect();
    names.sort_unstable();
    print_status(label, &names.join(", "));
}
