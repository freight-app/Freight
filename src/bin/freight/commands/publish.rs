use docify::extract::extract_dir;

use freight_core::manifest::types::Manifest;
use freight_core::manifest::{find_manifest_dir, load_manifest};
use freight_core::registry::freight_registry::FreightRegistry;
use freight_core::registry::host_triple;
use freight_core::toolchain::cache::GlobalConfig;

use crate::output::{print_error, print_status, print_success, print_warning};

#[derive(clap::Args)]
pub struct Args {
    /// Dry run: print what would be uploaded without sending
    #[arg(long)]
    pub dry_run: bool,
    /// Registry to publish to (default: first configured registry)
    #[arg(long, value_name = "NAME")]
    pub repo: Option<String>,
    /// Upload a prebuilt binary tarball for the given triple instead of source.
    /// Omit the triple to use the detected host triple (e.g. x86_64-linux-gnu).
    #[arg(long, value_name = "TRIPLE")]
    pub prebuilt: Option<Option<String>>,
}

impl Args {
    pub fn run(self) {
        if let Some(triple_opt) = self.prebuilt {
            cmd_publish_prebuilt(triple_opt.as_deref(), self.repo.as_deref());
        } else {
            cmd_publish(self.dry_run, self.repo.as_deref());
        }
    }
}

fn cmd_publish(dry_run: bool, repo: Option<&str>) {
    let project_dir = match super::common::locate_project_dir() {
        Some(d) => d,
        None => return,
    };
    let manifest = match load_manifest(&project_dir) {
        Ok(m) => m,
        Err(e) => {
            print_error(&e.to_string());
            return;
        }
    };

    let name = &manifest.package.name;
    let version = &manifest.package.version;
    let description = if manifest.package.description.is_empty() {
        None
    } else {
        Some(manifest.package.description.as_str())
    };
    let license = if manifest.package.license.is_empty() {
        None
    } else {
        Some(manifest.package.license.as_str())
    };

    if dry_run {
        print_status("dry-run", &format!("would publish {name}@{version}"));
        if let Some(d) = description {
            print_status("description", d);
        }
        if let Some(l) = license {
            print_status("license", l);
        }
        return;
    }

    let archive = project_dir
        .join("target")
        .join(format!("{name}-{version}.tar.gz"));
    if let Some(p) = archive.parent() {
        std::fs::create_dir_all(p).ok();
    }

    print_status("packaging", &format!("{name}@{version}"));

    let ok = std::process::Command::new("tar")
        .current_dir(&project_dir)
        .args([
            "--exclude=./target",
            "--exclude=./.freight-build",
            "-czf",
            &archive.to_string_lossy(),
            ".",
        ])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if !ok {
        print_error("failed to create tarball — is `tar` installed?");
        return;
    }

    let tarball = match std::fs::read(&archive) {
        Ok(b) => b,
        Err(e) => {
            print_error(&format!("cannot read tarball: {e}"));
            return;
        }
    };
    let _ = std::fs::remove_file(&archive);

    let config = {
        let mut cfg = GlobalConfig::load();
        if let Some(local) = GlobalConfig::load_local(&project_dir) {
            cfg.apply_local(local);
        }
        cfg
    };

    let registry: FreightRegistry = if let Some(rname) = repo {
        match config.registries.iter().find(|r| r.name == rname) {
            Some(c) => FreightRegistry::from_config(c),
            None => {
                print_error(&format!("unknown registry `{rname}`"));
                return;
            }
        }
    } else {
        match config.registries.first() {
            Some(c) => FreightRegistry::from_config(c),
            None => FreightRegistry::default_registry(),
        }
    };

    print_status(
        "publishing",
        &format!("{name}@{version} ({} bytes)", tarball.len()),
    );

    match registry.publish_package(
        name,
        version,
        None,
        description,
        license,
        &tarball,
        None,
        None,
    ) {
        Ok(()) => print_success(&format!("published {name}@{version}")),
        Err(e) => {
            print_error(&e.to_string());
            return;
        }
    }

    // Extract and upload API docs (non-fatal).
    let src_dir = project_dir.join("src");
    let scan_dir = if src_dir.is_dir() { src_dir } else { project_dir.clone() };
    let items = extract_dir(&scan_dir).items;
    if items.is_empty() {
        print_warning("no doc comments found — skipping docs upload");
    } else {
        print_status(
            "uploading",
            &format!("docs ({} items)", items.len()),
        );
        match docify::to_msgpack(&items) {
            Ok(blob) => {
                if let Err(e) = registry.upload_docs(name, version, &blob) {
                    print_warning(&format!("docs upload failed: {e}"));
                }
            }
            Err(e) => print_warning(&format!("docs serialization failed: {e}")),
        }
    }
}

