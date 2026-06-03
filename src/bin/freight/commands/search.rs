use freight_core::registry::repos::{registries_in_order, repo_by_name};
use freight_core::toolchain::cache::GlobalConfig;

use crate::output::{print_error, print_status, print_warning};
use owo_colors::OwoColorize;

#[derive(clap::Args)]
pub struct Args {
    /// Package name, keyword (#tag), or user (@username) to look up
    pub query: String,
    /// Registry to search (default: all configured registries in order)
    #[arg(long, value_name = "NAME")]
    pub repo: Option<String>,
}

impl Args {
    pub fn run(self) {
        cmd_search(&self.query, self.repo.as_deref());
    }
}

fn cmd_search(query: &str, repo: Option<&str>) {
    let config = {
        let mut cfg = GlobalConfig::load();
        let cwd = std::env::current_dir().unwrap_or_default();
        if let Some(proj) = freight_core::manifest::find_manifest_dir(&cwd) {
            if let Some(local) = GlobalConfig::load_local(&proj) {
                cfg.apply_local(local);
            }
        }
        cfg
    };

    let repos: Vec<Box<dyn freight_core::registry::PackageRepo>> = if let Some(rname) = repo {
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

    // @user → show user profile
    if let Some(username) = query.strip_prefix('@') {
        let username = username.trim();
        for r in &repos {
            if let Some(profile) = r.fetch_user_profile(username) {
                println!("@{}", profile.username.bright_blue().bold());
                if profile.packages.is_empty() {
                    println!("  no packages published");
                } else {
                    println!(
                        "  {:<32}  {:<12}  {}",
                        "package".bold(),
                        "version".bold(),
                        "description".bold()
                    );
                    println!("  {}", "─".repeat(68).bright_black());
                    for p in &profile.packages {
                        println!(
                            "  {:<32}  {:<12}  {}",
                            p.name.bright_blue(),
                            p.version.as_deref().unwrap_or("—").bright_black(),
                            p.description.as_deref().unwrap_or("").dimmed()
                        );
                    }
                }
                return;
            }
        }
        println!("user `@{username}` not found");
        return;
    }

    // #keyword → keyword-exact search; plain text → full-text search
    let is_keyword = query.starts_with('#');
    let display_query = if is_keyword { &query[1..] } else { query };

    let mut any = false;
    for r in &repos {
        let label = if r.repo_key().is_empty() {
            "freight.dev"
        } else {
            r.repo_key()
        };
        match r.search(query) {
            Ok(results) if !results.is_empty() => {
                if !any {
                    if is_keyword {
                        println!("packages tagged  #{}", display_query.bright_blue());
                        println!("{}", "─".repeat(72).bright_black());
                    }
                    println!(
                        "{:<32}  {:<12}  {}",
                        "name".bold(),
                        "latest".bold(),
                        "description".bold()
                    );
                    println!("{}", "─".repeat(72).bright_black());
                }
                for pkg in &results {
                    println!(
                        "{:<32}  {:<12}  {}",
                        pkg.name.bright_blue(),
                        pkg.latest.bright_black(),
                        pkg.description.as_deref().unwrap_or("").dimmed()
                    );
                }
                any = true;
            }
            Ok(_) => {
                if is_keyword {
                    print_status(label, &format!("no packages tagged `#{display_query}`"));
                } else {
                    print_status(label, &format!("no results for `{query}`"));
                }
            }
            Err(e) => {
                print_warning(&format!("{label}: {e}"));
            }
        }
    }

    if !any {
        if is_keyword {
            println!("no packages tagged `#{display_query}`");
        } else {
            println!("no packages found matching `{query}`");
        }
    }
}
