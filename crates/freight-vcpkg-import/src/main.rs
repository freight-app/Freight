use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use clap::Parser;
use flate2::write::GzEncoder;
use flate2::Compression;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

#[derive(Parser)]
#[command(
    name = "freight-vcpkg-import",
    about = "Install vcpkg packages and publish them to a freight registry"
)]
struct Args {
    /// Packages to install — e.g. "zlib" or "zlib:x64-linux"
    #[arg(required = true)]
    packages: Vec<String>,

    /// Freight registry base URL
    #[arg(long, default_value = "http://localhost:7878")]
    registry: String,

    /// Auth token (publish scope required)
    #[arg(long, env = "FREIGHT_TOKEN")]
    token: String,

    /// vcpkg triplet (auto-detected from host if omitted)
    #[arg(long)]
    triplet: Option<String>,

    /// Registry channel
    #[arg(long, default_value = "stable")]
    channel: String,

    /// Working directory for vcpkg installation (temp dir if omitted)
    #[arg(long)]
    workdir: Option<PathBuf>,

    /// Path to vcpkg binary
    #[arg(long, default_value = "vcpkg", env = "VCPKG")]
    vcpkg: String,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let host_triplet = args.triplet.unwrap_or_else(default_triplet);
    eprintln!("host triplet: {host_triplet}");

    let _tmp;
    let workdir: &Path = match &args.workdir {
        Some(d) => {
            fs::create_dir_all(d)?;
            d.as_path()
        }
        None => {
            _tmp = tempfile::tempdir()?;
            _tmp.path()
        }
    };

    let install_root = workdir.join("vcpkg_installed");
    let client = reqwest::blocking::Client::new();

    for pkg_spec in &args.packages {
        let (pkg_name, pkg_triplet) = split_spec(pkg_spec, &host_triplet);
        eprintln!("\n==> {pkg_name}:{pkg_triplet}");

        vcpkg_install(&args.vcpkg, &pkg_name, &pkg_triplet, &install_root)?;

        let triplet_dir = install_root.join(&pkg_triplet);
        let version = read_vcpkg_version(&triplet_dir, &pkg_name);
        let description = read_vcpkg_description(&triplet_dir, &pkg_name);
        eprintln!("    version: {version}");

        let src_tb = build_source_tarball(&triplet_dir, &pkg_name, &version, description.as_deref())?;
        eprintln!("    source tarball: {} bytes", src_tb.len());
        publish_source(&client, &args.registry, &args.token, &args.channel, &pkg_name, &version, src_tb)
            .with_context(|| format!("publishing source for {pkg_name} {version}"))?;
        eprintln!("    source published");

        let pre_tb = build_prebuilt_tarball(&triplet_dir)?;
        eprintln!("    prebuilt tarball: {} bytes", pre_tb.len());
        publish_prebuilt(&client, &args.registry, &args.token, &args.channel, &pkg_name, &version, &pkg_triplet, pre_tb)
            .with_context(|| format!("publishing prebuilt for {pkg_name} {version} {pkg_triplet}"))?;
        eprintln!("    prebuilt published");
    }

    eprintln!("\nAll done.");
    Ok(())
}

fn split_spec<'a>(spec: &'a str, fallback: &str) -> (String, String) {
    match spec.rsplit_once(':') {
        Some((name, triplet)) if !triplet.trim().is_empty() => {
            (name.to_string(), triplet.to_string())
        }
        _ => (spec.to_string(), fallback.to_string()),
    }
}

fn vcpkg_install(vcpkg_bin: &str, package: &str, triplet: &str, install_root: &Path) -> Result<()> {
    let status = Command::new(vcpkg_bin)
        .arg("install")
        .arg(format!("{package}:{triplet}"))
        .arg("--x-install-root")
        .arg(install_root)
        .status()
        .with_context(|| format!("failed to run vcpkg (tried: {vcpkg_bin})"))?;

    if !status.success() {
        bail!("vcpkg install exited with status {:?}", status.code());
    }
    Ok(())
}

