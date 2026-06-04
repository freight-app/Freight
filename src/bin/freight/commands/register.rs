use freight_core::registry::freight_registry::FreightRegistry;
use freight_core::registry::DEFAULT_REGISTRY_URL;
use freight_core::toolchain::cache::{Credentials, GlobalConfig, RegistryConfig};

use crate::output::{print_error, print_status, print_success, print_warning};

#[derive(clap::Args)]
pub struct Args {
    /// Registry base URL (default: first configured registry or https://freight.dev)
    #[arg(long, value_name = "URL")]
    pub registry: Option<String>,
    /// Username for the new account
    #[arg(long, value_name = "NAME")]
    pub username: Option<String>,
    /// Email address for the new account (optional)
    #[arg(long, value_name = "EMAIL")]
    pub email: Option<String>,
    /// Name for the initial API token (default: init)
    #[arg(long, value_name = "NAME")]
    pub token_name: Option<String>,
    /// Skip the interactive TUI and use plain CLI prompts instead
    #[arg(long)]
    pub notui: bool,
}

impl Args {
    pub fn run(self) {
        if self.notui {
            cmd_register(
                self.registry.as_deref(),
                self.username.as_deref(),
                self.email.as_deref(),
                self.token_name.as_deref(),
            );
        } else {
            let url = super::common::resolve_registry_url(self.registry.as_deref());
            match crate::tui::register::run(url.clone(), self.username, self.email, self.token_name)
            {
                Ok((uname, token)) => {
                    let name = super::common::registry_name_for(&url);
                    match Credentials::save(&name, &token) {
                        Ok(()) => crate::output::print_success(&format!(
                            "registered as `{uname}` — token stored in system keychain"
                        )),
                        Err(e) => {
                            crate::output::print_error(&e.to_string());
                            std::process::exit(1);
                        }
                    }
                }
                Err(e) if e.to_string() == "cancelled" => {}
                Err(e) => {
                    crate::output::print_error(&e.to_string());
                    std::process::exit(1);
                }
            }
        }
    }
}

fn cmd_register(
    registry_url: Option<&str>,
    username_arg: Option<&str>,
    email_arg: Option<&str>,
    token_name_arg: Option<&str>,
) {
    let config = GlobalConfig::load();

    let url = registry_url
        .map(str::to_string)
        .or_else(|| config.registries.first().map(|r| r.url.clone()))
        .unwrap_or_else(|| DEFAULT_REGISTRY_URL.to_string());

    let reg_name = config
        .registries
        .iter()
        .find(|r| r.url == url)
        .map(|r| r.name.clone())
        .unwrap_or_else(|| "freight".to_string());

    let username = match username_arg {
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

    if username.is_empty() {
        print_error("username cannot be empty");
        std::process::exit(1);
    }

    let password = {
        use std::io::{self, Write};
        print!("Password: ");
        io::stdout().flush().ok();
        let mut p1 = String::new();
        io::stdin().read_line(&mut p1).ok();
        let p1 = p1.trim().to_string();

        print!("Confirm password: ");
        io::stdout().flush().ok();
        let mut p2 = String::new();
        io::stdin().read_line(&mut p2).ok();
        let p2 = p2.trim().to_string();

        if p1 != p2 {
            print_error("passwords do not match");
            std::process::exit(1);
        }
        if p1.len() < 8 {
            print_error("password must be at least 8 characters");
            std::process::exit(1);
        }
        p1
    };

    let cfg = RegistryConfig {
        name: reg_name.clone(),
        url: url.clone(),
        token: None,
    };
    let registry = FreightRegistry::from_config(&cfg);

    print_status(
        "register",
        &format!("creating account `{username}` on {url}…"),
    );

    match registry.register_user(&username, &password, email_arg, token_name_arg) {
        Ok((_, token)) => match Credentials::save(&reg_name, &token) {
            Ok(()) => {
                print_success(&format!(
                    "registered as `{username}` — token stored in system keychain"
                ));
            }
            Err(e) => {
                print_success(&format!("registered as `{username}`"));
                print_warning(&format!("could not save token to keychain: {e}"));
            }
        },
        Err(e) => {
            print_error(&e.to_string());
            std::process::exit(1);
        }
    }
}
