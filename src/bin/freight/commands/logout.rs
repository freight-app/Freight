use freight_core::registry::DEFAULT_REGISTRY_URL;
use freight_core::toolchain::cache::{Credentials, GlobalConfig};

use crate::output::{print_error, print_success, print_warning};

#[derive(clap::Args)]
pub struct Args {
    /// Registry base URL or name (default: first configured registry or https://freight.dev)
    #[arg(value_name = "REGISTRY")]
    pub registry: Option<String>,
}

impl Args {
    pub fn run(self) {
        let config = GlobalConfig::load();

        // Accept either a URL or a registry name.
        let (name, url) = if let Some(ref given) = self.registry {
            if let Some(r) = config.registries.iter().find(|r| &r.name == given || &r.url == given) {
                (r.name.clone(), r.url.clone())
            } else {
                // Treat as URL; derive a name.
                (super::common::registry_name_for(given), given.clone())
            }
        } else {
            let url = config
                .registries
                .first()
                .map(|r| r.url.clone())
                .unwrap_or_else(|| DEFAULT_REGISTRY_URL.to_string());
            let name = super::common::registry_name_for(&url);
            (name, url)
        };

        match Credentials::delete(&name) {
            Ok(()) => print_success(&format!("logged out of {url} — token removed from keychain")),
            Err(e) => {
                print_warning(&format!("could not remove token from keychain: {e}"));
                print_error("logout failed");
                std::process::exit(1);
            }
        }
    }
}
