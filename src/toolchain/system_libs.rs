//! Built-in system-library stubs.
//!
//! Each stub describes a well-known OS library (pthread, ws2_32, …). Freight
//! uses these as the final step in `resolve_version_dep` when pkg-config fails.

use crate::supports::eval_supports;

// ── Public types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SystemLibStub {
    /// Package name (matches the dep key in `freight.toml`).
    pub name: String,
    /// Linker flag: `-l<link_name>`.
    pub link_name: String,
    /// Header filenames (display / TUI only).
    pub headers: Vec<String>,
}

// ── Hardcoded stubs ───────────────────────────────────────────────────────────

struct RawStub {
    name:     &'static str,
    link:     &'static str,
    supports: &'static str,
    hdrs:     &'static [&'static str],
}

const STUBS: &[RawStub] = &[
    // ── Cross-platform ────────────────────────────────────────────────────────
    RawStub {
        name: "m", link: "m",
        supports: "linux | freebsd | openbsd | netbsd | dragonfly | solaris | illumos | android",
        hdrs: &["math.h", "complex.h", "tgmath.h", "fenv.h"],
    },

    // ── Unix / POSIX ──────────────────────────────────────────────────────────
    RawStub {
        name: "pthread", link: "pthread",
        supports: "unix",
        hdrs: &["pthread.h", "semaphore.h", "sched.h"],
    },
    RawStub {
        name: "dl", link: "dl",
        supports: "linux | android | freebsd | netbsd | openbsd | dragonfly | solaris | illumos",
        hdrs: &["dlfcn.h"],
    },
    RawStub {
        name: "rt", link: "rt",
        supports: "linux | android | solaris | illumos",
        hdrs: &["time.h", "mqueue.h", "aio.h"],
    },
    RawStub {
        name: "resolv", link: "resolv",
        supports: "linux | freebsd | openbsd | netbsd | dragonfly | solaris | illumos",
        hdrs: &["resolv.h", "arpa/nameser.h"],
    },
    RawStub {
        name: "execinfo", link: "execinfo",
        supports: "freebsd | openbsd | netbsd | dragonfly",
        hdrs: &["execinfo.h"],
    },

    // ── Windows ───────────────────────────────────────────────────────────────
    RawStub {
        name: "kernel32", link: "kernel32",
        supports: "windows",
        hdrs: &["windows.h"],
    },
    RawStub {
        name: "user32", link: "user32",
        supports: "windows",
        hdrs: &["windows.h", "winuser.h"],
    },
    RawStub {
        name: "gdi32", link: "gdi32",
        supports: "windows",
        hdrs: &["windows.h", "wingdi.h"],
    },
    RawStub {
        name: "shell32", link: "shell32",
        supports: "windows",
        hdrs: &["shlobj.h", "shellapi.h"],
    },
    RawStub {
        name: "ole32", link: "ole32",
        supports: "windows",
        hdrs: &["objbase.h", "combaseapi.h"],
    },
    RawStub {
        name: "oleaut32", link: "oleaut32",
        supports: "windows",
        hdrs: &["oaidl.h", "oleauto.h"],
    },
    RawStub {
        name: "advapi32", link: "advapi32",
        supports: "windows",
        hdrs: &["windows.h", "wincrypt.h", "aclapi.h"],
    },
    RawStub {
        name: "ws2_32", link: "ws2_32",
        supports: "windows",
        hdrs: &["winsock2.h", "ws2tcpip.h", "mswsock.h"],
    },
    RawStub {
        name: "iphlpapi", link: "iphlpapi",
        supports: "windows",
        hdrs: &["iphlpapi.h", "iptypes.h"],
    },
    RawStub {
        name: "ntdll", link: "ntdll",
        supports: "windows",
        hdrs: &["winternl.h"],
    },
    RawStub {
        name: "dbghelp", link: "dbghelp",
        supports: "windows",
        hdrs: &["dbghelp.h"],
    },
    RawStub {
        name: "psapi", link: "psapi",
        supports: "windows",
        hdrs: &["psapi.h"],
    },
    RawStub {
        name: "winmm", link: "winmm",
        supports: "windows",
        hdrs: &["mmsystem.h", "timeapi.h"],
    },
    RawStub {
        name: "setupapi", link: "setupapi",
        supports: "windows",
        hdrs: &["setupapi.h", "devguid.h"],
    },
    RawStub {
        name: "comctl32", link: "comctl32",
        supports: "windows",
        hdrs: &["commctrl.h"],
    },
    RawStub {
        name: "comdlg32", link: "comdlg32",
        supports: "windows",
        hdrs: &["commdlg.h"],
    },
    RawStub {
        name: "bcrypt", link: "bcrypt",
        supports: "windows",
        hdrs: &["bcrypt.h"],
    },
    RawStub {
        name: "uuid", link: "uuid",
        supports: "windows",
        hdrs: &["guiddef.h", "basetyps.h"],
    },
    // ── Direct3D / DXGI ───────────────────────────────────────────────────────
    RawStub {
        name: "d3d11", link: "d3d11",
        supports: "windows",
        hdrs: &["d3d11.h", "d3dcommon.h"],
    },
    RawStub {
        name: "d3d12", link: "d3d12",
        supports: "windows",
        hdrs: &["d3d12.h", "d3d12sdklayers.h"],
    },
    RawStub {
        name: "dxgi", link: "dxgi",
        supports: "windows",
        hdrs: &["dxgi.h", "dxgi1_2.h", "dxgi1_6.h"],
    },
];

// ── Public API ────────────────────────────────────────────────────────────────

/// Return all built-in system-lib stubs that match the current host platform.
pub fn load_system_lib_stubs() -> Vec<SystemLibStub> {
    STUBS.iter()
        .filter(|s| eval_supports(s.supports))
        .map(|s| SystemLibStub {
            name:      s.name.to_string(),
            link_name: s.link.to_string(),
            headers:   s.hdrs.iter().map(|h| h.to_string()).collect(),
        })
        .collect()
}

/// Find the stub for `name` from a pre-loaded slice (case-insensitive).
pub fn find_stub<'a>(name: &str, stubs: &'a [SystemLibStub]) -> Option<&'a SystemLibStub> {
    stubs.iter().find(|s| s.name.eq_ignore_ascii_case(name))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pthread_loaded_on_unix() {
        if cfg!(unix) {
            let stubs = load_system_lib_stubs();
            let s = find_stub("pthread", &stubs).expect("pthread should be present on unix");
            assert_eq!(s.link_name, "pthread");
            assert!(s.headers.contains(&"pthread.h".to_string()));
        }
    }

    #[test]
    fn windows_stubs_not_loaded_on_unix() {
        if cfg!(unix) {
            let stubs = load_system_lib_stubs();
            assert!(find_stub("ws2_32", &stubs).is_none());
            assert!(find_stub("kernel32", &stubs).is_none());
        }
    }

    #[test]
    fn find_stub_case_insensitive() {
        let stubs = load_system_lib_stubs();
        if let Some(s) = stubs.first() {
            assert!(find_stub(&s.name.to_uppercase(), &stubs).is_some());
        }
    }
}
