use super::{DocItem, DocKind, DocLanguage, DocMeta, DocTag, TagKind};
use std::path::Path;

// ── Content predicate ─────────────────────────────────────────────────────────

pub(crate) fn item_has_content(item: &DocItem) -> bool {
    !item.brief.is_empty() || !item.body.is_empty() || !item.tags.is_empty()
}

// ── Item construction ─────────────────────────────────────────────────────────

pub(crate) fn build_item(
    raw_lines: Vec<String>,
    name: String,
    kind: DocKind,
    file: &Path,
    line: usize,
    lang: DocLanguage,
    signature: String,
) -> DocItem {
    let mut prose: Vec<String> = Vec::new();
    let mut tags: Vec<DocTag> = Vec::new();
    let mut cur_tag: Option<(TagKind, Option<String>, Vec<String>)> = None;

    for raw in &raw_lines {
        if let Some(tag) = parse_tag_start(raw) {
            if let Some((k, n, tl)) = cur_tag.take() {
                tags.push(DocTag {
                    kind: k,
                    name: n,
                    text: tl.join(" ").trim().to_string(),
                });
            }
            cur_tag = Some(tag);
        } else if cur_tag
            .as_ref()
            .map(|(kind, _, _)| *kind == TagKind::Brief && raw.trim().is_empty())
            .unwrap_or(false)
        {
            if let Some((k, n, tl)) = cur_tag.take() {
                tags.push(DocTag {
                    kind: k,
                    name: n,
                    text: tl.join(" ").trim().to_string(),
                });
            }
            prose.push(raw.clone());
        } else if let Some((_, _, ref mut tl)) = cur_tag {
            tl.push(raw.clone());
        } else {
            prose.push(raw.clone());
        }
    }
    if let Some((k, n, tl)) = cur_tag {
        tags.push(DocTag {
            kind: k,
            name: n,
            text: tl.join(" ").trim().to_string(),
        });
    }

    let explicit_brief = tags
        .iter()
        .find(|t| t.kind == TagKind::Brief)
        .map(|t| t.text.clone());
    let first_prose = prose.iter().position(|l| !l.trim().is_empty());

    let (brief, body) = match (explicit_brief, first_prose) {
        (Some(b), _) => {
            let body = prose
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join("\n")
                .trim()
                .to_string();
            (b, body)
        }
        (None, Some(idx)) => {
            let brief = prose[idx].trim().to_string();
            let body = prose[idx + 1..].join("\n").trim().to_string();
            (brief, body)
        }
        (None, None) => (String::new(), String::new()),
    };

    DocItem {
        name,
        kind,
        brief,
        body,
        tags,
        file: file.to_path_buf(),
        line,
        lang,
        signature: signature.trim().to_string(),
        meta: DocMeta::default(),
    }
}

// ── Tag parsing ───────────────────────────────────────────────────────────────

pub(crate) fn parse_tag_start(line: &str) -> Option<(TagKind, Option<String>, Vec<String>)> {
    let t = line.trim_start();
    // Doxygen tags use either `@tag` or `\tag`.
    let rest = if t.starts_with('@') || t.starts_with('\\') {
        &t[1..]
    } else {
        return None;
    };

    let (tag, rem) = match rest.find(char::is_whitespace) {
        Some(i) => (&rest[..i], rest[i..].trim()),
        None => (rest, ""),
    };
    let rem = rem.to_string();

    // Reject single-char escapes (\n, \t, \0, …) and tags that don't start with a letter.
    // Allow brackets/commas so Doxygen's @param[in], @param[out], @param[in,out] pass through.
    if tag.len() < 2
        || !tag
            .chars()
            .next()
            .map(|c| c.is_ascii_alphabetic())
            .unwrap_or(false)
    {
        return None;
    }

    match tag {
        "brief" => Some((TagKind::Brief, None, vec![rem])),
        "param" | "param[in]" | "param[out]" | "param[in,out]" => {
            let (pname, pdesc) = split_first_word(&rem);
            Some((TagKind::Param, Some(pname), vec![pdesc]))
        }
        // Template parameter — same structure as @param but kept distinguishable.
        "tparam" => {
            let (pname, pdesc) = split_first_word(&rem);
            Some((
                TagKind::Other("tparam".to_string()),
                Some(pname),
                vec![pdesc],
            ))
        }
        "return" | "returns" => Some((TagKind::Return, None, vec![rem])),
        // @retval <value> <desc> — value name embedded in label like @throws.
        "retval" => {
            let (val, desc) = split_first_word(&rem);
            let label = if val.is_empty() {
                "retval".into()
            } else {
                format!("retval {val}")
            };
            Some((TagKind::Other(label), None, vec![desc]))
        }
        "note" => Some((TagKind::Note, None, vec![rem])),
        "see" | "sa" => Some((TagKind::See, None, vec![rem])),
        "since" => Some((TagKind::Since, None, vec![rem])),
        "deprecated" => Some((TagKind::Deprecated, None, vec![rem])),
        "example" | "code" | "endcode" => Some((TagKind::Example, None, vec![rem])),
        "warning" | "warn" => Some((TagKind::Warning, None, vec![rem])),
        // @throw (singular) is identical to @throws / @exception.
        "throw" | "throws" | "exception" => {
            let (exc, desc) = split_first_word(&rem);
            let label = if exc.is_empty() {
                "throws".into()
            } else {
                format!("throws {exc}")
            };
            Some((TagKind::Other(label), None, vec![desc]))
        }
        other => Some((TagKind::Other(other.to_string()), None, vec![rem])),
    }
}

