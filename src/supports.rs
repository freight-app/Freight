//! Boolean platform expression evaluator for `supports = "..."` fields.
//!
//! Syntax: identifiers joined by `!` (not), `&` (and), `|` (or), and
//! parentheses. Operator precedence: `!` > `&` > `|`.
//!
//! Recognised identifiers (case-insensitive):
//!
//! OS:   linux, windows, mingw, uwp, macos, osx, freebsd, openbsd, netbsd, dragonfly,
//!       android, ios, solaris, illumos, unix (family), bsd (family)
//! Arch: x86, x64, x86_64, arm, arm64, aarch64, wasm32, riscv64, powerpc64
//!
//! Unknown identifiers evaluate to `false` (the platform tag is simply absent).

use std::collections::HashSet;

use anyhow::{Result, anyhow};

// ── Public entry point ────────────────────────────────────────────────────────

/// Evaluate a `supports` expression against the current host OS and arch.
/// Returns `true` when the expression holds, `false` on mismatch or parse error.
pub fn eval_supports(expr: &str) -> bool {
    let env = HostEnv::current();
    match SupportsExpr::parse(expr) {
        Ok(e) => e.eval(&env),
        Err(_) => false,
    }
}

// ── AST ───────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SupportsExpr {
    Ident(String),
    Not(Box<SupportsExpr>),
    And(Box<SupportsExpr>, Box<SupportsExpr>),
    Or(Box<SupportsExpr>, Box<SupportsExpr>),
}

impl SupportsExpr {
    pub fn parse(src: &str) -> Result<Self> {
        let mut p = Parser::new(src);
        let expr = p.parse_or()?;
        p.skip_ws();
        if !p.is_eof() {
            return Err(anyhow!(
                "unexpected token {:?} at byte {}",
                p.peek_char().unwrap(),
                p.pos
            ));
        }
        Ok(expr)
    }

    pub fn eval(&self, env: &HostEnv) -> bool {
        match self {
            Self::Ident(name) => env.has(name),
            Self::Not(inner) => !inner.eval(env),
            Self::And(l, r) => l.eval(env) && r.eval(env),
            Self::Or(l, r) => l.eval(env) || r.eval(env),
        }
    }
}

// ── Parser ────────────────────────────────────────────────────────────────────

struct Parser<'a> {
    src: &'a str,
    pub pos: usize,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str) -> Self {
        Self { src, pos: 0 }
    }

    fn parse_or(&mut self) -> Result<SupportsExpr> {
        let mut expr = self.parse_and()?;
        loop {
            self.skip_ws();
            if !self.eat('|') {
                break;
            }
            expr = SupportsExpr::Or(Box::new(expr), Box::new(self.parse_and()?));
        }
        Ok(expr)
    }

    fn parse_and(&mut self) -> Result<SupportsExpr> {
        let mut expr = self.parse_unary()?;
        loop {
            self.skip_ws();
            if !self.eat('&') {
                break;
            }
            expr = SupportsExpr::And(Box::new(expr), Box::new(self.parse_unary()?));
        }
        Ok(expr)
    }

    fn parse_unary(&mut self) -> Result<SupportsExpr> {
        self.skip_ws();
        if self.eat('!') {
            return Ok(SupportsExpr::Not(Box::new(self.parse_unary()?)));
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<SupportsExpr> {
        self.skip_ws();
        if self.eat('(') {
            let expr = self.parse_or()?;
            self.skip_ws();
            if !self.eat(')') {
                return Err(anyhow!("expected ')' at byte {}", self.pos));
            }
            return Ok(expr);
        }
        self.parse_ident()
    }

    fn parse_ident(&mut self) -> Result<SupportsExpr> {
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
            return Err(anyhow!("expected identifier at byte {}", self.pos));
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

    pub fn peek_char(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    pub fn is_eof(&self) -> bool {
        self.pos >= self.src.len()
    }
}

// ── Host environment ──────────────────────────────────────────────────────────

pub struct HostEnv {
    tags: HashSet<String>,
}

impl HostEnv {
    pub fn current() -> Self {
        let os = std::env::consts::OS;
        let arch = std::env::consts::ARCH;
        let mut tags = HashSet::new();

        // Primary OS tag.
        tags.insert(os.to_ascii_lowercase());

        // Aliases and family tags.
        match os {
            "macos" => {
                tags.insert("osx".into());
                tags.insert("unix".into());
            }
            "linux" | "android" | "ios" | "solaris" | "illumos" => {
                tags.insert("unix".into());
            }
            "freebsd" | "openbsd" | "netbsd" | "dragonfly" => {
                tags.insert("unix".into());
                tags.insert("bsd".into());
            }
            "windows" => {
                tags.insert("win32".into());
                // Detect MinGW/MSYS2 — MSYSTEM is set by all MSYS2 subsystems.
                let msystem = std::env::var("MSYSTEM").unwrap_or_default();
                if msystem.starts_with("MINGW")
                    || msystem.starts_with("UCRT")
                    || msystem.starts_with("CLANG")
                {
                    tags.insert("mingw".into());
                }
            }
            _ => {}
        }

        // Arch tags.
        tags.insert(arch.to_ascii_lowercase());
        match arch {
            "x86_64" => {
                tags.insert("x64".into());
            }
            "aarch64" => {
                tags.insert("arm64".into());
            }
            _ => {}
        }

        Self { tags }
    }

    pub fn has(&self, ident: &str) -> bool {
        self.tags.contains(&ident.to_ascii_lowercase())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn env_with(tags: &[&str]) -> HostEnv {
        HostEnv {
            tags: tags.iter().map(|s| s.to_ascii_lowercase()).collect(),
        }
    }

    #[test]
    fn simple_ident() {
        let env = env_with(&["linux", "x64", "unix"]);
        assert!(SupportsExpr::parse("linux").unwrap().eval(&env));
        assert!(!SupportsExpr::parse("windows").unwrap().eval(&env));
    }

    #[test]
    fn not_and_or() {
        let env = env_with(&["linux", "x64", "unix"]);
        assert!(SupportsExpr::parse("linux & !windows").unwrap().eval(&env));
        assert!(SupportsExpr::parse("windows | linux").unwrap().eval(&env));
        assert!(!SupportsExpr::parse("windows & linux").unwrap().eval(&env));
    }

    #[test]
    fn parentheses() {
        let env = env_with(&["linux", "x64", "unix"]);
        assert!(SupportsExpr::parse("(linux | windows) & !macos").unwrap().eval(&env));
    }

    #[test]
    fn unknown_ident_is_false() {
        let env = env_with(&["linux"]);
        assert!(!SupportsExpr::parse("unknown-platform").unwrap().eval(&env));
    }
}
