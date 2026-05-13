//! System package manager detection and install-hint generation.
//!
//! Freight does not auto-install via system package managers (that requires
//! elevated privileges). This module detects which PM is available and
//! generates helpful "try: sudo apt install libfoo-dev" hints that are surfaced
//! as `BuildEvent::Warning` messages when all other resolvers fail.

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
            Self::Apt    => "apt",
            Self::Dnf    => "dnf",
            Self::Pacman => "pacman",
            Self::Zypper => "zypper",
            Self::Brew   => "brew",
            Self::Winget => "winget",
        }
    }

    /// Return a suggested install command for the given package name.
    pub fn install_hint(self, package: &str) -> String {
        match self {
            Self::Apt    => format!("sudo apt install lib{package}-dev"),
            Self::Dnf    => format!("sudo dnf install {package}-devel"),
            Self::Pacman => format!("sudo pacman -S {package}"),
            Self::Zypper => format!("sudo zypper install {package}-devel"),
            Self::Brew   => format!("brew install {package}"),
            Self::Winget => format!("winget install {package}"),
        }
    }
}

/// Detect the first available system package manager on `PATH`.
pub fn detect() -> Option<SystemPm> {
    for (pm, binary) in [
        (SystemPm::Apt,    "apt-get"),
        (SystemPm::Dnf,    "dnf"),
        (SystemPm::Pacman, "pacman"),
        (SystemPm::Zypper, "zypper"),
        (SystemPm::Brew,   "brew"),
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