// ── Comment block collectors ──────────────────────────────────────────────────

pub(crate) fn collect_c_block(lines: &[&str], start: usize) -> (Vec<String>, usize) {
    let mut out = Vec::new();
    let first = lines[start].trim();
    let after = first[3..].trim();

    if let Some(content) = after.strip_suffix("*/") {
        out.push(content.trim().to_string());
        return (out, start);
    }
    if !after.is_empty() {
        out.push(after.to_string());
    }

    let mut i = start + 1;
    while i < lines.len() {
        let t = lines[i].trim();
        if t.ends_with("*/") {
            let content = t
                .strip_suffix("*/")
                .unwrap_or("")
                .trim_start_matches('*')
                .trim();
            if !content.is_empty() {
                out.push(content.to_string());
            }
            return (out, i);
        }
        let line = if let Some(r) = t.strip_prefix("* ") {
            r.to_string()
        } else if t == "*" {
            String::new()
        } else {
            t.strip_prefix('*').unwrap_or(t).to_string()
        };
        out.push(line);
        i += 1;
    }
    (out, i.saturating_sub(1))
}

pub(crate) fn collect_line_block(
    lines: &[&str],
    start: usize,
    prefix: &str,
) -> (Vec<String>, usize) {
    let mut out = Vec::new();
    let mut i = start;
    let double = format!("{prefix}/");
    while i < lines.len() {
        let t = lines[i].trim();
        if t.starts_with(prefix) && !t.starts_with(&double) {
            out.push(t[prefix.len()..].trim_start().to_string());
            i += 1;
        } else {
            break;
        }
    }
    (out, i.saturating_sub(1))
}

// ── Line navigation ───────────────────────────────────────────────────────────

pub(crate) fn next_non_blank<'a>(lines: &[&'a str], from: usize) -> &'a str {
    lines
        .iter()
        .skip(from)
        .find(|l| !l.trim().is_empty())
        .copied()
        .unwrap_or("")
}

pub(crate) fn next_decl_sym<'a>(lines: &[&'a str], from: usize) -> &'a str {
    let mut i = from;
    loop {
        let Some(&l) = lines.get(i) else { return "" };
        let t = l.trim();
        if t.is_empty() {
            i += 1;
            continue;
        }
        if t.starts_with("template") {
            i += 1;
            continue;
        }
        return t;
    }
}

// ── Identifier helpers ────────────────────────────────────────────────────────

pub(crate) fn first_ident(s: &str) -> String {
    s.split(|c: char| !c.is_alphanumeric() && c != '_')
        .next()
        .unwrap_or("")
        .to_string()
}

pub(crate) fn func_name_before_paren(t: &str) -> Option<String> {
    let paren = t.find('(')?;
    let before = t[..paren].trim_end();
    let word = before.split_whitespace().last()?;
    let name = word.trim_start_matches('*').trim_start_matches('~');
    if name.is_empty() {
        return None;
    }
    if matches!(name, "if" | "while" | "for" | "switch" | "do" | "catch") {
        return None;
    }
    if name
        .chars()
        .next()
        .map(|c| c.is_alphabetic() || c == '_')
        .unwrap_or(false)
    {
        Some(name.to_string())
    } else {
        None
    }
}

/// Case-insensitive prefix skip for ASCII-only languages (Fortran, Ada).
pub(crate) fn ci_ident_after(s: &str, prefix_lower: &str) -> String {
    let len = prefix_lower.len();
    if s.len() >= len && s[..len].eq_ignore_ascii_case(prefix_lower) {
        first_ident(s[len..].trim_start())
    } else {
        String::new()
    }
}

// ── Private ───────────────────────────────────────────────────────────────────

fn split_first_word(s: &str) -> (String, String) {
    match s.find(char::is_whitespace) {
        Some(i) => (s[..i].to_string(), s[i..].trim().to_string()),
        None => (s.to_string(), String::new()),
    }
}
