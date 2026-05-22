//! Target triple classification — arch, OS, and compiler family tokens.
//!
//! Tokens and their aliases are defined as static tables below.
//! Adding a new target means adding a row here.

// (token, canonical) pairs — all lowercase
static ARCH_TOKENS: &[(&str, &str)] = &[
    ("x86_64",      "x86_64"),
    ("amd64",       "x86_64"),
    ("x86-64",      "x86_64"),
    ("aarch64",     "aarch64"),
    ("arm64",       "aarch64"),
    ("arm",         "arm"),
    ("armv7",       "arm"),
    ("armv6",       "arm"),
    ("armhf",       "arm"),
    ("armel",       "arm"),
    ("i686",        "i686"),
    ("i386",        "i686"),
    ("i586",        "i686"),
    ("mips",        "mips"),
    ("mipsel",      "mips"),
    ("mips64",      "mips64"),
    ("mips64el",    "mips64"),
    ("powerpc",     "powerpc"),
    ("ppc",         "powerpc"),
    ("powerpc64",   "powerpc64"),
    ("ppc64",       "powerpc64"),
    ("ppc64le",     "powerpc64"),
    ("riscv32",     "riscv32"),
    ("riscv64",     "riscv64"),
    ("riscv64gc",   "riscv64"),
    ("s390x",       "s390x"),
    ("sparc64",     "sparc64"),
    ("sparc",       "sparc64"),
    ("loongarch64", "loongarch64"),
    ("wasm32",      "wasm32"),
];

static OS_TOKENS: &[(&str, &str)] = &[
    ("linux",     "linux"),
    ("windows",   "windows"),
    ("macos",     "macos"),
    ("darwin",    "macos"),
    ("freebsd",   "freebsd"),
    ("openbsd",   "openbsd"),
    ("netbsd",    "netbsd"),
    ("dragonfly", "dragonfly"),
    ("android",   "android"),
    ("ios",       "ios"),
    ("solaris",   "solaris"),
    ("illumos",   "illumos"),
    ("fuchsia",   "fuchsia"),
    ("none",      "none"),
];

// ── Triple parsing ────────────────────────────────────────────────────────────

/// Derive `(arch, os)` from a partial or full target specifier.
///
/// Freight accepts any subset of `arch-os-compiler_family` — missing
/// components default to the host value:
///
/// | Input               | arch    | os      |
/// |---------------------|---------|---------|
/// | `aarch64-linux-gnu` | aarch64 | linux   |
/// | `aarch64`           | aarch64 | *host*  |
/// | `linux`             | *host*  | linux   |
/// | `linux-gnu`         | *host*  | linux   |
/// | `x86_64-windows`    | x86_64  | windows |
///
/// Standard 4-part GNU triples (`x86_64-unknown-linux-gnu`,
/// `x86_64-pc-windows-msvc`, `x86_64-apple-darwin`) are also handled —
/// unrecognised vendor tokens (`unknown`, `pc`, `apple`) are skipped.
pub fn parse_triple(triple: &str) -> (String, String) {
    let mut arch_out: Option<&str> = None;
    let mut os_out:   Option<&str> = None;

    for part in triple.split('-') {
        let lower = part.to_lowercase();
        if arch_out.is_none() {
            if let Some(&(_, canonical)) = ARCH_TOKENS.iter().find(|&&(t, _)| t == lower) {
                arch_out = Some(canonical);
                continue;
            }
        }
        if os_out.is_none() {
            if let Some(&(_, canonical)) = OS_TOKENS.iter().find(|&&(t, _)| t == lower) {
                os_out = Some(canonical);
            }
        }
    }

    let arch = arch_out.map(str::to_string)
        .unwrap_or_else(|| std::env::consts::ARCH.to_string());
    let os   = os_out.map(str::to_string)
        .unwrap_or_else(|| std::env::consts::OS.to_string());
    (arch, os)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::parse_triple;

    #[test]
    fn full_triple() {
        assert_eq!(parse_triple("aarch64-linux-gnu"),   ("aarch64".into(), "linux".into()));
        assert_eq!(parse_triple("x86_64-windows-msvc"), ("x86_64".into(),  "windows".into()));
        assert_eq!(parse_triple("x86_64-macos-clang"),  ("x86_64".into(),  "macos".into()));
    }

    #[test]
    fn gnu_4part_triples() {
        assert_eq!(parse_triple("x86_64-unknown-linux-gnu"),  ("x86_64".into(),  "linux".into()));
        assert_eq!(parse_triple("x86_64-pc-windows-msvc"),    ("x86_64".into(),  "windows".into()));
        assert_eq!(parse_triple("x86_64-apple-darwin"),       ("x86_64".into(),  "macos".into()));
        assert_eq!(parse_triple("aarch64-unknown-linux-gnu"), ("aarch64".into(), "linux".into()));
    }

    #[test]
    fn arch_only_falls_back_to_host_os() {
        let (arch, os) = parse_triple("aarch64");
        assert_eq!(arch, "aarch64");
        assert_eq!(os, std::env::consts::OS);
    }

    #[test]
    fn os_only_falls_back_to_host_arch() {
        let (arch, os) = parse_triple("linux");
        assert_eq!(arch, std::env::consts::ARCH);
        assert_eq!(os, "linux");
    }

    #[test]
    fn compiler_only_falls_back_to_host_arch_and_os() {
        let (arch, os) = parse_triple("gnu");
        assert_eq!(arch, std::env::consts::ARCH);
        assert_eq!(os, std::env::consts::OS);
    }

    #[test]
    fn os_compiler_falls_back_to_host_arch() {
        let (arch, os) = parse_triple("linux-gnu");
        assert_eq!(arch, std::env::consts::ARCH);
        assert_eq!(os, "linux");
    }

    #[test]
    fn normalises_aliases() {
        assert_eq!(parse_triple("amd64-linux-gnu"),   ("x86_64".into(),  "linux".into()));
        assert_eq!(parse_triple("arm64-linux-gnu"),   ("aarch64".into(), "linux".into()));
        assert_eq!(parse_triple("x86_64-darwin-gnu"), ("x86_64".into(),  "macos".into()));
    }
}
