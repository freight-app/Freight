//! LaTeX math → single-line terminal text renderer.

use crate::markdown::protect_math;

// ── Public API ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Unicode,
    Ascii,
}

/// Render a math region (with or without surrounding `$`/`$$` delimiters).
pub fn render_math_block(raw: &str) -> String {
    render_math_block_with(raw, Mode::Unicode)
}

pub fn render_math_block_with(raw: &str, mode: Mode) -> String {
    render_one(strip_delimiters(raw), mode)
}

/// Render prose containing inline / display math regions.
///
/// Math regions are detected by [`protect_math`]; each is rendered in place
/// with [`render_math_block_with`]. Non-math text passes through unchanged.
pub fn render_math_lines(text: &str) -> String {
    render_math_lines_with(text, Mode::Unicode)
}

pub fn render_math_lines_with(text: &str, mode: Mode) -> String {
    let (protected, regions) = protect_math(text);
    if regions.is_empty() {
        return protected;
    }
    let mut s = protected;
    for r in &regions {
        let rendered = render_one(strip_delimiters(&r.raw), mode);
        s = s.replace(&r.placeholder, &rendered);
    }
    s
}

fn strip_delimiters(raw: &str) -> &str {
    let r = raw.trim();
    for (open, close) in [("$$", "$$"), ("\\[", "\\]"), ("\\(", "\\)")] {
        if r.len() >= 4 && r.starts_with(open) && r.ends_with(close) {
            return &r[2..r.len() - 2];
        }
    }
    if r.len() >= 2 && r.starts_with('$') && r.ends_with('$') {
        return &r[1..r.len() - 1];
    }
    r
}

fn render_one(src: &str, mode: Mode) -> String {
    let trimmed = src.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let toks = lex(trimmed);
    let mut p = Parser { toks: &toks, pos: 0 };
    let expr = parse_expr(&mut p, 0);
    let (s, _) = render_expr(&expr, mode);
    s
}

// ── Tokens ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    Cmd(String),
    Ident(String),
    Num(String),
    Op(String),
    LBrace, RBrace,
    LParen, RParen,
    LBracket, RBracket,
    Caret, Underscore,
    Comma,
    Amp,               // & (matrix cell sep)
    RowSep,            // \\ (matrix row sep)
    Prime,             // '
    Pipe,              // | (abs)
    DoubleVert,        // \| or \Vert (norm)
    LFloor, RFloor,
    LCeil, RCeil,
    LAngle, RAngle,
    Begin(String),     // \begin{env}
    End(String),       // \end{env}
    Text(String),      // \text{...} verbatim
}

fn lex(src: &str) -> Vec<Tok> {
    let chars: Vec<char> = src.chars().collect();
    let mut toks = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '\\' {
            i += 1;
            let start = i;
            while i < chars.len() && chars[i].is_alphabetic() {
                i += 1;
            }
            if i == start {
                if let Some(&p) = chars.get(i) {
                    i += 1;
                    match p {
                        ',' | ';' | '!' | ' ' | '\t' | '\n' => {}
                        '{' => toks.push(Tok::LBrace),
                        '}' => toks.push(Tok::RBrace),
                        '\\' => toks.push(Tok::RowSep),
                        '|' => toks.push(Tok::DoubleVert),
                        _ => toks.push(Tok::Cmd(p.to_string())),
                    }
                }
            } else {
                let name: String = chars[start..i].iter().collect();
                match name.as_str() {
                    "left" | "right" | "quad" | "qquad" | "displaystyle" | "textstyle" => {}
                    "begin" | "end" => {
                        while i < chars.len() && chars[i].is_whitespace() { i += 1; }
                        if chars.get(i) == Some(&'{') {
                            i += 1;
                            let env_start = i;
                            while i < chars.len() && chars[i] != '}' { i += 1; }
                            let env: String = chars[env_start..i].iter().collect();
                            if chars.get(i) == Some(&'}') { i += 1; }
                            if name == "begin" {
                                toks.push(Tok::Begin(env));
                            } else {
                                toks.push(Tok::End(env));
                            }
                        }
                    }
                    "text" | "mbox" => {
                        while i < chars.len() && chars[i].is_whitespace() { i += 1; }
                        if chars.get(i) == Some(&'{') {
                            i += 1;
                            let text_start = i;
                            let mut depth = 1;
                            while i < chars.len() && depth > 0 {
                                match chars[i] {
                                    '{' => { depth += 1; i += 1; }
                                    '}' => { depth -= 1; if depth == 0 { break; } i += 1; }
                                    _ => { i += 1; }
                                }
                            }
                            let text: String = chars[text_start..i].iter().collect();
                            if chars.get(i) == Some(&'}') { i += 1; }
                            toks.push(Tok::Text(text));
                        } else {
                            toks.push(Tok::Cmd(name));
                        }
                    }
                    "lfloor" => toks.push(Tok::LFloor),
                    "rfloor" => toks.push(Tok::RFloor),
                    "lceil"  => toks.push(Tok::LCeil),
                    "rceil"  => toks.push(Tok::RCeil),
                    "langle" => toks.push(Tok::LAngle),
                    "rangle" => toks.push(Tok::RAngle),
                    "Vert"   => toks.push(Tok::DoubleVert),
                    "vert" | "lvert" | "rvert" => toks.push(Tok::Pipe),
                    _ => toks.push(Tok::Cmd(name)),
                }
            }
            continue;
        }
        if c.is_ascii_digit() || (c == '.' && chars.get(i + 1).is_some_and(|n| n.is_ascii_digit())) {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            toks.push(Tok::Num(chars[start..i].iter().collect()));
            continue;
        }
        if c.is_alphabetic() {
            toks.push(Tok::Ident(c.to_string()));
            i += 1;
            continue;
        }
        i += 1;
        match c {
            '{' => toks.push(Tok::LBrace),
            '}' => toks.push(Tok::RBrace),
            '(' => toks.push(Tok::LParen),
            ')' => toks.push(Tok::RParen),
            '[' => toks.push(Tok::LBracket),
            ']' => toks.push(Tok::RBracket),
            '^' => toks.push(Tok::Caret),
            '_' => toks.push(Tok::Underscore),
            ',' => toks.push(Tok::Comma),
            '&' => toks.push(Tok::Amp),
            '|' => toks.push(Tok::Pipe),
            '\'' => toks.push(Tok::Prime),
            // Stray punctuation that may appear as math-atom content
            // (e.g. \overset{?}{=}, identifier suffixes).
            '?' | ';' | ':' => toks.push(Tok::Ident(c.to_string())),
            '+' | '-' | '*' | '/' | '=' => toks.push(Tok::Op(c.to_string())),
            '<' | '>' => {
                if chars.get(i) == Some(&'=') {
                    i += 1;
                    toks.push(Tok::Op(format!("{c}=")));
                } else {
                    toks.push(Tok::Op(c.to_string()));
                }
            }
            '!' if chars.get(i) == Some(&'=') => {
                i += 1;
                toks.push(Tok::Op("!=".into()));
            }
            '!' => toks.push(Tok::Ident("!".into())),
            _ => {}
        }
    }
    toks
}

// ── AST ───────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
enum Expr {
    Empty,
    Atom(String),
    Text(String),
    Bb(String),
    Group(Box<Expr>),
    Neg(Box<Expr>),
    Bin(BinOp, Box<Expr>, Box<Expr>),
    Pow(Box<Expr>, Box<Expr>),
    Sub(Box<Expr>, Box<Expr>),
    Primed(Box<Expr>, usize),
    Frac(Box<Expr>, Box<Expr>),
    Sqrt(Box<Expr>),
    BigOp(BigOpKind, Bounds, Box<Expr>, Option<String>),
    Call(String, Box<Expr>),
    Juxt(Vec<Expr>),
    Abs(Box<Expr>),
    Norm(Box<Expr>),
    Floor(Box<Expr>),
    Ceil(Box<Expr>),
    Inner(Vec<Expr>),
    Binom(Box<Expr>, Box<Expr>),
    Matrix(MatrixKind, Vec<Vec<Expr>>),
    Substack(Vec<Expr>),
    Not(Box<Expr>),
    Bracketed(Box<Expr>),    // [ … ]
    Tuple(Vec<Expr>),        // ( a, b, c )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BigOpKind {
    Sum, Prod, Int,
    BigCup, BigCap, BigOplus, BigOtimes, BigVee, BigWedge, BigUplus, BigSqcup,
}

