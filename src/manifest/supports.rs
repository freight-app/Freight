use super::types::{host_platforms, Manifest};

impl Manifest {
    /// Evaluate the optional `[package].supports` expression against this build.
    ///
    /// The syntax mirrors boolean platform expressions:
    /// identifiers, `!`, `&`, `|`, and parentheses. Missing `supports` means true.
    pub fn supports_current_platform(&self) -> Result<bool, String> {
        let Some(expr) = self.package.supports.as_deref() else {
            return Ok(true);
        };
        SupportsExpr::parse(expr)?.eval(&SupportEnv::from_manifest(self))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SupportsExpr {
    Ident(String),
    Not(Box<SupportsExpr>),
    And(Box<SupportsExpr>, Box<SupportsExpr>),
    Or(Box<SupportsExpr>, Box<SupportsExpr>),
}

impl SupportsExpr {
    fn parse(src: &str) -> Result<Self, String> {
        let mut parser = Parser::new(src);
        let expr = parser.parse_or()?;
        parser.skip_ws();
        if !parser.is_eof() {
            return Err(format!(
                "unexpected token {:?} at byte {}",
                parser.peek_char().unwrap(),
                parser.pos
            ));
        }
        Ok(expr)
    }

    fn eval(&self, env: &SupportEnv) -> Result<bool, String> {
        match self {
            SupportsExpr::Ident(name) => env.matches(name),
            SupportsExpr::Not(inner) => Ok(!inner.eval(env)?),
            SupportsExpr::And(lhs, rhs) => Ok(lhs.eval(env)? && rhs.eval(env)?),
            SupportsExpr::Or(lhs, rhs) => Ok(lhs.eval(env)? || rhs.eval(env)?),
        }
    }
}

#[derive(Debug)]
struct Parser<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str) -> Self {
        Self { src, pos: 0 }
    }

    fn parse_or(&mut self) -> Result<SupportsExpr, String> {
        let mut expr = self.parse_and()?;
        loop {
            self.skip_ws();
            if !self.eat('|') {
                break;
            }
            let rhs = self.parse_and()?;
            expr = SupportsExpr::Or(Box::new(expr), Box::new(rhs));
        }
        Ok(expr)
    }

    fn parse_and(&mut self) -> Result<SupportsExpr, String> {
        let mut expr = self.parse_unary()?;
        loop {
            self.skip_ws();
            if !self.eat('&') {
                break;
            }
            let rhs = self.parse_unary()?;
            expr = SupportsExpr::And(Box::new(expr), Box::new(rhs));
        }
        Ok(expr)
    }

    fn parse_unary(&mut self) -> Result<SupportsExpr, String> {
        self.skip_ws();
        if self.eat('!') {
            return Ok(SupportsExpr::Not(Box::new(self.parse_unary()?)));
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<SupportsExpr, String> {
        self.skip_ws();
        if self.eat('(') {
            let expr = self.parse_or()?;
            self.skip_ws();
            if !self.eat(')') {
                return Err(format!("expected ')' at byte {}", self.pos));
            }
            return Ok(expr);
        }
        self.parse_ident()
    }

    fn parse_ident(&mut self) -> Result<SupportsExpr, String> {
        self.skip_ws();
        let start = self.pos;
        while let Some(c) = self.peek_char() {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                self.pos += c.len_utf8();
            } else {
                break;
            }
        }
        if self.pos == start {
            return match self.peek_char() {
                Some(c) => Err(format!(
                    "expected identifier at byte {}, found {:?}",
                    self.pos, c
                )),
                None => Err(format!("expected identifier at byte {}", self.pos)),
            };
        }
        Ok(SupportsExpr::Ident(
            self.src[start..self.pos].to_ascii_lowercase(),
        ))
    }

    fn skip_ws(&mut self) {
        while let Some(c) = self.peek_char() {
            if c.is_whitespace() {
                self.pos += c.len_utf8();
            } else {
                break;
            }
        }
    }

    fn eat(&mut self, expected: char) -> bool {
        if self.peek_char() == Some(expected) {
            self.pos += expected.len_utf8();
            true
        } else {
            false
        }
    }

    fn peek_char(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }
    fn is_eof(&self) -> bool {
        self.pos >= self.src.len()
    }
}

#[derive(Debug, Clone)]
struct SupportEnv {
    os: String,
    arch: String,
    target: Option<String>,
}

impl SupportEnv {
    fn from_manifest(manifest: &Manifest) -> Self {
        Self {
            os: std::env::consts::OS.to_ascii_lowercase(),
            arch: manifest
                .target
                .arch
                .as_deref()
                .unwrap_or(std::env::consts::ARCH)
                .to_ascii_lowercase(),
            target: manifest
                .compiler
                .target
                .as_deref()
                .map(str::to_ascii_lowercase),
        }
    }