fn read_vcpkg_json(triplet_dir: &Path, pkg_name: &str) -> Option<Value> {
    let path = triplet_dir.join("share").join(pkg_name).join("vcpkg.json");
    let content = fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

fn read_vcpkg_version(triplet_dir: &Path, pkg_name: &str) -> String {
    if let Some(meta) = read_vcpkg_json(triplet_dir, pkg_name) {
        for key in &["version", "version-semver", "version-date", "version-string", "version-relaxed"] {
            if let Some(v) = meta[key].as_str() {
                // vcpkg version-date looks like "2024-01-01" — keep as-is
                return v.to_string();
            }
        }
    }
    "0.0.0".to_string()
}

fn read_vcpkg_description(triplet_dir: &Path, pkg_name: &str) -> Option<String> {
    let meta = read_vcpkg_json(triplet_dir, pkg_name)?;
    // description can be a string or an array of strings (first line = short desc)
    if let Some(s) = meta["description"].as_str() {
        return Some(s.to_string());
    }
    meta["description"].as_array()?.first()?.as_str().map(str::to_string)
}

fn build_source_tarball(
    triplet_dir: &Path,
    name: &str,
    version: &str,
    description: Option<&str>,
) -> Result<Vec<u8>> {
    let enc = GzEncoder::new(Vec::new(), Compression::best());
    let mut ar = tar::Builder::new(enc);

    // Minimal freight.toml
    let manifest = build_freight_toml(name, version, description);
    let mb = manifest.as_bytes();
    let mut hdr = tar::Header::new_gnu();
    hdr.set_path("freight.toml")?;
    hdr.set_size(mb.len() as u64);
    hdr.set_mode(0o644);
    hdr.set_cksum();
    ar.append(&hdr, mb)?;

    // Headers
    let include_dir = triplet_dir.join("include");
    if include_dir.is_dir() {
        ar.append_dir_all("include", &include_dir)?;
    }

    Ok(ar.into_inner()?.finish()?)
}

fn build_freight_toml(name: &str, version: &str, description: Option<&str>) -> String {
    let mut s = format!("[package]\nname = \"{name}\"\nversion = \"{version}\"\n");
    if let Some(d) = description {
        let escaped = d.replace('\\', "\\\\").replace('"', "\\\"");
        s.push_str(&format!("description = \"{escaped}\"\n"));
    }
    s
}

fn build_prebuilt_tarball(triplet_dir: &Path) -> Result<Vec<u8>> {
    let enc = GzEncoder::new(Vec::new(), Compression::best());
    let mut ar = tar::Builder::new(enc);

    let include_dir = triplet_dir.join("include");
    if include_dir.is_dir() {
        ar.append_dir_all("include", &include_dir)?;
    }

    let lib_dir = triplet_dir.join("lib");
    if lib_dir.is_dir() {
        ar.append_dir_all("lib", &lib_dir)?;
    }

    Ok(ar.into_inner()?.finish()?)
}

// freight wire format: [u32-LE JSON len][JSON bytes][u32-LE tarball len][tarball bytes]
fn publish_source(
    client: &reqwest::blocking::Client,
    registry: &str,
    token: &str,
    channel: &str,
    name: &str,
    version: &str,
    tarball: Vec<u8>,
) -> Result<()> {
    let meta = json!({ "name": name, "vers": version, "channel": channel }).to_string();
    let mb = meta.as_bytes();
    let mut body = Vec::with_capacity(8 + mb.len() + tarball.len());
    body.extend_from_slice(&(mb.len() as u32).to_le_bytes());
    body.extend_from_slice(mb);
    body.extend_from_slice(&(tarball.len() as u32).to_le_bytes());
    body.extend_from_slice(&tarball);

    let url = format!("{}/api/v1/packages", registry.trim_end_matches('/'));
    let resp = client.put(&url).bearer_auth(token).body(body).send()
        .with_context(|| format!("PUT {url}"))?;

    check_resp(resp)
}

fn publish_prebuilt(
    client: &reqwest::blocking::Client,
    registry: &str,
    token: &str,
    channel: &str,
    name: &str,
    version: &str,
    triplet: &str,
    tarball: Vec<u8>,
) -> Result<()> {
    let enc_name = name.replace('/', "%2F");
    let checksum = hex::encode(Sha256::digest(&tarball));
    let url = format!(
        "{}/api/v1/packages/{enc_name}/{version}/prebuilt/{triplet}?channel={channel}",
        registry.trim_end_matches('/')
    );

    let resp = client
        .put(&url)
        .bearer_auth(token)
        .header("Content-Type", "application/octet-stream")
        .header("X-Checksum-Sha256", &checksum)
        .body(tarball)
        .send()
        .with_context(|| format!("PUT {url}"))?;

    check_resp(resp)
}

fn check_resp(resp: reqwest::blocking::Response) -> Result<()> {
    let status = resp.status();
    let body: Value = resp.json().unwrap_or_default();
    if !status.is_success() {
        let detail = body["errors"][0]["detail"].as_str().unwrap_or("request failed");
        bail!("{status} — {detail}");
    }
    Ok(())
}

fn default_triplet() -> String {
    if let Ok(t) = std::env::var("VCPKG_DEFAULT_TRIPLET") {
        let t = t.trim().to_string();
        if !t.is_empty() {
            return t;
        }
    }
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64")  => "x64-windows".into(),
        ("windows", "x86")     => "x86-windows".into(),
        ("windows", "aarch64") => "arm64-windows".into(),
        ("macos",   "aarch64") => "arm64-osx".into(),
        ("macos",   _)         => "x64-osx".into(),
        ("linux",   "aarch64") => "arm64-linux".into(),
        ("linux",   _)         => "x64-linux".into(),
        (_,         "aarch64") => "arm64-linux".into(),
        _                      => "x64-linux".into(),
    }
}