#[derive(Debug, Clone, Default)]
struct Bounds {
    lower: Option<Box<Expr>>,
    upper: Option<Box<Expr>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MatrixKind { Pmatrix, Bmatrix, Vmatrix, VMatrix, BMatrix, Matrix, Cases, Aligned }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BinOp {
    Add, Sub, Mul, Div, Cdot, Times,
    Eq, Lt, Gt, Le, Ge, Ne, Approx, To,
    Pm, Mp,
    In, Subset, Cup, Cap,
    Equiv, Propto, Circ, Ast, Bmod,
    Subseteq, Supseteq, Supset, Subsetneq, Setminus,
    Ll, Gg,
    Perp, Parallel,
    Mid, Nmid,
    Wedge, Vee,
    Oplus, Otimes, Odot, Ominus,
    Nleq, Ngeq, Nsubseteq, Notin,
    Bullet,
    // Order / similarity
    Precedes, Succeeds, Preceq, Succeq,
    Sqsubseteq, Sqsupseteq,
    Cong, Simeq, Asymp,
    // Lattice / multiset ops
    Sqcup, Sqcap, Uplus,
    Amalg, Wr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Prec { Rel, Add, Mul, Unary, Pow, Atom }

fn op_prec(op: BinOp) -> Prec {
    use BinOp::*;
    match op {
        Add | Sub | Pm | Mp | Oplus | Ominus | Sqcup | Sqcap | Uplus => Prec::Add,
        Mul | Div | Cdot | Times | Circ | Ast | Bmod | Otimes | Odot | Bullet
        | Wedge | Vee | Setminus | Amalg | Wr => Prec::Mul,
        _ => Prec::Rel,
    }
}

fn op_bp(op: BinOp) -> (u8, u8) {
    use BinOp::*;
    match op {
        Eq | Lt | Gt | Le | Ge | Ne | Approx | To | Equiv | Propto
        | In | Notin | Subset | Subseteq | Supset | Supseteq | Subsetneq | Nsubseteq
        | Mid | Nmid | Perp | Parallel | Ll | Gg | Nleq | Ngeq
        | Precedes | Succeeds | Preceq | Succeq | Sqsubseteq | Sqsupseteq
        | Cong | Simeq | Asymp => (10, 11),
        Cup | Cap | Sqcup | Sqcap | Uplus => (20, 21),
        Add | Sub | Pm | Mp | Oplus | Ominus => (30, 31),
        Mul | Div | Cdot | Times | Circ | Ast | Bmod | Otimes | Odot | Bullet
        | Wedge | Vee | Setminus | Amalg | Wr => (40, 41),
    }
}

fn bin_from_op(s: &str) -> Option<BinOp> {
    Some(match s {
        "+" => BinOp::Add,
        "-" => BinOp::Sub,
        "*" => BinOp::Mul,
        "/" => BinOp::Div,
        "=" => BinOp::Eq,
        "<" => BinOp::Lt,
        ">" => BinOp::Gt,
        "<=" => BinOp::Le,
        ">=" => BinOp::Ge,
        "!=" => BinOp::Ne,
        _ => return None,
    })
}

fn bin_from_cmd(s: &str) -> Option<BinOp> {
    Some(match s {
        "cdot" => BinOp::Cdot,
        "times" => BinOp::Times,
        "div" => BinOp::Div,
        "pm" => BinOp::Pm,
        "mp" => BinOp::Mp,
        "le" | "leq" => BinOp::Le,
        "ge" | "geq" => BinOp::Ge,
        "ne" | "neq" => BinOp::Ne,
        "approx" => BinOp::Approx,
        "sim" => BinOp::Approx,
        "to" | "rightarrow" => BinOp::To,
        "in" => BinOp::In,
        "subset" => BinOp::Subset,
        "cup" => BinOp::Cup,
        "cap" => BinOp::Cap,
        "equiv" => BinOp::Equiv,
        "propto" => BinOp::Propto,
        "circ" => BinOp::Circ,
        "ast" | "star" => BinOp::Ast,
        "bmod" | "mod" => BinOp::Bmod,
        "subseteq" => BinOp::Subseteq,
        "supseteq" => BinOp::Supseteq,
        "supset" => BinOp::Supset,
        "subsetneq" => BinOp::Subsetneq,
        "setminus" => BinOp::Setminus,
        "ll" => BinOp::Ll,
        "gg" => BinOp::Gg,
        "perp" => BinOp::Perp,
        "parallel" => BinOp::Parallel,
        "mid" => BinOp::Mid,
        "nmid" => BinOp::Nmid,
        "wedge" | "land" => BinOp::Wedge,
        "vee" | "lor" => BinOp::Vee,
        "oplus" => BinOp::Oplus,
        "otimes" => BinOp::Otimes,
        "odot" => BinOp::Odot,
        "ominus" => BinOp::Ominus,
        "nleq" | "nle" => BinOp::Nleq,
        "ngeq" | "nge" => BinOp::Ngeq,
        "nsubseteq" => BinOp::Nsubseteq,
        "notin" | "nin" => BinOp::Notin,
        "bullet" => BinOp::Bullet,
        "prec" => BinOp::Precedes,
        "succ" => BinOp::Succeeds,
        "preceq" => BinOp::Preceq,
        "succeq" => BinOp::Succeq,
        "sqsubseteq" => BinOp::Sqsubseteq,
        "sqsupseteq" => BinOp::Sqsupseteq,
        "cong" => BinOp::Cong,
        "simeq" => BinOp::Simeq,
        "asymp" => BinOp::Asymp,
        "sqcup" => BinOp::Sqcup,
        "sqcap" => BinOp::Sqcap,
        "uplus" => BinOp::Uplus,
        "amalg" => BinOp::Amalg,
        "wr" => BinOp::Wr,
        _ => return None,
    })
}

fn matrix_kind_from(name: &str) -> MatrixKind {
    match name {
        "pmatrix" => MatrixKind::Pmatrix,
        "bmatrix" => MatrixKind::Bmatrix,
        "vmatrix" => MatrixKind::Vmatrix,
        "Vmatrix" => MatrixKind::VMatrix,
        "Bmatrix" => MatrixKind::BMatrix,
        "cases" | "dcases" | "rcases" => MatrixKind::Cases,
        "align" | "align*" | "aligned"
        | "gather" | "gather*" | "gathered"
        | "eqnarray" | "eqnarray*" | "split"
        | "equation" | "equation*" | "multline" | "multline*"
        | "subarray" => MatrixKind::Aligned,
        "smallmatrix" | "array" => MatrixKind::Matrix,
        _ => MatrixKind::Matrix,
    }
}

// ── Parser ────────────────────────────────────────────────────────────────────

struct Parser<'a> {
    toks: &'a [Tok],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&Tok> { self.toks.get(self.pos) }
    fn bump(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        if t.is_some() { self.pos += 1; }
        t
    }
    fn eat(&mut self, t: &Tok) -> bool {
        if self.peek() == Some(t) { self.pos += 1; true } else { false }
    }
}

fn parse_expr(p: &mut Parser, min_bp: u8) -> Expr {
    let mut left = parse_unary(p);
    loop {
        let op = match p.peek() {
            Some(Tok::Op(s)) => bin_from_op(s),
            Some(Tok::Cmd(s)) => bin_from_cmd(s),
            _ => None,
        };
        let Some(op) = op else { break };
        let (lbp, rbp) = op_bp(op);
        if lbp < min_bp { break; }
        p.bump();
        let right = parse_expr(p, rbp);
        left = Expr::Bin(op, Box::new(left), Box::new(right));
    }
    left
}

fn parse_unary(p: &mut Parser) -> Expr {
    if let Some(Tok::Op(s)) = p.peek() {
        match s.as_str() {
            "-" => { p.bump(); return Expr::Neg(Box::new(parse_unary(p))); }
            "+" => { p.bump(); return parse_unary(p); }
            _ => {}
        }
    }
    parse_postfix(p)
}

fn parse_postfix(p: &mut Parser) -> Expr {
    let first = parse_scripted(p);
    let mut items = vec![first];
    while is_juxt_start(p.peek()) {
        items.push(parse_scripted(p));
    }
    // Drop Empty placeholders (e.g. left over from \big / \phantom) so they don't
    // leak as orphan juxt spaces in the output.
    items.retain(|e| !matches!(e, Expr::Empty));
    match items.len() {
        0 => Expr::Empty,
        1 => items.pop().unwrap(),
        _ => Expr::Juxt(items),
    }
}

fn parse_scripted(p: &mut Parser) -> Expr {
    let mut base = parse_atom(p);
    let mut primes = 0usize;
    loop {
        match p.peek() {
            Some(Tok::Caret) => {
                p.bump();
                let exp = parse_atom_or_group(p);
                base = Expr::Pow(Box::new(base), Box::new(exp));
            }
            Some(Tok::Underscore) => {
                p.bump();
                let sub = parse_atom_or_group(p);
                base = Expr::Sub(Box::new(base), Box::new(sub));
            }
            Some(Tok::Prime) => { p.bump(); primes += 1; }
            _ => break,
        }
    }
    if primes > 0 {
        base = Expr::Primed(Box::new(base), primes);
    }
    base
}

fn parse_atom_or_group(p: &mut Parser) -> Expr {
    parse_atom(p)
}

fn parse_atom(p: &mut Parser) -> Expr {
    // Peek-then-bump: unrecognised tokens stay in the stream so outer parsers
    // (matrix body, abs/norm close, end-of-environment) can match them.
    let Some(t) = p.peek().cloned() else { return Expr::Empty };
    match t {
        Tok::Num(n) => { p.bump(); Expr::Atom(n) }
        Tok::Ident(s) => { p.bump(); Expr::Atom(s) }
        Tok::Cmd(c) => { p.bump(); parse_command(p, &c) }
        Tok::Text(s) => { p.bump(); Expr::Text(s) }
        Tok::LBrace => {
            p.bump();
            let inner = parse_expr(p, 0);
            p.eat(&Tok::RBrace);
            inner
        }
        Tok::LParen => {
            p.bump();
            let mut items = vec![parse_expr(p, 0)];
            while p.eat(&Tok::Comma) {
                items.push(parse_expr(p, 0));
            }
            p.eat(&Tok::RParen);
            if items.len() == 1 {
                Expr::Group(Box::new(items.pop().unwrap()))
            } else {
                Expr::Tuple(items)
            }
        }
        Tok::LBracket => {
            p.bump();
            let mut items = vec![parse_expr(p, 0)];
            while p.eat(&Tok::Comma) {
                items.push(parse_expr(p, 0));
            }
            p.eat(&Tok::RBracket);
            if items.len() == 1 {
                Expr::Bracketed(Box::new(items.pop().unwrap()))
            } else {
                // Bracketed comma-list — render with brackets, comma-joined.
                Expr::Bracketed(Box::new(Expr::Tuple(items)))
            }
        }
        Tok::Pipe => {
            p.bump();
            let inner = parse_expr(p, 0);
            p.eat(&Tok::Pipe);
            Expr::Abs(Box::new(inner))
        }
        Tok::DoubleVert => {
            p.bump();
            let inner = parse_expr(p, 0);
            p.eat(&Tok::DoubleVert);
            Expr::Norm(Box::new(inner))
        }
        Tok::LFloor => {
            p.bump();
            let inner = parse_expr(p, 0);
            p.eat(&Tok::RFloor);
            Expr::Floor(Box::new(inner))
        }
        Tok::LCeil => {
            p.bump();
            let inner = parse_expr(p, 0);
            p.eat(&Tok::RCeil);
            Expr::Ceil(Box::new(inner))
        }
        Tok::LAngle => {
            p.bump();
            let mut items = vec![parse_expr(p, 0)];
            while p.eat(&Tok::Comma) {
                items.push(parse_expr(p, 0));
            }
            p.eat(&Tok::RAngle);
            Expr::Inner(items)
        }
        Tok::Begin(env) => {
            p.bump();
            // \begin{array}{spec} and \begin{subarray}{spec} carry a column-spec
            // brace group that is layout-only; discard it before parsing content.
            if env == "array" || env == "subarray" {
                if p.eat(&Tok::LBrace) {
                    while !matches!(p.peek(), Some(Tok::RBrace) | None) { p.bump(); }
                    p.eat(&Tok::RBrace);
                }
            }
            let kind = matrix_kind_from(&env);
            parse_matrix_body(p, kind)
        }
        _ => Expr::Empty,
    }
}

fn parse_command(p: &mut Parser, name: &str) -> Expr {
    match name {
        "frac" | "dfrac" | "tfrac" => {
            let n = parse_brace_group(p);
            let d = parse_brace_group(p);
            Expr::Frac(Box::new(n), Box::new(d))
        }
        "binom" | "dbinom" | "tbinom" => {
            let n = parse_brace_group(p);
            let k = parse_brace_group(p);
            Expr::Binom(Box::new(n), Box::new(k))
        }
        "bar" | "hat" | "vec" | "tilde" | "dot" | "ddot"
        | "overline" | "underline" | "widehat" | "widetilde" | "overrightarrow" => {
            let arg = parse_brace_group(p);
            let canonical = match name {
                "widehat" => "hat",
                "widetilde" => "tilde",
                "overrightarrow" => "vec",
                _ => name,
            };
            Expr::Call(canonical.to_string(), Box::new(arg))
        }
        "mathbb" => {
            let inner = parse_brace_group(p);
            if let Expr::Atom(c) = &inner {
                if c.chars().count() == 1 {
                    return Expr::Bb(c.clone());
                }
            }
            inner
        }
        "mathcal" | "mathscr" => {
            let inner = parse_brace_group(p);
            if let Expr::Atom(c) = &inner {
                let mapped: String = c.chars().map(|ch| script_char(ch).unwrap_or(ch)).collect();
                if mapped != *c { return Expr::Atom(mapped); }
            }
            inner
        }
        "mathfrak" => {
            let inner = parse_brace_group(p);
            if let Expr::Atom(c) = &inner {
                let mapped: String = c.chars().map(|ch| fraktur_char(ch).unwrap_or(ch)).collect();
                if mapped != *c { return Expr::Atom(mapped); }
            }
            inner
        }
        "mathbf" | "mathit" | "mathrm" | "boldsymbol" | "mathsf" | "mathtt"
        | "operatorname" => {
            parse_brace_group(p)
        }
        "pmod" => {
            let n = parse_brace_group(p);
            Expr::Call("pmod".to_string(), Box::new(n))
        }
        "sqrt" => {
            if p.eat(&Tok::LBracket) {
                let _idx = parse_expr(p, 0);
                p.eat(&Tok::RBracket);
            }
            let body = parse_brace_group(p);
            Expr::Sqrt(Box::new(body))
        }
        "sum" | "prod" | "int" | "iint" | "iiint" | "oint"
        | "bigcup" | "bigcap" | "bigoplus" | "bigotimes"
        | "bigvee" | "bigwedge" | "biguplus" | "bigsqcup" => {
            let kind = match name {
                "sum"      => BigOpKind::Sum,
                "prod"     => BigOpKind::Prod,
                "bigcup"   => BigOpKind::BigCup,
                "bigcap"   => BigOpKind::BigCap,
                "bigoplus" => BigOpKind::BigOplus,
                "bigotimes"=> BigOpKind::BigOtimes,
                "bigvee"   => BigOpKind::BigVee,
                "bigwedge" => BigOpKind::BigWedge,
                "biguplus" => BigOpKind::BigUplus,
                "bigsqcup" => BigOpKind::BigSqcup,
                _          => BigOpKind::Int,
            };
            let (mut pre_sup, mut pre_sub) = (None, None);
            for _ in 0..2 {
                match p.peek() {
                    Some(Tok::Underscore) => { p.bump(); pre_sub = Some(Box::new(parse_atom_or_group(p))); }
                    Some(Tok::Caret) => { p.bump(); pre_sup = Some(Box::new(parse_atom_or_group(p))); }
                    _ => break,
                }
            }
            let bounds = Bounds { lower: pre_sub, upper: pre_sup };
            let body = parse_bigop_body(p);
            let (body, diff) = if matches!(kind, BigOpKind::Int) {
                extract_differential(body)
            } else {
                (body, None)
            };
            Expr::BigOp(kind, bounds, Box::new(body), diff)
        }
        "sin" | "cos" | "tan" | "cot" | "sec" | "csc"
        | "arcsin" | "arccos" | "arctan"
        | "sinh" | "cosh" | "tanh"
        | "log" | "ln" | "exp" | "lim" | "liminf" | "limsup" | "min" | "max" | "det"
        | "gcd" | "lcm" | "arg" | "dim" | "ker" | "deg"
        | "sup" | "inf" | "sgn" | "Pr" | "hom" | "im" | "re" | "ord" => {
            let mut sup = None;
            let mut sub = None;
            loop {
                match p.peek() {
                    Some(Tok::Caret) => { p.bump(); sup = Some(parse_atom_or_group(p)); }
                    Some(Tok::Underscore) => { p.bump(); sub = Some(parse_atom_or_group(p)); }
                    _ => break,
                }
            }
            let arg = parse_scripted(p);
            let mut e = Expr::Call(name.to_string(), Box::new(arg));
            if let Some(s) = sub { e = Expr::Sub(Box::new(e), Box::new(s)); }
            if let Some(s) = sup { e = Expr::Pow(Box::new(e), Box::new(s)); }
            e
        }
        // Stacked relations.
        "overset" | "stackrel" => {
            let top = parse_brace_group(p);
            let base = parse_brace_group(p);
            Expr::Pow(Box::new(base), Box::new(top))
        }
        "underset" => {
            let bot = parse_brace_group(p);
            let base = parse_brace_group(p);
            Expr::Sub(Box::new(base), Box::new(bot))
        }
        // \overbrace / \underbrace decorate visually only — pass through;
        // a trailing ^{ann} / _{ann} attaches via parse_scripted.
        "overbrace" | "underbrace" => parse_brace_group(p),
        "substack" => parse_substack(p),
        "neg" | "lnot" => Expr::Not(Box::new(parse_scripted(p))),
        // Delimiter size prefixes — drop; the following delimiter parses normally.
        "big" | "Big" | "bigg" | "Bigg"
        | "bigl" | "bigr" | "Bigl" | "Bigr"
        | "biggl" | "biggr" | "Biggl" | "Biggr" => Expr::Empty,
        // Invisible spacing/sizing — consume the brace group and emit nothing.
        "phantom" | "hphantom" | "vphantom" | "smash" | "mathstrut" => {
            let _ = parse_brace_group(p);
            Expr::Empty
        }
        _ => Expr::Atom(format!("\\{name}")),
    }
}

fn parse_substack(p: &mut Parser) -> Expr {
    if !p.eat(&Tok::LBrace) {
        return parse_atom(p);
    }
    let mut items: Vec<Expr> = Vec::new();
    loop {
        match p.peek() {
            Some(Tok::RBrace) | None => break,
            Some(Tok::Comma) | Some(Tok::RowSep) => { p.bump(); }
            _ => {
                let e = parse_expr(p, 0);
                if matches!(e, Expr::Empty) { p.bump(); }
                else { items.push(e); }
            }
        }
    }
    p.eat(&Tok::RBrace);
    Expr::Substack(items)
}

fn parse_brace_group(p: &mut Parser) -> Expr {
    if p.eat(&Tok::LBrace) {
        // Bare relation atom: `{=}` makes `=` behave as an atom in TeX.
        if let Some(Tok::Op(op)) = p.peek().cloned() {
            let saved = p.pos;
            p.bump();
            if p.eat(&Tok::RBrace) {
                return Expr::Atom(op);
            }
            p.pos = saved;
        }
        let inner = parse_expr(p, 0);
        p.eat(&Tok::RBrace);
        inner
    } else {
        parse_scripted(p)
    }
}

fn parse_bigop_body(p: &mut Parser) -> Expr {
    let first = parse_scripted(p);
    let mut items = vec![first];
    while is_juxt_start(p.peek()) {
        items.push(parse_scripted(p));
    }
    if items.len() == 1 { items.pop().unwrap() } else { Expr::Juxt(items) }
}

fn extract_differential(body: Expr) -> (Expr, Option<String>) {
    let Expr::Juxt(mut items) = body else { return (body, None) };
    if items.len() < 2 { return (Expr::Juxt(items), None); }
    let (last, second_last) = (&items[items.len() - 1], &items[items.len() - 2]);
    if let (Expr::Atom(d), Expr::Atom(v)) = (second_last, last) {
        if d == "d" {
            let var = v.clone();
            items.truncate(items.len() - 2);
            let new_body = if items.len() == 1 { items.pop().unwrap() } else { Expr::Juxt(items) };
            return (new_body, Some(format!("d{var}")));
        }
    }
    (Expr::Juxt(items), None)
}

fn is_juxt_start(t: Option<&Tok>) -> bool {
    match t {
        Some(Tok::Ident(_)) | Some(Tok::Num(_))
        | Some(Tok::LParen) | Some(Tok::LBrace) | Some(Tok::LBracket)
        | Some(Tok::LFloor) | Some(Tok::LCeil) | Some(Tok::LAngle)
        | Some(Tok::Begin(_)) | Some(Tok::Text(_)) => true,
        // Pipe/DoubleVert excluded — they open AND close, so juxt would mis-pair them.
        Some(Tok::Cmd(s)) => bin_from_cmd(s).is_none(),
        _ => false,
    }
}

fn parse_matrix_body(p: &mut Parser, kind: MatrixKind) -> Expr {
    let mut rows: Vec<Vec<Expr>> = vec![Vec::new()];
    loop {
        match p.peek() {
            None | Some(Tok::End(_)) => break,
            Some(Tok::Amp) => { p.bump(); }
            Some(Tok::RowSep) => {
                p.bump();
                rows.push(Vec::new());
            }
            _ => {
                let cell = parse_expr(p, 0);
                if !matches!(cell, Expr::Empty) {
                    rows.last_mut().unwrap().push(cell);
                } else {
                    // Avoid spinning on an unconsumed token.
                    p.bump();
                }
            }
        }
    }
    if matches!(p.peek(), Some(Tok::End(_))) {
        p.bump();
    }
    while rows.last().is_some_and(|r| r.is_empty()) {
        rows.pop();
    }
    Expr::Matrix(kind, rows)
}

// ── Renderer ──────────────────────────────────────────────────────────────────

fn render_expr(e: &Expr, mode: Mode) -> (String, Prec) {
    match e {
        Expr::Empty => (String::new(), Prec::Atom),
        Expr::Atom(s) => (render_symbol(s, mode), Prec::Atom),
        Expr::Text(s) => (s.clone(), Prec::Atom),
        Expr::Bb(s) => (render_bb(s, mode), Prec::Atom),
        Expr::Group(inner) => {
            let (s, _) = render_expr(inner, mode);
            (format!("({s})"), Prec::Atom)
        }
        Expr::Neg(x) => {
            let (s, p) = render_expr(x, mode);
            let s = wrap(s, p, Prec::Unary);
            (format!("-{s}"), Prec::Unary)
        }
        Expr::Bin(op, a, b) => render_bin(*op, a, b, mode),
        Expr::Pow(base, exp) => render_pow(base, exp, mode),
        Expr::Sub(base, sub) => render_sub(base, sub, mode),
        Expr::Primed(base, n) => render_primed(base, *n, mode),
        Expr::Frac(n, d) => render_frac(n, d, mode),
        Expr::Sqrt(x) => render_sqrt(x, mode),
        Expr::BigOp(kind, b, body, diff) => render_bigop(*kind, b, body, diff.as_deref(), mode),
        Expr::Call(name, arg) => render_call(name, arg, mode),
        Expr::Juxt(items) => render_juxt(items, mode),
        Expr::Abs(x) => render_delim_pair(x, "|", "|", mode),
        Expr::Norm(x) => {
            let (open, close) = if mode == Mode::Unicode { ("‖", "‖") } else { ("||", "||") };
            render_delim_pair(x, open, close, mode)
        }
        Expr::Floor(x) => {
            let (open, close) = if mode == Mode::Unicode { ("⌊", "⌋") } else { ("floor(", ")") };
            render_delim_pair(x, open, close, mode)
        }
        Expr::Ceil(x) => {
            let (open, close) = if mode == Mode::Unicode { ("⌈", "⌉") } else { ("ceil(", ")") };
            render_delim_pair(x, open, close, mode)
        }
        Expr::Inner(items) => render_inner(items, mode),
        Expr::Binom(n, k) => render_binom(n, k, mode),
        Expr::Matrix(kind, rows) => render_matrix(*kind, rows, mode),
        Expr::Substack(items) => {
            let parts: Vec<String> = items.iter().map(|e| render_expr(e, mode).0).collect();
            (parts.join(", "), Prec::Atom)
        }
        Expr::Not(x) => {
            let (s, p) = render_expr(x, mode);
            let s = if p < Prec::Unary { format!("({s})") } else { s };
            let sym = if mode == Mode::Unicode { "¬" } else { "!" };
            (format!("{sym}{s}"), Prec::Unary)
        }
        Expr::Bracketed(inner) => {
            // Comma-list inside [] (e.g. [a,b] interval): render items without extra parens.
            let s = match inner.as_ref() {
                Expr::Tuple(items) => {
                    items.iter().map(|e| render_expr(e, mode).0).collect::<Vec<_>>().join(", ")
                }
                _ => render_expr(inner, mode).0,
            };
            (format!("[{s}]"), Prec::Atom)
        }
        Expr::Tuple(items) => {
            let parts: Vec<String> = items.iter().map(|e| render_expr(e, mode).0).collect();
            (format!("({})", parts.join(", ")), Prec::Atom)
        }
    }
}

fn wrap(s: String, child: Prec, parent: Prec) -> String {
    if child < parent { format!("({s})") } else { s }
}

fn render_bin(op: BinOp, a: &Expr, b: &Expr, mode: Mode) -> (String, Prec) {
    let here = op_prec(op);
    let (sa, pa) = render_expr(a, mode);
    let (sb, pb) = render_expr(b, mode);
    let sa = wrap(sa, pa, here);
    let sb = match op {
        BinOp::Sub | BinOp::Div => {
            if pb <= here { format!("({sb})") } else { sb }
        }
        _ => wrap(sb, pb, here),
    };
    (format!("{sa} {sym} {sb}", sym = bin_symbol(op, mode)), here)
}

fn render_pow(base: &Expr, exp: &Expr, mode: Mode) -> (String, Prec) {
    let (sb, pb) = render_expr(base, mode);
    // Strict: nested Pow/Sub/Call in the base need parens to disambiguate.
    let sb = if pb < Prec::Atom { format!("({sb})") } else { sb };
    let (se, pe) = render_expr(exp, mode);
    if mode == Mode::Unicode {
        if let Some(sup) = to_superscript(&se) {
            return (format!("{sb}{sup}"), Prec::Pow);
        }
    }
    let exp_str = if pe >= Prec::Pow { se } else { format!("({se})") };
    (format!("{sb}^{exp_str}"), Prec::Pow)
}

fn render_sub(base: &Expr, sub: &Expr, mode: Mode) -> (String, Prec) {
    let (sb, pb) = render_expr(base, mode);
    let sb = if pb < Prec::Atom { format!("({sb})") } else { sb };
    let (ss, ps) = render_expr(sub, mode);
    if mode == Mode::Unicode {
        if let Some(sub_uni) = to_subscript(&ss) {
            return (format!("{sb}{sub_uni}"), Prec::Pow);
        }
    }
    let sub_str = if ps >= Prec::Pow { ss } else { format!("({ss})") };
    (format!("{sb}_{sub_str}"), Prec::Pow)
}

fn render_primed(base: &Expr, n: usize, mode: Mode) -> (String, Prec) {
    let (sb, pb) = render_expr(base, mode);
    let sb = if pb < Prec::Atom { format!("({sb})") } else { sb };
    let suffix = match (mode, n) {
        (_, 0) => String::new(),
        (Mode::Unicode, 1) => "′".into(),
        (Mode::Unicode, 2) => "″".into(),
        (Mode::Unicode, 3) => "‴".into(),
        (Mode::Unicode, 4) => "⁗".into(),
        (Mode::Unicode, k) => "′".repeat(k),
        (Mode::Ascii, k) => "'".repeat(k),
    };
    (format!("{sb}{suffix}"), Prec::Pow)
}

fn render_frac(n: &Expr, d: &Expr, mode: Mode) -> (String, Prec) {
    let (sn, pn) = render_expr(n, mode);
    let (sd, pd) = render_expr(d, mode);
    let sym = if mode == Mode::Unicode { "÷" } else { "/" };
    // Strict on both sides so adjacent ÷ never becomes ambiguous.
    let sn = if pn <= Prec::Mul { format!("({sn})") } else { sn };
    let sd = if pd <= Prec::Mul { format!("({sd})") } else { sd };
    (format!("{sn} {sym} {sd}"), Prec::Mul)
}

fn render_sqrt(x: &Expr, mode: Mode) -> (String, Prec) {
    let (s, _) = render_expr(x, mode);
    let simple = matches!(x, Expr::Atom(_));
    let out = match (mode, simple) {
        (Mode::Unicode, true) => format!("√{s}"),
        (Mode::Unicode, false) => format!("√({s})"),
        (Mode::Ascii, _) => format!("sqrt({s})"),
    };
    (out, Prec::Atom)
}

fn render_bigop(kind: BigOpKind, b: &Bounds, body: &Expr, diff: Option<&str>, mode: Mode) -> (String, Prec) {
    let sym = match (kind, mode) {
        (BigOpKind::Sum, Mode::Unicode) => "∑",
        (BigOpKind::Sum, Mode::Ascii) => "sum",
        (BigOpKind::Prod, Mode::Unicode) => "∏",
        (BigOpKind::Prod, Mode::Ascii) => "prod",
        (BigOpKind::Int, Mode::Unicode) => "∫",
        (BigOpKind::Int, Mode::Ascii) => "int",
        (BigOpKind::BigCup,   Mode::Unicode) => "⋃",
        (BigOpKind::BigCup,   Mode::Ascii)   => "Union",
        (BigOpKind::BigCap,   Mode::Unicode) => "⋂",
        (BigOpKind::BigCap,   Mode::Ascii)   => "Inter",
        (BigOpKind::BigOplus, Mode::Unicode) => "⨁",
        (BigOpKind::BigOplus, Mode::Ascii)   => "Oplus",
        (BigOpKind::BigOtimes,Mode::Unicode) => "⨂",
        (BigOpKind::BigOtimes,Mode::Ascii)   => "Otimes",
        (BigOpKind::BigVee,   Mode::Unicode) => "⋁",
        (BigOpKind::BigVee,   Mode::Ascii)   => "Or",
        (BigOpKind::BigWedge, Mode::Unicode) => "⋀",
        (BigOpKind::BigWedge, Mode::Ascii)   => "And",
        (BigOpKind::BigUplus, Mode::Unicode) => "⨄",
        (BigOpKind::BigUplus, Mode::Ascii)   => "Uplus",
        (BigOpKind::BigSqcup, Mode::Unicode) => "⨆",
        (BigOpKind::BigSqcup, Mode::Ascii)   => "Sqcup",
    };
    let bounds = format_bounds(b, mode);
    let (body_str, body_prec) = render_expr(body, mode);
    let body_str = if body_prec < Prec::Mul { format!("({body_str})") } else { body_str };

    let mut out = if bounds.is_empty() {
        format!("{sym} {body_str}")
    } else {
        format!("{sym}{bounds} {body_str}")
    };
    if let Some(d) = diff {
        out.push(' ');
        out.push_str(d);
    }
    (out, Prec::Mul)
}

fn format_bounds(b: &Bounds, mode: Mode) -> String {
    match (b.lower.as_deref(), b.upper.as_deref()) {
        (None, None) => String::new(),
        (Some(lo), None) => {
            let (s, _) = render_expr(lo, mode);
            format!("[{s}]")
        }
        (None, Some(hi)) => {
            let (s, _) = render_expr(hi, mode);
            format!("[..{s}]")
        }
        (Some(lo), Some(hi)) => {
            let (his, _) = render_expr(hi, mode);
            // Iteration form: `_{var = start}` → render as `[var = start..end]`.
            if let Expr::Bin(BinOp::Eq, lhs, rhs) = lo {
                let (var, _) = render_expr(lhs, mode);
                let (start, _) = render_expr(rhs, mode);
                format!("[{var} = {start}..{his}]")
            } else {
                let (los, _) = render_expr(lo, mode);
                format!("[{los}..{his}]")
            }
        }
    }
}

fn render_call(name: &str, arg: &Expr, mode: Mode) -> (String, Prec) {
    // \pmod{n} → "(mod n)" suffix — different shape from regular function calls.
    if name == "pmod" {
        let (s, _) = render_expr(arg, mode);
        return (format!("(mod {s})"), Prec::Atom);
    }
    if matches!(arg, Expr::Empty) {
        return (name.to_string(), Prec::Pow);
    }
    let (s, p) = render_expr(arg, mode);
    if matches!(arg, Expr::Group(_) | Expr::Tuple(_) | Expr::Bracketed(_)) {
        return (format!("{name}{s}"), Prec::Atom);
    }
    if matches!(arg, Expr::Atom(_) | Expr::Bb(_) | Expr::Text(_)) {
        return (format!("{name} {s}"), Prec::Pow);
    }
    if p >= Prec::Pow {
        (format!("{name} {s}"), Prec::Pow)
    } else {
        (format!("{name}({s})"), Prec::Atom)
    }
}

fn render_juxt(items: &[Expr], mode: Mode) -> (String, Prec) {
    let parts: Vec<String> = items.iter().map(|e| {
        let (s, p) = render_expr(e, mode);
        wrap(s, p, Prec::Mul)
    }).collect();
    let mut out = String::new();
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            // Skip the joiner space when either side already pads with whitespace
            // (e.g. \text{ if } already carries its own surrounding spaces).
            let prev = &parts[i - 1];
            if !prev.ends_with(' ') && !part.starts_with(' ') {
                out.push(' ');
            }
        }
        out.push_str(part);
    }
    (out, Prec::Mul)
}

