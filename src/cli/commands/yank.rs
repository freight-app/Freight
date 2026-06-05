use freight::manifest::load_manifest;
use freight::registry::freight_registry::FreightRegistry;
use freight::toolchain::cache::GlobalConfig;

use crate::output::{print_error, print_status, print_success};

#[derive(clap::Args)]
pub struct Args {
    /// Package name and version to yank (e.g. mylib@1.0.0)
    /// Omit the package name to use the current project
    pub version: String,
    /// Undo a yank (re-allow installs)
    #[arg(long)]
    pub undo: bool,
    /// Registry to operate on (default: first configured registry)
    #[arg(long, short = 'r', value_name = "REGISTRY")]
    pub registry: Option<String>,
}

impl Args {
    pub fn run(self) {
        cmd_yank(&self.version, self.undo, self.registry.as_deref());
    }
}

fn cmd_yank(version_arg: &str, undo: bool, repo: Option<&str>) {
    let (pkg_name, version) = if let Some(at) = version_arg.find('@') {
        (version_arg[..at].to_string(), &version_arg[at + 1..])
    } else {
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
        (manifest.package.name.clone(), version_arg)
    };

    let config = GlobalConfig::load();
    let registry: FreightRegistry = if let Some(rname) = repo {
        match config.registries.iter().find(|r| r.name == rname) {
            Some(c) => FreightRegistry::from_config(c),
            None => {
                print_error(&format!("unknown registry `{rname}`"));
                return;
            }
        }
    } else {
        match config.registries.first() {
            Some(c) => FreightRegistry::from_config(c),
            None => FreightRegistry::default_registry(),
        }
    };

    let action = if undo { "unyank" } else { "yank" };
    print_status(action, &format!("{pkg_name}@{version}"));

    match registry.yank_version(&pkg_name, version, !undo) {
        Ok(()) => print_success(&format!("{action}ed {pkg_name}@{version}")),
        Err(e) => print_error(&e.to_string()),
    }
}
