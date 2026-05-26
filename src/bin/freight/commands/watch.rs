use std::env;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use freight_core::build::build_project_with;
use freight_core::manifest::find_manifest_dir;

use crate::output::{print_error, print_success};

#[derive(clap::Args)]
pub struct Args {
    #[arg(long)]
    pub release: bool,
    #[command(flatten)]
    pub build: super::common::BuildFlags,
}

impl Args {
    pub fn run(self) {
        self.build.apply();
        cmd_watch(self.release);
    }
}

pub fn cmd_watch(release: bool) {
    let profile = if release { "release" } else { "dev" };

    let Ok(cwd) = env::current_dir() else {
        print_error("cannot read working directory");
        return;
    };
    let Some(project_dir) = find_manifest_dir(&cwd) else {
        print_error("no freight.toml found");
        return;
    };

    let mut watch_paths: Vec<PathBuf> = Vec::new();
    let src_dir = project_dir.join("src");
    if src_dir.exists() { watch_paths.push(src_dir); }
    let manifest = project_dir.join("freight.toml");
    if manifest.exists() { watch_paths.push(manifest); }
    let script = project_dir.join("build.freight");
    if script.exists() { watch_paths.push(script); }
    let include_dir = project_dir.join("include");
    if include_dir.exists() { watch_paths.push(include_dir); }

    let (tx, rx) = mpsc::channel::<notify::Result<Event>>();
    let mut watcher = match RecommendedWatcher::new(tx, notify::Config::default()) {
        Ok(w) => w,
        Err(e) => { print_error(&format!("failed to initialise file watcher: {e}")); return; }
    };

    for path in &watch_paths {
        if let Err(e) = watcher.watch(path, RecursiveMode::Recursive) {
            print_error(&format!("cannot watch {}: {e}", path.display()));
            return;
        }
    }

    use owo_colors::OwoColorize;
    println!("  {} source files — press Ctrl+C to stop", "Watching".bold().cyan());

    run_build(profile, &project_dir);

    let debounce = Duration::from_millis(200);
    loop {
        match rx.recv() {
            Err(_) => break,
            Ok(Err(e)) => { print_error(&format!("watch error: {e}")); continue; }
            Ok(Ok(ev)) => {
                if !is_relevant(&ev) { continue; }
            }
        }
        loop {
            match rx.recv_timeout(debounce) {
                Ok(_) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
        run_build(profile, &project_dir);
    }
}

fn is_relevant(ev: &Event) -> bool {
    matches!(
        ev.kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    )
}

fn run_build(profile: &str, project_dir: &std::path::Path) {
    use owo_colors::OwoColorize;
    println!("\n  {} …", "Rebuilding".bold().cyan());
    match build_project_with(profile, &[], true, &[], &super::build::make_progress()) {
        Ok(output) => {
            println!();
            print_success(&format!(
                "{} ({} compiled, {} up to date)",
                output.package_name, output.compiled, output.skipped,
            ));
            let _ = project_dir;
        }
        Err(e) => { println!(); print_error(&e.to_string()); }
    }
}
