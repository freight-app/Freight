//! `freight admin` — registry moderation & administration.
//!
//! Every subcommand talks to a registry's admin HTTP API using the token stored
//! for that registry (`freight login`). Authorization is enforced server-side by
//! role tier; a 403 surfaces as a "permission denied" error here. The command is
//! only useful to accounts the server has granted a moderator/admin tier.

use freight::registry::freight_registry::FreightRegistry;
use freight::toolchain::cache::GlobalConfig;

use crate::output::{print_error, print_status, print_success};

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    pub command: AdminCmd,
    /// Registry to operate on (default: first configured registry)
    #[arg(long, short = 'r', value_name = "REGISTRY", global = true)]
    pub registry: Option<String>,
}

#[derive(clap::Subcommand)]
pub enum AdminCmd {
    /// Show registry-wide counts (packages, users, open reports, …)
    Overview,
    /// List abuse/problem reports awaiting triage
    Reports {
        /// Filter by status (e.g. open, resolved, dismissed)
        #[arg(long)]
        status: Option<String>,
    },
    /// Resolve a report (mark the issue as actioned)
    Resolve {
        /// Report id
        id: i64,
        /// Dismiss instead of resolve (no action needed)
        #[arg(long)]
        dismiss: bool,
        /// Note recorded with the resolution
        #[arg(long)]
        note: Option<String>,
    },
    /// List all user accounts and their role tiers
    Users,
    /// Set a user's role tier (user/moderator/admin)
    SetRole {
        /// Username to modify
        user: String,
        /// New role tier
        role: String,
    },
    /// Show the account the current token authenticates as
    Whoami,
}

impl Args {
    pub fn run(self) {
        let Some(registry) = resolve_registry(self.registry.as_deref()) else {
            return;
        };
        match self.command {
            AdminCmd::Overview => cmd_overview(&registry),
            AdminCmd::Reports { status } => cmd_reports(&registry, status.as_deref()),
            AdminCmd::Resolve { id, dismiss, note } => {
                cmd_resolve(&registry, id, dismiss, note.as_deref())
            }
            AdminCmd::Users => cmd_users(&registry),
            AdminCmd::SetRole { user, role } => cmd_set_role(&registry, &user, &role),
            AdminCmd::Whoami => cmd_whoami(&registry),
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

fn cmd_overview(registry: &FreightRegistry) {
    match registry.admin_overview() {
        Ok(o) => {
            println!("packages        {}", o.packages);
            println!("versions        {}", o.versions);
            println!("users           {}", o.users);
            println!("admins          {}", o.admins);
            println!("active tokens   {}", o.active_tokens);
            println!("downloads       {}", o.downloads_total);
            println!("open reports    {}", o.open_reports);
        }
        Err(e) => print_error(&e.to_string()),
    }
}

fn cmd_reports(registry: &FreightRegistry, status: Option<&str>) {
    match registry.list_reports(status) {
        Ok(reports) if reports.is_empty() => println!("No reports."),
        Ok(reports) => {
            for r in reports {
                let version = r.version.as_deref().unwrap_or("*");
                println!(
                    "#{}  [{}]  {}@{}  — {}",
                    r.id, r.status, r.package, version, r.reason
                );
                if let Some(details) = r.details.as_deref().filter(|d| !d.is_empty()) {
                    println!("    {details}");
                }
                if let Some(res) = r.resolution.as_deref().filter(|d| !d.is_empty()) {
                    println!("    resolution: {res}");
                }
            }
        }
        Err(e) => print_error(&e.to_string()),
    }
}

fn cmd_resolve(registry: &FreightRegistry, id: i64, dismiss: bool, note: Option<&str>) {
    let status = if dismiss { "dismissed" } else { "resolved" };
    print_status(status, &format!("report #{id}"));
    match registry.resolve_report(id, status, note.unwrap_or("")) {
        Ok(()) => print_success(&format!("report #{id} {status}")),
        Err(e) => print_error(&e.to_string()),
    }
}

fn cmd_users(registry: &FreightRegistry) {
    match registry.list_users() {
        Ok(users) if users.is_empty() => println!("No users."),
        Ok(users) => {
            for u in users {
                let email = u.email.as_deref().unwrap_or("-");
                println!("{:<20} {:<10} {}", u.username, u.role, email);
            }
        }
        Err(e) => print_error(&e.to_string()),
    }
}

fn cmd_set_role(registry: &FreightRegistry, user: &str, role: &str) {
    print_status("set-role", &format!("{user} → {role}"));
    match registry.set_role(user, role) {
        Ok(()) => print_success(&format!("{user} is now `{role}`")),
        Err(e) => print_error(&e.to_string()),
    }
}

fn cmd_whoami(registry: &FreightRegistry) {
    match registry.me() {
        Ok(me) => {
            let email = me.email.as_deref().unwrap_or("-");
            let role = if me.role.is_empty() { "user" } else { &me.role };
            println!("{}  ({})  {}", me.login, role, email);
        }
        Err(e) => print_error(&e.to_string()),
    }
}