fn render_delim_pair(x: &Expr, open: &str, close: &str, mode: Mode) -> (String, Prec) {
    let (s, _) = render_expr(x, mode);
    (format!("{open}{s}{close}"), Prec::Atom)
}

fn render_inner(items: &[Expr], mode: Mode) -> (String, Prec) {
    let (open, close) = if mode == Mode::Unicode { ("⟨", "⟩") } else { ("<", ">") };
    let parts: Vec<String> = items.iter().map(|e| render_expr(e, mode).0).collect();
    (format!("{open}{}{close}", parts.join(", ")), Prec::Atom)
}

fn render_binom(n: &Expr, k: &Expr, _mode: Mode) -> (String, Prec) {
    let (sn, _) = render_expr(n, _mode);
    let (sk, _) = render_expr(k, _mode);
    (format!("C({sn}, {sk})"), Prec::Atom)
}

fn render_matrix(kind: MatrixKind, rows: &[Vec<Expr>], mode: Mode) -> (String, Prec) {
    match kind {
        MatrixKind::Cases => render_cases(rows, mode),
        MatrixKind::Aligned => render_aligned(rows, mode),
        _ => {
            let (open, close) = matrix_brackets(kind, mode);
            let row_strs: Vec<String> = rows.iter().map(|row| {
                let cells: Vec<String> = row.iter().map(|e| render_expr(e, mode).0).collect();
                cells.join(", ")
            }).collect();
            (format!("{open}{}{close}", row_strs.join("; ")), Prec::Atom)
        }
    }
}

