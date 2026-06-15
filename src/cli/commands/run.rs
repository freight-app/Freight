use std::env;
use std::path::PathBuf;
use std::process::Command;

use freight::build::{build_examples_with, build_project_at, build_project_with};
use freight::manifest::{find_manifest_dir, load_manifest, load_workspace_manifest};

use crate::output::print_error;

#[derive(clap::Args)]
pub struct Args {
    #[arg(long)]
    pub release: bool,
    /// Binary to run when the project has multiple [[bin]] targets
    #[arg(long, value_name = "NAME")]
    pub bin: Option<String>,
    /// Run an example (from examples/ or [[example]]) instead of a binary
    #[arg(long, value_name = "NAME", conflicts_with = "bin")]
    pub example: Option<String>,
    /// Activate specific features (comma-separated or repeated)
    #[arg(long, value_name = "FEATURES", value_delimiter = ',')]
    pub features: Vec<String>,
    /// Do not activate default features
    #[arg(long)]
    pub no_default_features: bool,
    /// Arguments to pass to the binary
    #[arg(last = true)]
    pub args: Vec<String>,
    /// Enable sanitizers for this run (e.g. address,undefined). Overrides the profile setting.
    #[arg(long, value_name = "LIST", value_delimiter = ',')]
    pub sanitize: Vec<String>,
    /// Select a specific workspace member to run
    #[arg(long, short = 'p', value_name = "PACKAGE")]
    pub package: Option<String>,
    #[command(flatten)]
    pub build: super::common::BuildFlags,
}

impl Args {
    pub fn run(self) {
        self.build.apply();
        if let Some(example) = self.example.as_deref() {
            cmd_run_example(
                self.release,
                example,
                &self.features,
                !self.no_default_features,
                &self.sanitize,
                &self.args,
            );
            return;
        }
        cmd_run(
            self.release,
            self.package.as_deref(),
            self.bin.as_deref(),
            &self.features,
            !self.no_default_features,
            &self.args,
            &self.sanitize,
        );
    }
}

fn cmd_run_example(
    release: bool,
    example: &str,
    features: &[String],
    use_defaults: bool,
    sanitize: &[String],
    run_args: &[String],
) {
    let profile = if release { "release" } else { "dev" };
    if at_workspace_root() {
        print_error("`--example` is not supported at a workspace root — run it from a member");
        return;
    }
    let output = match build_examples_with(
        profile,
        Some(example),
        features,
        use_defaults,
        sanitize,
        &super::build::make_progress(),
    ) {
        Ok(o) => o,
        Err(e) => {
            println!();
            print_error(&e.to_string());
            return;
        }
    };
    match output.binaries.as_slice() {
        [] => print_error(&format!("no example named {example:?}")),
        [bin] => {
            println!();
            use owo_colors::OwoColorize;
            println!("    {} {}", "Running".bold().green(), bin.display());
            println!();
            let status = Command::new(bin).args(run_args).status();
            match status {
                Ok(s) if !s.success() => {
                    if let Some(code) = s.code() {
                        print_error(&format!("process exited with code {code}"));
                    }
                }
                Err(e) => print_error(&format!("failed to run example: {e}")),
                Ok(_) => {}
            }
        }
        _ => print_error("multiple examples matched — this is a bug"),
    }
}

