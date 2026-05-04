use std::path::PathBuf;

use freight_core::install::{install_project, package_project, InstalledKind, InstallOptions};
use freight_core::manifest::find_manifest_dir;

use crate::output::{print_error, print_status, print_success, print_warning};

pub fn cmd_install(prefix: Option<&str>, destdir: Option<&str>, release: bool, no_build: bool, target: Option<&str>) {
    let cwd         = std::env::current_dir().expect("cannot read cwd");
    let project_dir = find_manifest_dir(&cwd).unwrap_or(cwd);

    let opts = InstallOptions {
        prefix:   prefix.map(PathBuf::from).unwrap_or_else(|| {
            if cfg!(windows) { PathBuf::from(r"C:\Program Files") }
            else             { PathBuf::from("/usr/local") }
        }),
        destdir:  destdir.map(PathBuf::from),
        release,
        no_build,
        target:   target.map(str::to_string),
    };

    let display_prefix = opts.destdir.as_ref()
        .map(|d| format!("{} (destdir: {})", opts.prefix.display(), d.display()))
        .unwrap_or_else(|| opts.prefix.display().to_string());

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
            print_success(&format!("{n} file{} installed", if n == 1 { "" } else { "s" }));
        }
        Err(e) => print_error(&e.to_string()),
    }
}

pub fn cmd_package(release: bool, targets: &[String]) {
    let cwd         = std::env::current_dir().expect("cannot read cwd");
    let project_dir = find_manifest_dir(&cwd).unwrap_or(cwd);

    // No explicit targets → native build.
    if targets.is_empty() {
        print_status("Packaging", &project_dir.display().to_string());
        match package_project(&project_dir, release, None) {
            Ok(archive) => print_success(&format!("→ {}", archive.display())),
            Err(e)      => print_error(&e.to_string()),
        }
        return;
    }

    let mut succeeded = 0usize;
    for target in targets {
        print_status("Packaging", &format!("{} [{}]", project_dir.display(), target));
        match package_project(&project_dir, release, Some(target)) {
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