fn render_cases(rows: &[Vec<Expr>], mode: Mode) -> (String, Prec) {
    let row_strs: Vec<String> = rows.iter().map(|row| match row.len() {
        0 => String::new(),
        1 => render_expr(&row[0], mode).0,
        _ => {
            let val = render_expr(&row[0], mode).0;
            let parts: Vec<String> = row[1..].iter().map(|e| render_expr(e, mode).0).collect();
            // Smart-join: avoid double space when adjacent pieces already pad.
            let mut cond = String::new();
            for (i, part) in parts.iter().enumerate() {
                if i > 0 {
                    let prev = &parts[i - 1];
                    if !prev.ends_with(' ') && !part.starts_with(' ') { cond.push(' '); }
                }
                cond.push_str(part);
            }
            let trimmed = cond.trim();
            let lower = trimmed.to_ascii_lowercase();
            // Skip auto-prefix when source already supplies the connector
            // (\text{if …}, \text{otherwise}, \text{when …}).
            let already_phrased = lower.starts_with("if ")
                || lower.starts_with("when ")
                || lower.starts_with("for ")
                || lower.starts_with("else")
                || lower == "otherwise";
            let sep = if already_phrased { " " } else { " if " };
            format!("{val}{sep}{trimmed}")
        }
    }).collect();
    (format!("{{{}}}", row_strs.join("; ")), Prec::Atom)
}

