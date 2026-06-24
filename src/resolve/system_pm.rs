//! System package manager detection and install-hint generation.
//!
//! Freight does not auto-install via system package managers (that requires
//! elevated privileges). This module detects which PM is available and
//! generates helpful "try: sudo apt install libfoo-dev" hints that are surfaced
//! as `BuildEvent::Warning` messages when all other resolvers fail.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The detected system package manager, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemPm {
    Apt,
    Dnf,
    Pacman,
    Zypper,
    Brew,
    Winget,
}

impl SystemPm {
    pub fn name(self) -> &'static str {
        match self {
            Self::Apt => "apt",
            Self::Dnf => "dnf",
            Self::Pacman => "pacman",
            Self::Zypper => "zypper",
            Self::Brew => "brew",
            Self::Winget => "winget",
        }
    }

    /// Best-effort one-line description of the OS package that owns `pc_file`
    /// (a pkg-config `.pc` file). Used to fill the `description` of a generated
    /// system-registry stub. `None` when the PM can't be queried or the file is
    /// unowned (e.g. a hand-installed `.pc`).
    pub fn describe(self, pc_file: &Path) -> Option<String> {
        let file = pc_file.to_string_lossy();
        match self {
            Self::Apt => {
                let owner = parse_dpkg_owner(&run(&["dpkg", "-S", &file])?)?;
                let desc = run(&["dpkg-query", "-W", "-f=${Description}", &owner])?;
                Some(first_line(&desc))
            }
            Self::Dnf | Self::Zypper => {
                let owner = run(&["rpm", "-qf", "--qf", "%{NAME}", &file])?;
                let owner = owner.trim();
                if owner.is_empty() || owner.contains("not owned") {
                    return None;
                }
                let desc = run(&["rpm", "-q", "--qf", "%{SUMMARY}", owner])?;
                Some(desc.trim().to_string()).filter(|s| !s.is_empty())
            }
            Self::Pacman => {
                let owner = parse_pacman_owner(&run(&["pacman", "-Qo", &file])?)?;
                parse_pacman_description(&run(&["pacman", "-Qi", &owner])?)
            }
            // brew/winget don't track `.pc` file ownership; no reliable lookup.
            Self::Brew | Self::Winget => None,
        }
    }

    /// Return a suggested install command for the given package name.
    pub fn install_hint(self, package: &str) -> String {
        match self {
            Self::Apt => format!("sudo apt install lib{package}-dev"),
            Self::Dnf => format!("sudo dnf install {package}-devel"),
            Self::Pacman => format!("sudo pacman -S {package}"),
            Self::Zypper => format!("sudo zypper install {package}-devel"),
            Self::Brew => format!("brew install {package}"),
            Self::Winget => format!("winget install {package}"),
        }
    }
}

/// Detect the first available system package manager on `PATH`.
pub fn detect() -> Option<SystemPm> {
    for (pm, binary) in [
        (SystemPm::Apt, "apt-get"),
        (SystemPm::Dnf, "dnf"),
        (SystemPm::Pacman, "pacman"),
        (SystemPm::Zypper, "zypper"),
        (SystemPm::Brew, "brew"),
        (SystemPm::Winget, "winget"),
    ] {
        if is_on_path(binary) {
            return Some(pm);
        }
    }
    None
}

fn is_on_path(binary: &str) -> bool {
    Command::new(binary)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Locate a pkg-config package's `.pc` file (the artifact OS package managers
/// track ownership of). Uses pkg-config's auto-set `pcfiledir` variable.
pub fn pc_file_path(name: &str) -> Option<PathBuf> {
    let out = Command::new("pkg-config")
        .args(["--variable=pcfiledir", name])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if dir.is_empty() {
        return None;
    }
    let pc = Path::new(&dir).join(format!("{name}.pc"));
    pc.is_file().then_some(pc)
}

/// Run a command, returning trimmed stdout on success (`None` otherwise).
fn run(argv: &[&str]) -> Option<String> {
    let out = Command::new(argv[0]).args(&argv[1..]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").trim().to_string()
}

/// `dpkg -S /path/foo.pc` → `libfoo-dev: /path/foo.pc` → `libfoo-dev`.
fn parse_dpkg_owner(out: &str) -> Option<String> {
    let line = out.lines().next()?;
    let pkg = line.split(':').next()?.trim();
    // A multi-arch line can read "pkg:amd64: /path"; keep the package name only.
    let pkg = pkg.split_whitespace().next().unwrap_or(pkg);
    (!pkg.is_empty()).then(|| pkg.to_string())
}

/// `pacman -Qo /path/foo.pc` → `/path/foo.pc is owned by foo 1.2-3` → `foo`.
fn parse_pacman_owner(out: &str) -> Option<String> {
    let line = out.lines().next()?;
    let after = line.split("owned by").nth(1)?.trim();
    after.split_whitespace().next().map(str::to_string)
}

/// Extract the `Description : ...` field from `pacman -Qi` output.
fn parse_pacman_description(out: &str) -> Option<String> {
    out.lines()
        .find_map(|l| l.split_once(':').filter(|(k, _)| k.trim() == "Description"))
        .map(|(_, v)| v.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dpkg_owner_parsing() {
        assert_eq!(
            parse_dpkg_owner("zlib1g-dev:amd64: /usr/lib/x86_64-linux-gnu/pkgconfig/zlib.pc"),
            Some("zlib1g-dev".to_string())
        );
        assert_eq!(
            parse_dpkg_owner("libssl-dev: /usr/lib/pkgconfig/openssl.pc"),
            Some("libssl-dev".to_string())
        );
    }

    #[test]
    fn pacman_owner_and_description_parsing() {
        assert_eq!(
            parse_pacman_owner("/usr/lib/pkgconfig/zlib.pc is owned by zlib 1.3-2"),
            Some("zlib".to_string())
        );
        let qi = "Name            : zlib\nDescription     : Compression library\nArchitecture    : x86_64\n";
        assert_eq!(
            parse_pacman_description(qi),
            Some("Compression library".to_string())
        );
    }
}
