use freight::dep_cmds::{fetch_url_deps, invalidate_url_dep, update_git_deps, GitDepAction};
use freight::manifest::load_manifest;
use freight::manifest::types::Dependency;

use crate::output::{print_error, print_status, print_success};

#[derive(clap::Args)]
pub struct Args {
    pub package: Option<String>,
}

impl Args {
    pub fn run(self) {
        cmd_update(self.package.as_deref());
    }
}

fn cmd_update(package: Option<&str>) {
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

    let target = package.map(|p| p.to_string());

    let path_count = manifest
        .dependencies
        .iter()
        .filter(|(name, dep)| {
            target.as_deref().map_or(true, |t| t == name.as_str())
                && matches!(dep, Dependency::Detailed(d) if d.path.is_some())
        })
        .count();

    match update_git_deps(&project_dir, target.as_deref()) {
        Ok(outcomes) => {
            for o in outcomes {
                match o.action {
                    GitDepAction::Updated => print_success(&format!("updated `{}`", o.name)),
                    GitDepAction::Skipped => {
                        print_status("skip", &format!("`{}` (rev-pinned)", o.name))
                    }
                    _ => {}
                }
            }
        }
        Err(e) => {
            print_error(&e.to_string());
            return;
        }
    }

    let url_count = manifest
        .dependencies
        .iter()
        .filter(|(name, dep)| {
            target.as_deref().map_or(true, |t| t == name.as_str())
                && matches!(dep, Dependency::Detailed(d) if d.url.is_some())
        })
        .count();
    if url_count > 0 {
        for (name, dep) in &manifest.dependencies {
            if target.as_deref().map_or(true, |t| t == name.as_str()) {
                if let Dependency::Detailed(d) = dep {
                    if d.url.is_some() {
                        invalidate_url_dep(&project_dir, name);
                    }
                }
            }
        }
        match fetch_url_deps(&project_dir) {
            Ok(outcomes) => {
                for (name, _) in outcomes {
                    print_success(&format!("re-fetched `{name}`"));
                }
            }
            Err(e) => {
                print_error(&e.to_string());
                return;
            }
        }
    }

    if path_count == 0
        && !manifest
            .dependencies
            .values()
            .any(|d| matches!(d, Dependency::Detailed(dd) if dd.git.is_some()))
        && url_count == 0
    {
        if let Some(pkg) = package {
            print_error(&format!("`{pkg}` not found in [dependencies]"));
        } else {
            println!("no dependencies to update");
        }
        return;
    }

    super::common::refresh_lock(&project_dir);

    if path_count > 0 {
        print_success(&format!("refreshed lockfile for {path_count} path dep(s)"));
    }
}