fn render_aligned(rows: &[Vec<Expr>], mode: Mode) -> (String, Prec) {
    // `&` is an alignment marker only — cells in a row concatenate without separator.
    // Trim each row to avoid leading spaces when the first cell is empty (e.g. `\\ = c`).
    let row_strs: Vec<String> = rows.iter().map(|row| {
        let parts: Vec<String> = row.iter().map(|e| render_expr(e, mode).0).collect();
        parts.concat().trim().to_string()
    }).collect();
    (row_strs.join("; "), Prec::Atom)
}

fn matrix_brackets(kind: MatrixKind, mode: Mode) -> (&'static str, &'static str) {
    use MatrixKind::*;
    match (kind, mode) {
        (Pmatrix, _)              => ("(", ")"),
        (Bmatrix | Matrix, _)     => ("[", "]"),
        (Vmatrix, _)              => ("|", "|"),
        (VMatrix, Mode::Unicode)  => ("‖", "‖"),
        (VMatrix, Mode::Ascii)    => ("||", "||"),
        (BMatrix, _)              => ("{", "}"),
        // Cases/Aligned use bespoke renderers and never hit this path.
        (Cases | Aligned, _)      => ("", ""),
    }
}

fn render_bb(c: &str, mode: Mode) -> String {
    if mode == Mode::Unicode {
        if let Some(u) = blackboard_bold(c) { return u.to_string(); }
    }
    c.to_string()
}

fn blackboard_bold(c: &str) -> Option<&'static str> {
    Some(match c {
        "A" => "𝔸", "B" => "𝔹", "C" => "ℂ", "D" => "𝔻", "E" => "𝔼",
        "F" => "𝔽", "G" => "𝔾", "H" => "ℍ", "I" => "𝕀", "J" => "𝕁",
        "K" => "𝕂", "L" => "𝕃", "M" => "𝕄", "N" => "ℕ", "O" => "𝕆",
        "P" => "ℙ", "Q" => "ℚ", "R" => "ℝ", "S" => "𝕊", "T" => "𝕋",
        "U" => "𝕌", "V" => "𝕍", "W" => "𝕎", "X" => "𝕏", "Y" => "𝕐",
        "Z" => "ℤ",
        _ => return None,
    })
}

fn script_char(c: char) -> Option<char> {
    Some(match c {
        'A' => '𝒜', 'B' => 'ℬ', 'C' => '𝒞', 'D' => '𝒟', 'E' => 'ℰ',
        'F' => 'ℱ', 'G' => '𝒢', 'H' => 'ℋ', 'I' => 'ℐ', 'J' => '𝒥',
        'K' => '𝒦', 'L' => 'ℒ', 'M' => 'ℳ', 'N' => '𝒩', 'O' => '𝒪',
        'P' => '𝒫', 'Q' => '𝒬', 'R' => 'ℛ', 'S' => '𝒮', 'T' => '𝒯',
        'U' => '𝒰', 'V' => '𝒱', 'W' => '𝒲', 'X' => '𝒳', 'Y' => '𝒴',
        'Z' => '𝒵',
        _ => return None,
    })
}

fn fraktur_char(c: char) -> Option<char> {
    Some(match c {
        'A' => '𝔄', 'B' => '𝔅', 'C' => 'ℭ', 'D' => '𝔇', 'E' => '𝔈',
        'F' => '𝔉', 'G' => '𝔊', 'H' => 'ℌ', 'I' => 'ℑ', 'J' => '𝔍',
        'K' => '𝔎', 'L' => '𝔏', 'M' => '𝔐', 'N' => '𝔑', 'O' => '𝔒',
        'P' => '𝔓', 'Q' => '𝔔', 'R' => 'ℜ', 'S' => '𝔖', 'T' => '𝔗',
        'U' => '𝔘', 'V' => '𝔙', 'W' => '𝔚', 'X' => '𝔛', 'Y' => '𝔜',
        'Z' => 'ℨ',
        'a' => '𝔞', 'b' => '𝔟', 'c' => '𝔠', 'd' => '𝔡', 'e' => '𝔢',
        'f' => '𝔣', 'g' => '𝔤', 'h' => '𝔥', 'i' => '𝔦', 'j' => '𝔧',
        'k' => '𝔨', 'l' => '𝔩', 'm' => '𝔪', 'n' => '𝔫', 'o' => '𝔬',
        'p' => '𝔭', 'q' => '𝔮', 'r' => '𝔯', 's' => '𝔰', 't' => '𝔱',
        'u' => '𝔲', 'v' => '𝔳', 'w' => '𝔴', 'x' => '𝔵', 'y' => '𝔶',
        'z' => '𝔷',
        _ => return None,
    })
}

// ── Symbol tables ─────────────────────────────────────────────────────────────

fn render_symbol(s: &str, mode: Mode) -> String {
    if let Some(name) = s.strip_prefix('\\') {
        if mode == Mode::Unicode {
            if let Some(u) = unicode_symbol(name) {
                return u.to_string();
            }
        }
        return ascii_symbol(name).to_string();
    }
    s.to_string()
}

fn unicode_symbol(name: &str) -> Option<&'static str> {
    Some(match name {
        "alpha" => "α", "beta" => "β", "gamma" => "γ", "delta" => "δ",
        "epsilon" => "ε", "varepsilon" => "ε",
        "zeta" => "ζ", "eta" => "η", "theta" => "θ", "vartheta" => "ϑ",
        "iota" => "ι", "kappa" => "κ", "lambda" => "λ", "mu" => "μ",
        "nu" => "ν", "xi" => "ξ", "omicron" => "ο", "pi" => "π",
        "rho" => "ρ", "sigma" => "σ", "varsigma" => "ς",
        "tau" => "τ", "upsilon" => "υ", "phi" => "φ", "varphi" => "ϕ",
        "chi" => "χ", "psi" => "ψ", "omega" => "ω",
        "Gamma" => "Γ", "Delta" => "Δ", "Theta" => "Θ", "Lambda" => "Λ",
        "Xi" => "Ξ", "Pi" => "Π", "Sigma" => "Σ", "Upsilon" => "Υ",
        "Phi" => "Φ", "Psi" => "Ψ", "Omega" => "Ω",
        "infty" | "infinity" => "∞",
        "partial" => "∂", "nabla" => "∇",
        "cdots" | "ldots" | "dots" | "vdots" | "ddots" => "…",
        "forall" => "∀", "exists" => "∃",
        "emptyset" | "varnothing" => "∅",
        "Rightarrow" | "implies" => "⇒",
        "Leftarrow" => "⇐",
        "Leftrightarrow" | "iff" => "⇔",
        "leftarrow" | "gets" => "←",
        "mapsto" => "↦",
        "aleph" => "ℵ",
        "hbar" => "ℏ",
        "ell" => "ℓ",
        "Re" => "ℜ",
        "Im" => "ℑ",
        "wp" => "℘",
        "imath" => "ı",
        "jmath" => "ȷ",
        "angle" => "∠",
        "therefore" => "∴",
        "because" => "∵",
        "complement" => "∁",
        "top" => "⊤",
        "bot" => "⊥",
        "square" | "Box" => "□",
        "triangle" => "△",
        "diamond" | "Diamond" => "⋄",
        "dagger" => "†",
        "ddagger" => "‡",
        "vdash" => "⊢",
        "dashv" => "⊣",
        "models" => "⊨",
        // Arrows
        "uparrow" => "↑",
        "downarrow" => "↓",
        "updownarrow" => "↕",
        "Uparrow" => "⇑",
        "Downarrow" => "⇓",
        "longrightarrow" => "⟶",
        "longleftarrow" => "⟵",
        "longmapsto" => "⟼",
        "longleftrightarrow" => "⟷",
        "Longrightarrow" => "⟹",
        "Longleftarrow" => "⟸",
        "Longleftrightarrow" => "⟺",
        "hookrightarrow" => "↪",
        "hookleftarrow" => "↩",
        "rightharpoonup" => "⇀",
        "leftharpoonup" => "↼",
        "rightleftharpoons" => "⇌",
        "nwarrow" => "↖",
        "nearrow" => "↗",
        "searrow" => "↘",
        "swarrow" => "↙",
        // Small extras
        "nexists" => "∄",
        "flat" => "♭",
        "sharp" => "♯",
        "natural" => "♮",
        "oiint" => "∯",
        "oiiint" => "∰",
        "iiiint" => "⨌",
        "heartsuit" => "♥",
        "spadesuit" => "♠",
        "clubsuit" => "♣",
        "diamondsuit" => "♦",
        "sphericalangle" => "∢",
        "measuredangle" => "∡",
        _ => return None,
    })
}

fn ascii_symbol(name: &str) -> &str {
    match name {
        "infty" | "infinity" => "inf",
        "Rightarrow" | "implies" => "=>",
        "Leftarrow" => "<=",
        "Leftrightarrow" | "iff" => "<=>",
        "leftarrow" | "gets" => "<-",
        "mapsto" => "|->",
        "cdots" | "ldots" | "dots" | "vdots" | "ddots" => "...",
        "uparrow" | "Uparrow" => "^",
        "downarrow" | "Downarrow" => "v",
        "updownarrow" => "^v",
        "longrightarrow" | "hookrightarrow" | "rightharpoonup" | "nearrow" | "searrow" => "->",
        "longleftarrow"  | "hookleftarrow"  | "leftharpoonup"  | "nwarrow" | "swarrow" => "<-",
        "longleftrightarrow" | "rightleftharpoons" => "<->",
        "longmapsto" => "|->",
        "Longrightarrow" => "=>",
        "Longleftarrow" => "<=",
        "Longleftrightarrow" => "<=>",
        _ => name,
    }
}