pub fn cmd_run(
    release: bool,
    package: Option<&str>,
    bin: Option<&str>,
    features: &[String],
    use_defaults: bool,
    run_args: &[String],
    sanitize: &[String],
) {
    let profile = if release { "release" } else { "dev" };

    if at_workspace_root() {
        let Some(pkg) = package else {
            print_error("`freight run` is not supported at workspace root — use `-p <package>` to select a member");
            return;
        };
        let member_dir = match find_workspace_member_dir(pkg) {
            Some(d) => d,
            None => {
                print_error(&format!("package `{pkg}` not found in workspace"));
                return;
            }
        };
        let output = match build_project_at(
            &member_dir,
            profile,
            features,
            use_defaults,
            None,
            sanitize,
            &super::build::make_progress(),
            None,
        ) {
            Ok(o) => o,
            Err(e) => {
                println!();
                print_error(&e.to_string());
                return;
            }
        };
        let default_run = load_manifest(&member_dir)
            .ok()
            .and_then(|m| m.package.default_run);
        run_binary(output, bin, default_run.as_deref(), run_args);
        return;
    }

    if package.is_some() {
        print_error("`-p` can only be used at a workspace root");
        return;
    }

    let output = match build_project_with(
        profile,
        features,
        use_defaults,
        sanitize,
        &super::build::make_progress(),
    ) {
        Ok(o) => o,
        Err(e) => {
            println!();
            print_error(&e.to_string());
            return;
        }
    };

    let default_run = env::current_dir()
        .ok()
        .and_then(|cwd| find_manifest_dir(&cwd))
        .and_then(|dir| load_manifest(&dir).ok())
        .and_then(|m| m.package.default_run);
    run_binary(output, bin, default_run.as_deref(), run_args);
}

fn run_binary(
    output: freight::build::BuildOutput,
    bin: Option<&str>,
    default_run: Option<&str>,
    run_args: &[String],
) {
    let candidate: Option<std::path::PathBuf> = match bin {
        Some(name) => {
            let matched: Vec<_> = output
                .binaries
                .iter()
                .filter(|p| p.file_name().and_then(|n| n.to_str()) == Some(name))
                .cloned()
                .collect();
            match matched.as_slice() {
                [b] => Some(b.clone()),
                [] => {
                    print_error(&format!(
                        "no binary named {name:?} — available: {}",
                        output
                            .binaries
                            .iter()
                            .filter_map(|p| p.file_name()?.to_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                    return;
                }
                _ => Some(matched[0].clone()),
            }
        }
        None => match output.binaries.as_slice() {
            [] => {
                print_error("no binary target produced — add a [[bin]] section to freight.toml");
                return;
            }
            [b] => Some(b.clone()),
            multiple => {
                // Fall back to the manifest's `default-run` when set.
                let chosen = default_run.and_then(|name| {
                    multiple
                        .iter()
                        .find(|p| p.file_name().and_then(|n| n.to_str()) == Some(name))
                        .cloned()
                });
                match chosen {
                    Some(b) => Some(b),
                    None => {
                        let avail = multiple
                            .iter()
                            .filter_map(|p| p.file_name()?.to_str())
                            .collect::<Vec<_>>()
                            .join(", ");
                        if let Some(name) = default_run {
                            print_error(&format!(
                                "default-run = {name:?} does not match any built binary: {avail}"
                            ));
                        } else {
                            print_error(&format!(
                                "multiple [[bin]] targets — use --bin <name> to select one \
                                 (or set package.default-run): {avail}"
                            ));
                        }
                        return;
                    }
                }
            }
        },
    };

    if let Some(bin_path) = candidate {
        println!();
        use owo_colors::OwoColorize;
        println!("    {} {}", "Running".bold().green(), bin_path.display());
        println!();
        let status = Command::new(&bin_path).args(run_args).status();
        match status {
            Ok(s) if !s.success() => {
                if let Some(code) = s.code() {
                    print_error(&format!("process exited with code {code}"));
                }
            }
            Err(e) => print_error(&format!("failed to run binary: {e}")),
            Ok(_) => {}
        }
    }
}

fn find_workspace_member_dir(pkg: &str) -> Option<PathBuf> {
    let cwd = env::current_dir().ok()?;
    let ws_dir = find_manifest_dir(&cwd)?;
    let ws = load_workspace_manifest(&ws_dir)?;
    ws.members.iter().find_map(|m| {
        let dir = ws_dir.join(m.trim_end_matches('/'));
        if dir.file_name().and_then(|n| n.to_str()) == Some(pkg) {
            Some(dir)
        } else {
            None
        }
    })
}

fn at_workspace_root() -> bool {
    let Ok(cwd) = env::current_dir() else {
        return false;
    };
    let Some(dir) = find_manifest_dir(&cwd) else {
        return false;
    };
    load_workspace_manifest(&dir).is_some()
}