    fn matches(&self, raw_name: &str) -> Result<bool, String> {
        let name = raw_name.to_ascii_lowercase();
        let result = match name.as_str() {
            "windows" => self.matches_os("windows", &["windows"]),
            "mingw" => self.is_mingw(),
            "linux" => self.matches_os("linux", &["linux"]),
            "osx" | "macos" => self.matches_os("macos", &["darwin", "apple"]),
            "android" => self.matches_os("android", &["android"]),
            "ios" => self.matches_os("ios", &["ios"]),
            "uwp" => self.target_contains("uwp") || self.target_contains("windowsapp"),
            "unix" => self.matches_unix(),
            "bsd" | "freebsd" | "openbsd" | "netbsd" | "dragonfly" | "solaris" | "illumos" => {
                self.matches_os(name.as_str(), &[name.as_str()])
            }
            "x86" => {
                self.arch == "x86"
                    || self.arch == "i386"
                    || self.arch == "i686"
                    || self.target_arch_is(&["i386", "i486", "i586", "i686", "x86"])
            }
            "x64" | "x86_64" | "amd64" => {
                self.arch == "x86_64"
                    || self.arch == "x64"
                    || self.arch == "amd64"
                    || self.target_arch_is(&["x86_64", "amd64"])
            }
            "arm" => self.arch == "arm" || self.target_arch_is(&["arm", "armv7", "armv6"]),
            "arm64" | "aarch64" => {
                self.arch == "aarch64"
                    || self.arch == "arm64"
                    || self.target_arch_is(&["aarch64", "arm64"])
            }
            "wasm32" | "wasm64" | "mips" | "mips64" | "powerpc" | "powerpc64" | "riscv64"
            | "s390x" | "sparc64" => self.arch == name || self.target_arch_is(&[name.as_str()]),
            _ => return Err(format!("unknown supports identifier {:?}", raw_name)),
        };
        Ok(result)
    }

    fn is_mingw(&self) -> bool {
        if let Some(target) = &self.target {
            target.contains("mingw") || target.contains("windows-gnu")
        } else {
            // Native build: MSYSTEM is set by all MSYS2 subsystems (MINGW64, UCRT64, CLANG64 …)
            self.os == "windows"
                && std::env::var("MSYSTEM")
                    .map(|m| {
                        m.starts_with("MINGW")
                            || m.starts_with("UCRT")
                            || m.starts_with("CLANG")
                    })
                    .unwrap_or(false)
        }
    }

    fn matches_os(&self, host_os: &str, target_needles: &[&str]) -> bool {
        if self.target.is_some() {
            target_needles
                .iter()
                .any(|needle| self.target_contains(needle))
        } else {
            self.os == host_os
        }
    }

    fn matches_unix(&self) -> bool {
        if self.target.is_some() {
            [
                "linux",
                "darwin",
                "apple",
                "freebsd",
                "openbsd",
                "netbsd",
                "dragonfly",
                "android",
                "ios",
                "solaris",
                "illumos",
            ]
            .iter()
            .any(|needle| self.target_contains(needle))
        } else {
            host_platforms().iter().any(|p| *p == "unix")
        }
    }

    fn target_contains(&self, needle: &str) -> bool {
        self.target
            .as_deref()
            .is_some_and(|target| target.contains(needle))
    }

    fn target_arch_is(&self, needles: &[&str]) -> bool {
        self.target
            .as_deref()
            .and_then(|target| target.split('-').next())
            .is_some_and(|arch| {
                needles
                    .iter()
                    .any(|needle| arch.eq_ignore_ascii_case(needle))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(os: &str, arch: &str, target: Option<&str>) -> SupportEnv {
        SupportEnv {
            os: os.into(),
            arch: arch.into(),
            target: target.map(str::to_string),
        }
    }

    fn eval(expr: &str, env: &SupportEnv) -> Result<bool, String> {
        SupportsExpr::parse(expr)?.eval(env)
    }

    #[test]
    fn supports_expr_handles_nested_boolean_expression() {
        let expr = "(windows & !uwp & (x86 | x64)) | (!windows & !osx)";
        assert!(eval(
            expr,
            &env("windows", "x86_64", Some("x86_64-pc-windows-msvc"))
        )
        .unwrap());
        assert!(!eval(
            expr,
            &env("windows", "x86_64", Some("x86_64-uwp-windows-msvc"))
        )
        .unwrap());
        assert!(!eval(expr, &env("macos", "aarch64", None)).unwrap());
        assert!(eval(expr, &env("linux", "x86_64", None)).unwrap());
    }

    #[test]
    fn supports_expr_obeys_operator_precedence() {
        let e = env("linux", "x86_64", None);
        assert!(eval("windows | linux & x64", &e).unwrap());
        assert!(!eval("(windows | linux) & x86", &e).unwrap());
    }

    #[test]
    fn supports_expr_uses_target_os_when_cross_compiling() {
        let e = env("macos", "x86_64", Some("x86_64-unknown-linux-gnu"));
        assert!(eval("linux & unix", &e).unwrap());
        assert!(!eval("osx", &e).unwrap());
    }

    #[test]
    fn supports_expr_reports_syntax_and_unknown_identifiers() {
        assert!(SupportsExpr::parse("windows &").is_err());
        assert!(eval("plan9", &env("linux", "x86_64", None)).is_err());
    }

    #[test]
    fn supports_expr_distinguishes_mingw_from_msvc() {
        let msvc = env("windows", "x86_64", Some("x86_64-pc-windows-msvc"));
        let gnu  = env("windows", "x86_64", Some("x86_64-pc-windows-gnu"));
        let mingw = env("windows", "x86_64", Some("x86_64-w64-mingw32"));
        assert!(!eval("mingw", &msvc).unwrap());
        assert!(eval("mingw", &gnu).unwrap());
        assert!(eval("mingw", &mingw).unwrap());
        // windows is true for all three
        assert!(eval("windows", &msvc).unwrap());
        assert!(eval("windows", &gnu).unwrap());
    }
}