fn bin_symbol(op: BinOp, mode: Mode) -> &'static str {
    use BinOp::*;
    match (op, mode) {
        (Add, _) => "+",
        (Sub, _) => "-",
        (Mul | Cdot, Mode::Unicode) => "·",
        (Mul | Cdot, Mode::Ascii) => "*",
        (Div, Mode::Unicode) => "÷",
        (Div, Mode::Ascii) => "/",
        (Times, Mode::Unicode) => "×",
        (Times, Mode::Ascii) => "x",
        (Eq, _) => "=",
        (Lt, _) => "<",
        (Gt, _) => ">",
        (Le, Mode::Unicode) => "≤",
        (Le, Mode::Ascii) => "<=",
        (Ge, Mode::Unicode) => "≥",
        (Ge, Mode::Ascii) => ">=",
        (Ne, Mode::Unicode) => "≠",
        (Ne, Mode::Ascii) => "!=",
        (Approx, Mode::Unicode) => "≈",
        (Approx, Mode::Ascii) => "~=",
        (To, Mode::Unicode) => "→",
        (To, Mode::Ascii) => "->",
        (Pm, Mode::Unicode) => "±",
        (Pm, Mode::Ascii) => "+/-",
        (Mp, Mode::Unicode) => "∓",
        (Mp, Mode::Ascii) => "-/+",
        (In, Mode::Unicode) => "∈",
        (In, Mode::Ascii) => "in",
        (Subset, Mode::Unicode) => "⊂",
        (Subset, Mode::Ascii) => "subset",
        (Cup, Mode::Unicode) => "∪",
        (Cup, Mode::Ascii) => "U",
        (Cap, Mode::Unicode) => "∩",
        (Cap, Mode::Ascii) => "&",
        (Equiv, Mode::Unicode) => "≡",
        (Equiv, Mode::Ascii) => "equiv",
        (Propto, Mode::Unicode) => "∝",
        (Propto, Mode::Ascii) => "propto",
        (Circ, Mode::Unicode) => "∘",
        (Circ, Mode::Ascii) => "o",
        (Ast, _) => "*",
        (Bmod, _) => "mod",
        (Subseteq, Mode::Unicode) => "⊆",
        (Subseteq, Mode::Ascii) => "subseteq",
        (Supseteq, Mode::Unicode) => "⊇",
        (Supseteq, Mode::Ascii) => "supseteq",
        (Supset, Mode::Unicode) => "⊃",
        (Supset, Mode::Ascii) => "supset",
        (Subsetneq, Mode::Unicode) => "⊊",
        (Subsetneq, Mode::Ascii) => "subsetneq",
        (Setminus, Mode::Unicode) => "∖",
        (Setminus, Mode::Ascii) => "\\",
        (Ll, Mode::Unicode) => "≪",
        (Ll, Mode::Ascii) => "<<",
        (Gg, Mode::Unicode) => "≫",
        (Gg, Mode::Ascii) => ">>",
        (Perp, Mode::Unicode) => "⟂",
        (Perp, Mode::Ascii) => "_|_",
        (Parallel, Mode::Unicode) => "∥",
        (Parallel, Mode::Ascii) => "||",
        (Mid, Mode::Unicode) => "∣",
        (Mid, Mode::Ascii) => "|",
        (Nmid, Mode::Unicode) => "∤",
        (Nmid, Mode::Ascii) => "!|",
        (Wedge, Mode::Unicode) => "∧",
        (Wedge, Mode::Ascii) => "and",
        (Vee, Mode::Unicode) => "∨",
        (Vee, Mode::Ascii) => "or",
        (Oplus, Mode::Unicode) => "⊕",
        (Oplus, Mode::Ascii) => "(+)",
        (Otimes, Mode::Unicode) => "⊗",
        (Otimes, Mode::Ascii) => "(x)",
        (Odot, Mode::Unicode) => "⊙",
        (Odot, Mode::Ascii) => "(.)",
        (Ominus, Mode::Unicode) => "⊖",
        (Ominus, Mode::Ascii) => "(-)",
        (Nleq, Mode::Unicode) => "≰",
        (Nleq, Mode::Ascii) => "!<=",
        (Ngeq, Mode::Unicode) => "≱",
        (Ngeq, Mode::Ascii) => "!>=",
        (Nsubseteq, Mode::Unicode) => "⊈",
        (Nsubseteq, Mode::Ascii) => "!subseteq",
        (Notin, Mode::Unicode) => "∉",
        (Notin, Mode::Ascii) => "not in",
        (Bullet, Mode::Unicode) => "•",
        (Bullet, Mode::Ascii) => ".",
        (Precedes,   Mode::Unicode) => "≺",
        (Precedes,   Mode::Ascii)   => "prec",
        (Succeeds,   Mode::Unicode) => "≻",
        (Succeeds,   Mode::Ascii)   => "succ",
        (Preceq,     Mode::Unicode) => "⪯",
        (Preceq,     Mode::Ascii)   => "preceq",
        (Succeq,     Mode::Unicode) => "⪰",
        (Succeq,     Mode::Ascii)   => "succeq",
        (Sqsubseteq, Mode::Unicode) => "⊑",
        (Sqsubseteq, Mode::Ascii)   => "sqsubseteq",
        (Sqsupseteq, Mode::Unicode) => "⊒",
        (Sqsupseteq, Mode::Ascii)   => "sqsupseteq",
        (Cong,       Mode::Unicode) => "≅",
        (Cong,       Mode::Ascii)   => "cong",
        (Simeq,      Mode::Unicode) => "≃",
        (Simeq,      Mode::Ascii)   => "simeq",
        (Asymp,      Mode::Unicode) => "≍",
        (Asymp,      Mode::Ascii)   => "asymp",
        (Sqcup,      Mode::Unicode) => "⊔",
        (Sqcup,      Mode::Ascii)   => "sqcup",
        (Sqcap,      Mode::Unicode) => "⊓",
        (Sqcap,      Mode::Ascii)   => "sqcap",
        (Uplus,      Mode::Unicode) => "⊎",
        (Uplus,      Mode::Ascii)   => "uplus",
        (Amalg,      Mode::Unicode) => "⨿",
        (Amalg,      Mode::Ascii)   => "amalg",
        (Wr,         Mode::Unicode) => "≀",
        (Wr,         Mode::Ascii)   => "wr",
    }
}

pub fn to_superscript(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        out.push(sup_char(c)?);
    }
    Some(out)
}

pub fn to_subscript(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        out.push(sub_char(c)?);
    }
    Some(out)
}

fn sup_char(c: char) -> Option<char> {
    Some(match c {
        '0' => '⁰', '1' => '¹', '2' => '²', '3' => '³', '4' => '⁴',
        '5' => '⁵', '6' => '⁶', '7' => '⁷', '8' => '⁸', '9' => '⁹',
        '+' => '⁺', '-' => '⁻', '=' => '⁼', '(' => '⁽', ')' => '⁾',
        'a' => 'ᵃ', 'b' => 'ᵇ', 'c' => 'ᶜ', 'd' => 'ᵈ', 'e' => 'ᵉ',
        'f' => 'ᶠ', 'g' => 'ᵍ', 'h' => 'ʰ', 'i' => 'ⁱ', 'j' => 'ʲ',
        'k' => 'ᵏ', 'l' => 'ˡ', 'm' => 'ᵐ', 'n' => 'ⁿ', 'o' => 'ᵒ',
        'p' => 'ᵖ', 'r' => 'ʳ', 's' => 'ˢ', 't' => 'ᵗ', 'u' => 'ᵘ',
        'v' => 'ᵛ', 'w' => 'ʷ', 'x' => 'ˣ', 'y' => 'ʸ', 'z' => 'ᶻ',
        _ => return None,
    })
}

