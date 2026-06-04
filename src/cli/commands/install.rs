use std::path::PathBuf;

use freight::install::{install_project, installer_project, package_project, InstallOptions, InstalledKind};
use freight::manifest::{find_manifest_dir, load_workspace_manifest};

use crate::output::{print_error, print_status, print_success, print_warning};

#[derive(clap::Args)]
pub struct Args {
    /// Installation prefix (default: /usr/local)
    #[arg(long, value_name = "PATH", default_value = "/usr/local")]
    pub prefix: String,
    /// Staging root prepended before prefix (for package managers / fakeroot)
    #[arg(long, value_name = "PATH")]
    pub destdir: Option<String>,
    /// Install release build (default: true)
    #[arg(long, default_value_t = true)]
    pub release: bool,
    /// Skip the build step; install from existing target/ outputs
    #[arg(long)]
    pub no_build: bool,
    /// Cross-compilation target triple (e.g. aarch64-linux-gnu)
    #[arg(long, value_name = "TRIPLE")]
    pub target: Option<String>,
    #[command(flatten)]
    pub build: super::common::BuildFlags,
}

impl Args {
    pub fn run(self) {
        self.build.apply();
        cmd_install(
            Some(&self.prefix),
            self.destdir.as_deref(),
            self.release,
            self.no_build,
            self.target.as_deref(),
        );
    }
}

#[derive(clap::Args)]
pub struct PackageArgs {
    /// Package the release build (default: true)
    #[arg(long, default_value_t = true)]
    pub release: bool,
    /// Target triples to package, comma-separated (e.g. aarch64-linux-gnu,x86_64-linux-gnu).
    /// Omit for a native build. Unsupported combinations are skipped with a warning.
    #[arg(long, value_name = "TRIPLES", value_delimiter = ',')]
    pub target: Vec<String>,
    /// Produce a native installer instead of a plain archive.
    /// Linux → .deb  |  macOS → .dmg  |  Windows → NSIS .exe
    #[arg(long)]
    pub installer: bool,
    #[command(flatten)]
    pub build: super::common::BuildFlags,
}

impl PackageArgs {
    pub fn run(self) {
        self.build.apply();
        cmd_package(self.release, &self.target, self.installer);
    }
}

pub fn cmd_install(
    prefix: Option<&str>,
    destdir: Option<&str>,
    release: bool,
    no_build: bool,
    target: Option<&str>,
) {
    let cwd = std::env::current_dir().expect("cannot read cwd");

    let opts = InstallOptions {
        prefix: prefix.map(PathBuf::from).unwrap_or_else(|| {
            if cfg!(windows) {
                PathBuf::from(r"C:\Program Files")
            } else {
                PathBuf::from("/usr/local")
            }
        }),
        destdir: destdir.map(PathBuf::from),
        release,
        no_build,
        target: target.map(str::to_string),
    };

    let display_prefix = opts
        .destdir
        .as_ref()
        .map(|d| format!("{} (destdir: {})", opts.prefix.display(), d.display()))
        .unwrap_or_else(|| opts.prefix.display().to_string());

    if let Some(ws) = load_workspace_manifest(&cwd) {
        let mut total = 0usize;
        let mut any_error = false;
        for member in &ws.members {
            let member_dir = cwd.join(member);
            use owo_colors::OwoColorize;
            print_status(
                "Installing",
                &format!("{} → {}", member.bold(), display_prefix),
            );
            match install_project(&member_dir, &opts) {
                Ok(result) => {
                    for item in &result.items {
                        if !matches!(item.kind, InstalledKind::Symlink) {
                            print_status(
                                &format!("  {} ({})", "Install".to_string(), item.kind.label()),
                                &item.dst.display().to_string(),
                            );
                        }
                    }
                    total += result.items.len();
                }
                Err(e) => {
                    print_error(&format!("{member}: {e}"));
                    any_error = true;
                }
            }
        }
        if !any_error {
            print_success(&format!(
                "{total} file{} installed",
                if total == 1 { "" } else { "s" }
            ));
        }
        return;
    }

    let project_dir = find_manifest_dir(&cwd).unwrap_or(cwd);
    print_status("Installing", &display_prefix);

    match install_project(&project_dir, &opts) {
        Ok(result) => {
            for item in &result.items {
                if !matches!(item.kind, InstalledKind::Symlink) {
                    print_status(
                        &format!("  {} ({})", "Install".to_string(), item.kind.label()),
                        &item.dst.display().to_string(),
                    );
                }
            }
            let n = result.items.len();
            print_success(&format!(
                "{n} file{} installed",
                if n == 1 { "" } else { "s" }
            ));
        }
        Err(e) => print_error(&e.to_string()),
    }
}

pub fn cmd_package(release: bool, targets: &[String], installer: bool) {
    let cwd = std::env::current_dir().expect("cannot read cwd");
    let project_dir = find_manifest_dir(&cwd).unwrap_or(cwd);

    let pack = |target: Option<&str>| {
        if installer {
            installer_project(&project_dir, release, target)
        } else {
            package_project(&project_dir, release, target)
        }
    };

    // No explicit targets → native build.
    if targets.is_empty() {
        let label = if installer { "Installer" } else { "Packaging" };
        print_status(label, &project_dir.display().to_string());
        match pack(None) {
            Ok(archive) => print_success(&format!("→ {}", archive.display())),
            Err(e) => print_error(&e.to_string()),
        }
        return;
    }

    let mut succeeded = 0usize;
    for target in targets {
        let label = if installer { "Installer" } else { "Packaging" };
        print_status(label, &format!("{} [{}]", project_dir.display(), target));
        match pack(Some(target)) {
            Ok(archive) => {
                print_success(&format!("→ {}", archive.display()));
                succeeded += 1;
            }
            Err(e) => {
                print_warning(&format!("skipping {target}: {e}"));
            }
        }
    }

    if succeeded == 0 {
        print_error("all targets failed — no archives produced");
    }
}
