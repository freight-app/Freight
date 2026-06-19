use freight::build::{test_project_with, test_workspace_with};

use crate::output::{print_error, print_success};

#[derive(clap::Args)]
pub struct Args {
    pub name: Option<String>,
    #[arg(long)]
    pub release: bool,
    /// Activate specific features (comma-separated or repeated)
    #[arg(long, value_name = "FEATURES", value_delimiter = ',')]
    pub features: Vec<String>,
    /// Do not activate default features
    #[arg(long)]
    pub no_default_features: bool,
    /// Enable sanitizers for this run (e.g. address,undefined). Overrides the profile setting.
    #[arg(long, value_name = "LIST", value_delimiter = ',')]
    pub sanitize: Vec<String>,
    /// Select a specific workspace member to test
    #[arg(long, short = 'p', value_name = "PACKAGE")]
    pub package: Option<String>,
    #[command(flatten)]
    pub build: super::common::BuildFlags,
}

impl Args {
    pub fn run(self) {
        self.build.apply();
        cmd_test(
            self.name.as_deref(),
            self.release,
            self.package.as_deref(),
            &self.features,
            !self.no_default_features,
            &self.sanitize,
        );
    }
}

pub fn cmd_test(
    filter: Option<&str>,
    release: bool,
    package: Option<&str>,
    features: &[String],
    use_defaults: bool,
    sanitize: &[String],
) {
    let profile = if release { "release" } else { "debug" };
    let progress = super::build::make_progress();
    if super::build::at_workspace_root() {
        match test_workspace_with(profile, filter, package, features, use_defaults, &progress) {
            Ok(summary) => {
                println!();
                if summary.total == 0 {
                    println!("no test files found in any workspace member");
                    return;
                }
                if summary.failed == 0 {
                    print_success(&format!(
                        "test result: ok. {} passed; 0 failed",
                        summary.passed,
                    ));
                } else {
                    print_error(&format!(
                        "test result: FAILED. {} passed; {} failed",
                        summary.passed, summary.failed,
                    ));
                }
            }
            Err(e) => {
                println!();
                print_error(&e.to_string());
            }
        }
        return;
    }

    if package.is_some() {
        print_error("`-p` can only be used at a workspace root");
        return;
    }

    match test_project_with(profile, filter, features, use_defaults, sanitize, &progress) {
        Ok(summary) => {
            println!();
            if summary.total == 0 {
                println!("no test files found under tests/");
                return;
            }
            if summary.failed == 0 {
                print_success(&format!(
                    "test result: ok. {} passed; 0 failed",
                    summary.passed,
                ));
            } else {
                print_error(&format!(
                    "test result: FAILED. {} passed; {} failed",
                    summary.passed, summary.failed,
                ));
            }
        }
        Err(e) => {
            println!();
            print_error(&e.to_string());
        }
    }
}