fn sub_char(c: char) -> Option<char> {
    Some(match c {
        '0' => '₀', '1' => '₁', '2' => '₂', '3' => '₃', '4' => '₄',
        '5' => '₅', '6' => '₆', '7' => '₇', '8' => '₈', '9' => '₉',
        '+' => '₊', '-' => '₋', '=' => '₌', '(' => '₍', ')' => '₎',
        'a' => 'ₐ', 'e' => 'ₑ', 'h' => 'ₕ', 'i' => 'ᵢ', 'j' => 'ⱼ',
        'k' => 'ₖ', 'l' => 'ₗ', 'm' => 'ₘ', 'n' => 'ₙ', 'o' => 'ₒ',
        'p' => 'ₚ', 'r' => 'ᵣ', 's' => 'ₛ', 't' => 'ₜ', 'u' => 'ᵤ',
        'v' => 'ᵥ', 'x' => 'ₓ',
        _ => return None,
    })
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn r(s: &str) -> String { render_math_block_with(&format!("${s}$"), Mode::Unicode) }
    fn a(s: &str) -> String { render_math_block_with(&format!("${s}$"), Mode::Ascii) }

    // ── Headline examples ────────────────────────────────────────────────────

    #[test]
    fn fraction_with_compound_numerator() {
        assert_eq!(r(r"\frac{x^2 + 1}{\alpha}"), "(x² + 1) ÷ α");
    }

    #[test]
    fn sqrt_compound() {
        assert_eq!(r(r"\sqrt{x^2 + y^2}"), "√(x² + y²)");
    }

    #[test]
    fn sum_with_iteration() {
        assert_eq!(r(r"\sum_{i=0}^{n} x_i"), "∑[i = 0..n] xᵢ");
    }

    #[test]
    fn integral_with_differential() {
        assert_eq!(r(r"\int_0^1 x^2 dx"), "∫[0..1] x² dx");
    }

    #[test]
    fn nested_fractions_wrap_recursively() {
        assert_eq!(r(r"\frac{\frac{1}{2}}{\frac{3}{4}}"), "(1 ÷ 2) ÷ (3 ÷ 4)");
    }

    // ── Sqrt variants ────────────────────────────────────────────────────────

    #[test]
    fn sqrt_atom_has_no_parens() {
        assert_eq!(r(r"\sqrt{x}"), "√x");
        assert_eq!(r(r"\sqrt{2}"), "√2");
    }

    #[test]
    fn sqrt_with_index_ignored() {
        assert_eq!(r(r"\sqrt[3]{x}"), "√x");
    }

    // ── Powers and subscripts ────────────────────────────────────────────────

    #[test]
    fn unicode_superscript_for_digits() {
        assert_eq!(r("x^2"), "x²");
        assert_eq!(r("x^{12}"), "x¹²");
    }

    #[test]
    fn fallback_for_unmapped_superscript() {
        // 'q' has no Unicode superscript form.
        assert_eq!(r("x^q"), "x^q");
        // Multi-char compound exponents that can't all map.
        assert_eq!(r("x^{a+q}"), "x^(a + q)");
    }

    #[test]
    fn pow_base_wraps_nested() {
        // (x^2)^3 must be visually distinct from x^(2^3).
        assert_eq!(r("{x^2}^3"), "(x²)³");
    }

    #[test]
    fn subscript_letter_falls_back() {
        // 'y' has no Unicode subscript form.
        assert_eq!(r("x_y"), "x_y");
    }

    // ── Functions ────────────────────────────────────────────────────────────

    #[test]
    fn function_with_atom_arg() {
        assert_eq!(r(r"\sin x"), "sin x");
        assert_eq!(r(r"\ln y"), "ln y");
    }

    #[test]
    fn function_with_paren_arg() {
        assert_eq!(r(r"\sin(x^2)"), "sin(x²)");
    }

    #[test]
    fn function_with_compound_arg_gets_parens() {
        assert_eq!(r(r"\log{x + 1}"), "log(x + 1)");
    }

    // ── Big operators ────────────────────────────────────────────────────────

    #[test]
    fn sum_without_iteration_var() {
        assert_eq!(r(r"\sum_{n} x_n"), "∑[n] xₙ");
    }

    #[test]
    fn prod_with_bounds() {
        assert_eq!(r(r"\prod_{k=1}^{n} k"), "∏[k = 1..n] k");
    }

    #[test]
    fn integral_bounds_only() {
        assert_eq!(r(r"\int_a^b f(x) dx"), "∫[a..b] f (x) dx");
    }

    // ── Greek ────────────────────────────────────────────────────────────────

    #[test]
    fn greek_letters() {
        assert_eq!(r(r"\alpha + \beta - \gamma"), "α + β - γ");
        assert_eq!(r(r"\Sigma \Omega"), "Σ Ω");
    }

    // ── Operators ────────────────────────────────────────────────────────────

    #[test]
    fn unicode_operators() {
        assert_eq!(r(r"a \leq b"), "a ≤ b");
        assert_eq!(r(r"a \neq b"), "a ≠ b");
        assert_eq!(r(r"a \cdot b"), "a · b");
        assert_eq!(r(r"a \times b"), "a × b");
        assert_eq!(r(r"x \to \infty"), "x → ∞");
    }

    #[test]
    fn precedence_inserts_parens() {
        assert_eq!(r("a * (b + c)"), "a · (b + c)");
        assert_eq!(r("a - {b - c}"), "a - (b - c)");
        assert_eq!(r("a - b - c"), "a - b - c");
    }

    // ── ASCII mode ───────────────────────────────────────────────────────────

    #[test]
    fn ascii_fraction() {
        assert_eq!(a(r"\frac{x^2 + 1}{\alpha}"), "(x^2 + 1) / alpha");
    }

    #[test]
    fn ascii_sqrt() {
        assert_eq!(a(r"\sqrt{x^2 + y^2}"), "sqrt(x^2 + y^2)");
    }

    #[test]
    fn ascii_sum() {
        assert_eq!(a(r"\sum_{i=0}^{n} x_i"), "sum[i = 0..n] x_i");
    }

    #[test]
    fn ascii_integral() {
        assert_eq!(a(r"\int_0^1 x^2 dx"), "int[0..1] x^2 dx");
    }

    #[test]
    fn ascii_nested_fraction() {
        assert_eq!(a(r"\frac{\frac{1}{2}}{\frac{3}{4}}"), "(1 / 2) / (3 / 4)");
    }

    // ── Prose passthrough ────────────────────────────────────────────────────

    #[test]
    fn prose_passthrough() {
        assert_eq!(render_math_lines("plain text, no math"), "plain text, no math");
    }

    #[test]
    fn prose_with_inline_math() {
        assert_eq!(
            render_math_lines("where $x^2 + 1$ is the input"),
            "where x² + 1 is the input"
        );
    }

    #[test]
    fn prose_with_multiple_regions() {
        assert_eq!(
            render_math_lines(r"first $\alpha$, then $\beta$"),
            "first α, then β"
        );
    }

    // ── Abs and norm ─────────────────────────────────────────────────────────

    #[test]
    fn abs_basic() {
        assert_eq!(r("|x|"), "|x|");
        assert_eq!(r("|x + y|"), "|x + y|");
    }

    #[test]
    fn abs_in_expression() {
        assert_eq!(r("|x| + |y|"), "|x| + |y|");
    }

    #[test]
    fn norm_with_vbar() {
        assert_eq!(r(r"\|v\|"), "‖v‖");
        assert_eq!(a(r"\|v\|"), "||v||");
    }

    // ── Floor / ceil / inner ─────────────────────────────────────────────────

    #[test]
    fn floor_unicode_and_ascii() {
        assert_eq!(r(r"\lfloor x \rfloor"), "⌊x⌋");
        assert_eq!(r(r"\lfloor x/2 \rfloor"), "⌊x ÷ 2⌋");
        assert_eq!(a(r"\lfloor x \rfloor"), "floor(x)");
    }

    #[test]
    fn ceil_basic() {
        assert_eq!(r(r"\lceil x \rceil"), "⌈x⌉");
        assert_eq!(a(r"\lceil x \rceil"), "ceil(x)");
    }

    #[test]
    fn inner_product() {
        assert_eq!(r(r"\langle x, y \rangle"), "⟨x, y⟩");
        assert_eq!(a(r"\langle x, y \rangle"), "<x, y>");
    }

    // ── Binom and primes ─────────────────────────────────────────────────────

    #[test]
    fn binomial_coefficient() {
        assert_eq!(r(r"\binom{n}{k}"), "C(n, k)");
        assert_eq!(r(r"\binom{n+1}{k}"), "C(n + 1, k)");
    }

    #[test]
    fn prime_notation() {
        assert_eq!(r("f'"), "f′");
        assert_eq!(r("f''"), "f″");
        assert_eq!(r("f'''"), "f‴");
        assert_eq!(a("f''"), "f''");
    }

    // ── Text / mathbb / font wrappers ────────────────────────────────────────

    #[test]
    fn text_inside_math_keeps_spacing() {
        assert_eq!(render_math_lines(r"$x \text{ if } y$"), "x if y");
    }

    #[test]
    fn mathbb_sets() {
        assert_eq!(r(r"x \in \mathbb{R}"), "x ∈ ℝ");
        assert_eq!(r(r"\mathbb{N}"), "ℕ");
        assert_eq!(a(r"x \in \mathbb{R}"), "x in R");
    }

    #[test]
    fn font_wrappers_unwrap() {
        assert_eq!(r(r"\mathbf{v}"), "v");
        assert_eq!(r(r"\mathit{x}"), "x");
        assert_eq!(r(r"\boldsymbol{\alpha}"), "α");
    }

    // ── Accents ──────────────────────────────────────────────────────────────

    #[test]
    fn accents_render_as_calls() {
        assert_eq!(r(r"\bar{x}"), "bar x");
        assert_eq!(r(r"\hat{\theta}"), "hat θ");
        assert_eq!(r(r"\vec{v}"), "vec v");
        assert_eq!(r(r"\dot{q}"), "dot q");
        assert_eq!(r(r"\tilde{x}"), "tilde x");
    }

    // ── New operators ────────────────────────────────────────────────────────

    #[test]
    fn new_binary_operators() {
        assert_eq!(r(r"a \equiv b"), "a ≡ b");
        assert_eq!(r(r"y \propto x"), "y ∝ x");
        assert_eq!(r(r"f \circ g"), "f ∘ g");
        assert_eq!(r(r"a \bmod n"), "a mod n");
    }

    #[test]
    fn modular_equivalence() {
        assert_eq!(r(r"a \equiv b \pmod{n}"), "a ≡ b (mod n)");
    }

    // ── Matrices ─────────────────────────────────────────────────────────────

    #[test]
    fn matrix_pmatrix() {
        assert_eq!(
            r(r"\begin{pmatrix} 1 & 2 \\ 3 & 4 \end{pmatrix}"),
            "(1, 2; 3, 4)"
        );
    }

    #[test]
    fn matrix_bmatrix() {
        assert_eq!(
            r(r"\begin{bmatrix} a & b \\ c & d \end{bmatrix}"),
            "[a, b; c, d]"
        );
    }

    #[test]
    fn matrix_vmatrix() {
        assert_eq!(
            r(r"\begin{vmatrix} a & b \\ c & d \end{vmatrix}"),
            "|a, b; c, d|"
        );
    }

    #[test]
    fn matrix_double_vmatrix() {
        assert_eq!(
            r(r"\begin{Vmatrix} v_1 & v_2 \end{Vmatrix}"),
            "‖v₁, v₂‖"
        );
        assert_eq!(
            a(r"\begin{Vmatrix} v_1 & v_2 \end{Vmatrix}"),
            "||v_1, v_2||"
        );
    }

    #[test]
    fn matrix_column_vector() {
        assert_eq!(
            r(r"\begin{pmatrix} x \\ y \\ z \end{pmatrix}"),
            "(x; y; z)"
        );
    }

    #[test]
    fn matrix_row_vector() {
        assert_eq!(
            r(r"\begin{pmatrix} 1 & 2 & 3 \end{pmatrix}"),
            "(1, 2, 3)"
        );
    }

    #[test]
    fn matrix_with_inner_math() {
        assert_eq!(
            r(r"\begin{pmatrix} \frac{1}{2} & \alpha^2 \\ \sqrt{x} & 0 \end{pmatrix}"),
            "(1 ÷ 2, α²; √x, 0)"
        );
    }

    // ── Set / order / divisibility / geometry / logic / algebra ops ──────────

    #[test]
    fn set_operators() {
        assert_eq!(r(r"A \subseteq B"), "A ⊆ B");
        assert_eq!(r(r"A \supseteq B"), "A ⊇ B");
        assert_eq!(r(r"A \setminus B"), "A ∖ B");
        assert_eq!(r(r"A \supset B"), "A ⊃ B");
        assert_eq!(a(r"A \subseteq B"), "A subseteq B");
    }

    #[test]
    fn order_operators() {
        assert_eq!(r(r"a \ll b"), "a ≪ b");
        assert_eq!(r(r"a \gg b"), "a ≫ b");
    }

    #[test]
    fn geometry_operators() {
        assert_eq!(r(r"u \perp v"), "u ⟂ v");
        assert_eq!(r(r"u \parallel v"), "u ∥ v");
    }

    #[test]
    fn divisibility_operators() {
        assert_eq!(r(r"p \mid n"), "p ∣ n");
        assert_eq!(r(r"p \nmid n"), "p ∤ n");
    }

    #[test]
    fn logic_operators() {
        assert_eq!(r(r"p \wedge q"), "p ∧ q");
        assert_eq!(r(r"p \vee q"), "p ∨ q");
        assert_eq!(r(r"\neg p"), "¬p");
        assert_eq!(a(r"\neg p"), "!p");
    }

    #[test]
    fn algebra_operators() {
        assert_eq!(r(r"a \oplus b"), "a ⊕ b");
        assert_eq!(r(r"a \otimes b"), "a ⊗ b");
        assert_eq!(r(r"a \odot b"), "a ⊙ b");
    }

    #[test]
    fn negated_relations() {
        assert_eq!(r(r"a \nleq b"), "a ≰ b");
        assert_eq!(r(r"a \ngeq b"), "a ≱ b");
        assert_eq!(r(r"x \notin S"), "x ∉ S");
        assert_eq!(r(r"A \nsubseteq B"), "A ⊈ B");
    }

    #[test]
    fn mod_alias() {
        assert_eq!(r(r"a \mod n"), "a mod n");
    }

    // ── New symbols ──────────────────────────────────────────────────────────

    #[test]
    fn physics_constants() {
        assert_eq!(r(r"\hbar \omega"), "ℏ ω");
        assert_eq!(r(r"\ell"), "ℓ");
    }

    #[test]
    fn complex_parts() {
        assert_eq!(r(r"\Re z + i \Im z"), "ℜ z + i ℑ z");
    }

    #[test]
    fn proof_symbols() {
        assert_eq!(r(r"\therefore"), "∴");
        assert_eq!(r(r"\because"), "∵");
    }

    #[test]
    fn logic_symbols() {
        assert_eq!(r(r"\top"), "⊤");
        assert_eq!(r(r"\bot"), "⊥");
        assert_eq!(r(r"\vdash"), "⊢");
        assert_eq!(r(r"\models"), "⊨");
    }

    // ── Delimiter size prefixes and phantom ──────────────────────────────────

    #[test]
    fn big_delimiters_dropped() {
        assert_eq!(r(r"\big(x\big)"), "(x)");
        assert_eq!(r(r"\Big[\frac{1}{2}\Big]"), "[1 ÷ 2]");
    }

    #[test]
    fn phantom_dropped() {
        assert_eq!(r(r"x \phantom{y} + z"), "x + z");
    }

    // ── Operator names ───────────────────────────────────────────────────────

    #[test]
    fn extended_operator_names() {
        assert_eq!(r(r"\gcd(a, b)"), "gcd(a, b)");
        assert_eq!(r(r"\dim V"), "dim V");
        assert_eq!(r(r"\ker f"), "ker f");
        assert_eq!(r(r"\sup S"), "sup S");
    }

    // ── Stacked relations ────────────────────────────────────────────────────

    #[test]
    fn overset_underset() {
        assert_eq!(r(r"\overset{?}{=}"), "=^?");
        assert_eq!(r(r"\underset{n \to \infty}{\lim}"), "(lim)_(n → ∞)");
        assert_eq!(r(r"\stackrel{!}{=}"), "=^!");
    }

    #[test]
    fn overbrace_passes_through_with_annotation() {
        assert_eq!(r(r"\overbrace{x + y}^{n}"), "(x + y)ⁿ");
    }

    // ── Substack ─────────────────────────────────────────────────────────────

    #[test]
    fn substack_renders_as_comma_list() {
        assert_eq!(
            r(r"\sum_{\substack{i, j \\ i \neq j}} a"),
            "∑[i, j, i ≠ j] a"
        );
    }

    // ── Cases environment ────────────────────────────────────────────────────

    #[test]
    fn cases_with_math_condition_inserts_if() {
        assert_eq!(
            r(r"\begin{cases} a & x > 0 \\ b & x \leq 0 \end{cases}"),
            "{a if x > 0; b if x ≤ 0}"
        );
    }

    #[test]
    fn cases_with_text_condition_keeps_wording() {
        assert_eq!(
            r(r"\begin{cases} a & \text{if } x > 0 \\ b & \text{otherwise} \end{cases}"),
            "{a if x > 0; b otherwise}"
        );
    }

    // ── Aligned / align / gather ─────────────────────────────────────────────

    #[test]
    fn aligned_environment() {
        assert_eq!(
            r(r"\begin{aligned} a &= b \\ c &= d \end{aligned}"),
            "a = b; c = d"
        );
    }

    #[test]
    fn gather_environment() {
        assert_eq!(
            r(r"\begin{gather} x + y = 1 \\ x - y = 0 \end{gather}"),
            "x + y = 1; x - y = 0"
        );
    }

    // ── Robustness ───────────────────────────────────────────────────────────

    #[test]
    fn unknown_command_falls_back_to_name() {
        assert_eq!(r(r"\foo + 1"), "foo + 1");
    }

    #[test]
    fn empty_math_region() {
        assert_eq!(r(""), "");
        assert_eq!(render_math_block("$$"), "");
    }

    #[test]
    fn strip_delimiters_variants() {
        assert_eq!(strip_delimiters("$x$"), "x");
        assert_eq!(strip_delimiters("$$x$$"), "x");
        assert_eq!(strip_delimiters(r"\(x\)"), "x");
        assert_eq!(strip_delimiters(r"\[x\]"), "x");
        assert_eq!(strip_delimiters("x"), "x");
    }

    // ── Big operators ────────────────────────────────────────────────────────

    #[test]
    fn bigcup_with_bounds() {
        assert_eq!(r(r"\bigcup_{i=1}^{n} A_i"), "⋃[i = 1..n] Aᵢ");
    }

    #[test]
    fn bigcap_no_upper() {
        assert_eq!(r(r"\bigcap_{k} B_k"), "⋂[k] Bₖ");
    }

    #[test]
    fn bigoplus_with_index() {
        assert_eq!(r(r"\bigoplus_{i} V_i"), "⨁[i] Vᵢ");
    }

    #[test]
    fn bigvee_bigwedge() {
        assert_eq!(r(r"\bigvee_{i} p_i"), "⋁[i] pᵢ");
        assert_eq!(r(r"\bigwedge_{i} q_i"), "⋀[i] qᵢ");
    }

    // ── Order / similarity relations ─────────────────────────────────────────

    #[test]
    fn preorder_relations() {
        assert_eq!(r(r"a \prec b"), "a ≺ b");
        assert_eq!(r(r"a \succ b"), "a ≻ b");
        assert_eq!(r(r"a \preceq b"), "a ⪯ b");
        assert_eq!(r(r"a \succeq b"), "a ⪰ b");
    }

    #[test]
    fn square_order_relations() {
        assert_eq!(r(r"a \sqsubseteq b"), "a ⊑ b");
        assert_eq!(r(r"a \sqsupseteq b"), "a ⊒ b");
    }

    #[test]
    fn similarity_relations() {
        assert_eq!(r(r"A \cong B"), "A ≅ B");
        assert_eq!(r(r"f \simeq g"), "f ≃ g");
        assert_eq!(r(r"a \asymp b"), "a ≍ b");
    }

    // ── Lattice / multiset ops ────────────────────────────────────────────────

    #[test]
    fn sqcup_sqcap_uplus() {
        assert_eq!(r(r"A \sqcup B"), "A ⊔ B");
        assert_eq!(r(r"A \sqcap B"), "A ⊓ B");
        assert_eq!(r(r"A \uplus B"), "A ⊎ B");
        assert_eq!(r(r"G \wr H"), "G ≀ H");
    }

    // ── liminf / limsup ──────────────────────────────────────────────────────

    #[test]
    fn liminf_limsup() {
        assert_eq!(r(r"\limsup x_n"), "limsup xₙ");
        assert_eq!(r(r"\liminf a_n"), "liminf aₙ");
    }

    // ── mathcal / mathfrak / mathscr ─────────────────────────────────────────

    #[test]
    fn mathcal_script() {
        assert_eq!(r(r"\mathcal{A}"), "𝒜");
        assert_eq!(r(r"\mathcal{F}"), "ℱ");
        assert_eq!(r(r"\mathcal{L}"), "ℒ");
        assert_eq!(r(r"\mathscr{H}"), "ℋ");
    }

    #[test]
    fn mathfrak_fraktur() {
        assert_eq!(r(r"\mathfrak{g}"), "𝔤");
        assert_eq!(r(r"\mathfrak{A}"), "𝔄");
        assert_eq!(r(r"\mathfrak{n}"), "𝔫");
    }

    // ── New environments ──────────────────────────────────────────────────────

    #[test]
    fn equation_environment() {
        assert_eq!(r(r"\begin{equation} E = mc^2 \end{equation}"), "E = m c²");
    }

    #[test]
    fn array_environment() {
        assert_eq!(
            r(r"\begin{array}{cc} a & b \\ c & d \end{array}"),
            "[a, b; c, d]"
        );
    }

    #[test]
    fn smallmatrix_environment() {
        assert_eq!(
            r(r"\begin{smallmatrix} 1 & 0 \\ 0 & 1 \end{smallmatrix}"),
            "[1, 0; 0, 1]"
        );
    }

    #[test]
    fn multline_environment() {
        assert_eq!(
            r(r"\begin{multline} a + b \\ = c \end{multline}"),
            "a + b; = c"
        );
    }

    // ── Arrows ───────────────────────────────────────────────────────────────

    #[test]
    fn vertical_arrows() {
        assert_eq!(r(r"\uparrow"), "↑");
        assert_eq!(r(r"\downarrow"), "↓");
        assert_eq!(r(r"\updownarrow"), "↕");
        assert_eq!(r(r"\Uparrow"), "⇑");
        assert_eq!(r(r"\Downarrow"), "⇓");
    }

    #[test]
    fn long_arrows() {
        assert_eq!(r(r"\longrightarrow"), "⟶");
        assert_eq!(r(r"\longleftarrow"), "⟵");
        assert_eq!(r(r"\longleftrightarrow"), "⟷");
        assert_eq!(r(r"\longmapsto"), "⟼");
        assert_eq!(r(r"\Longrightarrow"), "⟹");
    }

    #[test]
    fn harpoon_arrows() {
        assert_eq!(r(r"\hookrightarrow"), "↪");
        assert_eq!(r(r"\hookleftarrow"), "↩");
        assert_eq!(r(r"\rightharpoonup"), "⇀");
        assert_eq!(r(r"\leftharpoonup"), "↼");
        assert_eq!(r(r"\rightleftharpoons"), "⇌");
    }

    #[test]
    fn diagonal_arrows() {
        assert_eq!(r(r"\nearrow"), "↗");
        assert_eq!(r(r"\searrow"), "↘");
        assert_eq!(r(r"\nwarrow"), "↖");
        assert_eq!(r(r"\swarrow"), "↙");
    }

    // ── Small extras ─────────────────────────────────────────────────────────

    #[test]
    fn small_symbol_extras() {
        assert_eq!(r(r"\nexists"), "∄");
        assert_eq!(r(r"\flat"), "♭");
        assert_eq!(r(r"\sharp"), "♯");
        assert_eq!(r(r"\natural"), "♮");
        assert_eq!(r(r"\heartsuit"), "♥");
        assert_eq!(r(r"\spadesuit"), "♠");
        assert_eq!(r(r"\clubsuit"), "♣");
        assert_eq!(r(r"\diamondsuit"), "♦");
    }

    #[test]
    fn angle_symbols() {
        assert_eq!(r(r"\sphericalangle"), "∢");
        assert_eq!(r(r"\measuredangle"), "∡");
    }

    // ── ASCII mode for new features ───────────────────────────────────────────

    #[test]
    fn ascii_big_ops() {
        assert_eq!(a(r"\bigcup_{i} A_i"), "Union[i] A_i");
        assert_eq!(a(r"\bigcap_{i} B_i"), "Inter[i] B_i");
    }

    #[test]
    fn ascii_new_relations() {
        assert_eq!(a(r"a \prec b"), "a prec b");
        assert_eq!(a(r"A \cong B"), "A cong B");
        assert_eq!(a(r"A \sqcup B"), "A sqcup B");
    }

    #[test]
    fn ascii_arrows() {
        assert_eq!(a(r"\longrightarrow"), "->");
        assert_eq!(a(r"\longleftarrow"), "<-");
        assert_eq!(a(r"\Longrightarrow"), "=>");
        assert_eq!(a(r"\uparrow"), "^");
    }
}
