use std::path::{Path, PathBuf};

use freight::dep_cmds::{regen_lock, RegenLockOutcome};
use freight::manifest::find_manifest_dir;

use crate::output::{print_error, print_warning};

// ── Shared build flags ────────────────────────────────────────────────────────

/// Common flags for commands that invoke the freight build engine.
/// Flatten into a command's `Args` with `#[command(flatten)]`.
#[derive(clap::Args, Clone)]
pub struct BuildFlags {
    /// Print every compiler and linker invocation
    #[arg(long, short = 'v')]
    pub verbose: bool,
    /// Number of parallel compile jobs (default: min(logical CPUs, 6))
    #[arg(long, short = 'j', value_name = "N")]
    pub jobs: Option<usize>,
    /// Do not access the network; use only dependencies already in `.pkgs/`
    #[arg(long)]
    pub offline: bool,
    /// Require freight.lock to be up to date; never rewrite it
    #[arg(long)]
    pub locked: bool,
    /// Equivalent to `--offline --locked`
    #[arg(long)]
    pub frozen: bool,
}

impl BuildFlags {
    /// Set `FREIGHT_VERBOSE` / `FREIGHT_OFFLINE` / `FREIGHT_LOCKED` and configure
    /// the rayon thread pool. Call once, before any build engine use.
    pub fn apply(&self) {
        // Safety: single-threaded here; rayon workers not yet started.
        unsafe {
            if self.verbose {
                std::env::set_var("FREIGHT_VERBOSE", "1");
            }
            if self.offline || self.frozen {
                std::env::set_var("FREIGHT_OFFLINE", "1");
            }
            if self.locked || self.frozen {
                std::env::set_var("FREIGHT_LOCKED", "1");
            }
        }
        apply_jobs(self.jobs);
    }
}

/// Configure the rayon thread pool from an optional `--jobs N` value.
pub fn apply_jobs(jobs: Option<usize>) {
    let n = jobs.unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .min(6)
    });
    rayon::ThreadPoolBuilder::new()
        .num_threads(n)
        .build_global()
        .ok();
}

pub fn locate_project_dir() -> Option<PathBuf> {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            print_error(&format!("cannot read cwd: {e}"));
            return None;
        }
    };
    match find_manifest_dir(&cwd) {
        Some(d) => Some(d),
        None => {
            print_error("no freight.toml found");
            None
        }
    }
}

pub fn refresh_lock(project_dir: &Path) {
    match regen_lock(project_dir) {
        Ok(RegenLockOutcome::Wrote) => {}
        Ok(RegenLockOutcome::Skipped) => {
            print_warning(
                "freight.lock not updated — run `freight fetch` after downloading dependencies",
            );
        }
        Err(e) => {
            print_error(&format!("cannot write freight.lock: {e}"));
        }
    }
}

/// Resolve the registry URL from the explicit flag, first configured registry,
/// or the default freight.dev URL.
pub fn resolve_registry_url(registry: Option<&str>) -> String {
    use freight::toolchain::cache::GlobalConfig;
    registry
        .map(str::to_string)
        .or_else(|| {
            GlobalConfig::load()
                .registries
                .into_iter()
                .next()
                .map(|r| r.url)
        })
        .unwrap_or_else(|| "https://freight.dev".to_string())
}

/// Return the configured registry name for a URL, falling back to "freight".
pub fn registry_name_for(url: &str) -> String {
    use freight::toolchain::cache::GlobalConfig;
    GlobalConfig::load()
        .registries
        .iter()
        .find(|r| r.url == url)
        .map(|r| r.name.clone())
        .unwrap_or_else(|| "freight".to_string())
}

/// Call /api/v1/users/login with username + password and save the resulting token.
/// Used by `freight login --username NAME --password PASS` (non-TUI path).
pub fn login_with_credentials(
    registry_url: Option<&str>,
    username: Option<&str>,
    password: Option<&str>,
) {
    use sha2::{Digest, Sha256};

    let url = resolve_registry_url(registry_url);
    let name = registry_name_for(&url);

    let username = match username {
        Some(u) => u.to_string(),
        None => {
            use std::io::{self, Write};
            print!("Username: ");
            io::stdout().flush().ok();
            let mut u = String::new();
            io::stdin().read_line(&mut u).ok();
            u.trim().to_string()
        }
    };
    let password = match password {
        Some(p) => p.to_string(),
        None => {
            use std::io::{self, Write};
            print!("Password: ");
            io::stdout().flush().ok();
            let mut p = String::new();
            io::stdin().read_line(&mut p).ok();
            p.trim().to_string()
        }
    };

    if username.is_empty() {
        crate::output::print_error("username cannot be empty");
        std::process::exit(1);
    }

    // SHA-256 pre-hash — registry stores Argon2id(SHA-256(plaintext))
    let pw_hash: String = Sha256::digest(password.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();

    let rt = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(e) => {
            crate::output::print_error(&e.to_string());
            std::process::exit(1);
        }
    };
    let token = match rt.block_on(async {
        let resp = reqwest::Client::new()
            .post(format!("{url}/api/v1/users/login"))
            .json(&serde_json::json!({ "username": username, "password": pw_hash }))
            .send()
            .await?;
        let body: serde_json::Value = resp.json().await?;
        if let Some(t) = body["token"].as_str() {
            anyhow::Ok(t.to_string())
        } else {
            let detail = body["errors"][0]["detail"]
                .as_str()
                .unwrap_or("login failed");
            anyhow::bail!("{detail}")
        }
    }) {
        Ok(t) => t,
        Err(e) => {
            crate::output::print_error(&format!("login failed: {e}"));
            std::process::exit(1);
        }
    };

    match freight::toolchain::cache::Credentials::save(&name, &token) {
        Ok(()) => crate::output::print_success(&format!(
            "logged in as `{username}` — token stored in system keychain"
        )),
        Err(e) => {
            crate::output::print_error(&e.to_string());
            std::process::exit(1);
        }
    }
}