fn cmd_publish_prebuilt(triple: Option<&str>, repo: Option<&str>) {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            print_error(&format!("cannot read cwd: {e}"));
            return;
        }
    };
    let project_dir = match find_manifest_dir(&cwd) {
        Some(d) => d,
        None => {
            print_error("no freight.toml found");
            return;
        }
    };
    let manifest = match load_manifest(&project_dir) {
        Ok(m) => m,
        Err(e) => {
            print_error(&e.to_string());
            return;
        }
    };

    let triple = triple.map(str::to_string).unwrap_or_else(host_triple);
    let name = &manifest.package.name;
    let version = &manifest.package.version;

    print_status(
        "prebuilt",
        &format!("packaging `{name}@{version}` for {triple}…"),
    );

    let tarball = match build_prebuilt_tarball(&project_dir, &manifest, &triple) {
        Ok(t) => t,
        Err(e) => {
            print_error(&format!("packaging failed: {e}"));
            return;
        }
    };

    let config = {
        let mut cfg = GlobalConfig::load();
        if let Some(local) = GlobalConfig::load_local(&project_dir) {
            cfg.apply_local(local);
        }
        cfg
    };
    let registry: FreightRegistry = if let Some(rname) = repo {
        match config.registries.iter().find(|c| c.name == rname) {
            Some(c) => FreightRegistry::from_config(c),
            None => {
                print_error(&format!("registry `{rname}` not found in config"));
                return;
            }
        }
    } else {
        match config.registries.first() {
            Some(c) => FreightRegistry::from_config(c),
            None => FreightRegistry::default_registry(),
        }
    };

    let channel: Option<&str> = None;
    match registry.upload_prebuilt(name, version, channel, &triple, &tarball) {
        Ok(()) => print_success(&format!(
            "published prebuilt `{name}@{version}` for {triple}"
        )),
        Err(e) => print_error(&format!("upload failed: {e}")),
    }
}

fn build_prebuilt_tarball(
    project_dir: &std::path::Path,
    manifest: &Manifest,
    _triple: &str,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let name = &manifest.package.name;
    let version = &manifest.package.version;
    let desc = &manifest.package.description;

    let enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut ar = tar::Builder::new(enc);

    let include_dir = project_dir.join("include");
    if include_dir.is_dir() {
        ar.append_dir_all("include", &include_dir)?;
    }

    let release_dir = project_dir.join("target").join("release");
    for ext in &["a", "so", "dll", "dylib", "lib"] {
        for stem in &[format!("lib{name}"), name.clone()] {
            let candidate = release_dir.join(format!("{stem}.{ext}"));
            if candidate.is_file() {
                let dest = format!("lib/{stem}.{ext}");
                ar.append_path_with_name(&candidate, &dest)?;
            }
        }
    }

    let pc = format!(
        "prefix=/usr/local\n\
         libdir=${{prefix}}/lib\n\
         includedir=${{prefix}}/include\n\
         \n\
         Name: {name}\n\
         Description: {desc}\n\
         Version: {version}\n\
         Cflags: -I${{includedir}}\n\
         Libs: -L${{libdir}} -l{name}\n",
    );
    let pc_bytes = pc.as_bytes();
    let pc_path = format!("lib/pkgconfig/{name}.pc");
    let mut header = tar::Header::new_gnu();
    header.set_size(pc_bytes.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    ar.append_data(&mut header, &pc_path, pc_bytes)?;

    let gz = ar.into_inner()?.finish()?;
    Ok(gz)
}
