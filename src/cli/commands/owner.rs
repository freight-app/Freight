//! `freight owner` — manage a package's members and their per-package roles.
//!
//! Roles (lowest → highest): `publisher` (publish only), `maintainer` (publish +
//! yank), `owner` (full control, incl. managing members). Only members with the
//! `owner` role (or a registry admin) may change membership — enforced
//! server-side, so a 403 surfaces here as "permission denied".

use freight::manifest::load_manifest;
use freight::registry::freight_registry::FreightRegistry;
use freight::toolchain::cache::GlobalConfig;

use crate::output::{print_error, print_status, print_success};

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    pub command: OwnerCmd,
    /// Registry to operate on (default: first configured registry)
    #[arg(long, short = 'r', value_name = "REGISTRY", global = true)]
    pub registry: Option<String>,
}

#[derive(clap::Subcommand)]
pub enum OwnerCmd {
    /// List a package's members and their roles
    List {
        /// Package name (defaults to the current project)
        package: Option<String>,
    },
    /// Add a member (or change their role) on a package
    Add {
        /// Package name
        package: String,
        /// Username to add
        user: String,
        /// Role to grant: publisher, maintainer, or owner
        #[arg(long, default_value = "owner")]
        role: String,
    },
    /// Remove a member from a package
    Remove {
        /// Package name
        package: String,
        /// Username to remove
        user: String,
    },
    /// Change an existing member's role
    SetRole {
        /// Package name
        package: String,
        /// Username to modify
        user: String,
        /// New role: publisher, maintainer, or owner
        role: String,
    },
}

impl Args {
    pub fn run(self) {
        let Some(registry) = resolve_registry(self.registry.as_deref()) else {
            return;
        };
        match self.command {
            OwnerCmd::List { package } => {
                let Some(pkg) = package.or_else(current_package) else {
                    return;
                };
                cmd_list(&registry, &pkg);
            }
            OwnerCmd::Add { package, user, role } => cmd_add(&registry, &package, &user, &role),
            OwnerCmd::Remove { package, user } => cmd_remove(&registry, &package, &user),
            OwnerCmd::SetRole { package, user, role } => {
                cmd_set_role(&registry, &package, &user, &role)
            }
        }
    }
}

/// Build a [`FreightRegistry`] from the named registry, or the first configured
/// one, falling back to the default public registry.
fn resolve_registry(repo: Option<&str>) -> Option<FreightRegistry> {
    let config = GlobalConfig::load();
    match repo {
        Some(rname) => match config.registries.iter().find(|r| r.name == rname) {
            Some(c) => Some(FreightRegistry::from_config(c)),
            None => {
                print_error(&format!("unknown registry `{rname}`"));
                None
            }
        },
        None => Some(match config.registries.first() {
            Some(c) => FreightRegistry::from_config(c),
            None => FreightRegistry::default_registry(),
        }),
    }
}

/// Resolve the current project's package name from `freight.toml`.
fn current_package() -> Option<String> {
    let project_dir = super::common::locate_project_dir()?;
    match load_manifest(&project_dir) {
        Ok(m) => Some(m.package.name),
        Err(e) => {
            print_error(&e.to_string());
            None
        }
    }
}

fn cmd_list(registry: &FreightRegistry, package: &str) {
    match registry.list_package_members(package) {
        Ok(members) if members.is_empty() => println!("No members."),
        Ok(members) => {
            for m in members {
                let role = if m.role.is_empty() { "owner" } else { &m.role };
                println!("{:<24} {}", m.login, role);
            }
        }
        Err(e) => print_error(&e.to_string()),
    }
}

fn cmd_add(registry: &FreightRegistry, package: &str, user: &str, role: &str) {
    print_status("owner", &format!("add {user} to {package} as {role}"));
    match registry.add_package_member(package, user, role) {
        Ok(msg) => print_success(&msg),
        Err(e) => print_error(&e.to_string()),
    }
}

fn cmd_remove(registry: &FreightRegistry, package: &str, user: &str) {
    print_status("owner", &format!("remove {user} from {package}"));
    match registry.remove_package_member(package, user) {
        Ok(msg) => print_success(&msg),
        Err(e) => print_error(&e.to_string()),
    }
}

fn cmd_set_role(registry: &FreightRegistry, package: &str, user: &str, role: &str) {
    print_status("owner", &format!("{user} → {role} on {package}"));
    match registry.set_package_member_role(package, user, role) {
        Ok(()) => print_success(&format!("{user} is now `{role}` on {package}")),
        Err(e) => print_error(&e.to_string()),
    }
}
