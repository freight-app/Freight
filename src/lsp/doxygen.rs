//! Extract a C/C++ header's top-of-file Doxygen banner.
//!
//! Many headers open with a file-level documentation block:
//! ```c
//! /**
//!  * @file vec2.h
//!  * @brief 2D vector maths.
//!  * @author Jane Doe
//!  * @date 2024
//!  */
//! ```
//! [`extract_file_doc`] finds that block (skipping `#pragma once`, include
//! guards, and a leading licence comment that carries no Doxygen commands) and
//! returns its `@brief`, free description, and the remaining `@tag` lines so the
//! LSP can show them in the `#include` hover and the completion doc panel.

/// A parsed file-level documentation banner.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FileDoc {
    pub brief: Option<String>,
    pub description: String,
    /// Ordered `(tag, value)` pairs for `@author`, `@date`, `@version`, … with
    /// the leading `@`/`\` stripped and the name lowercased.
    pub tags: Vec<(String, String)>,
}

impl FileDoc {
    fn is_empty(&self) -> bool {
        self.brief.is_none() && self.description.is_empty() && self.tags.is_empty()
    }

    /// The brief line: the explicit `@brief`, else the first sentence of the
    /// description (common for tag-less prose banners).
    pub fn brief_line(&self) -> Option<String> {
        if let Some(b) = &self.brief {
            return Some(b.clone());
        }
        let d = self.description.trim();
        if d.is_empty() {
            return None;
        }
        let end = d.find(". ").map(|i| i + 1).unwrap_or(d.len());
        Some(d[..end].trim().to_string())
    }

    /// `Author Name (contact)` when an `@author` is present — the contact is the
    /// `<…>` / `(…)` embedded in the author, or a separate `@contact`/`@email`.
    pub fn author_line(&self) -> Option<String> {
        let tag = |names: &[&str]| {
            self.tags
                .iter()
                .find(|(k, _)| names.contains(&k.as_str()))
                .map(|(_, v)| v.clone())
        };
        let author = tag(&["author", "authors", "maintainer"])?;
        let (name, embedded) = split_contact(&author);
        let contact = embedded.or_else(|| tag(&["contact", "email"]));
        Some(match contact {
            Some(c) => format!("{name} ({c})"),
            None => name,
        })
    }
}

/// Split `"Name <email>"` / `"Name (email)"` into `(name, Some(contact))`.
fn split_contact(s: &str) -> (String, Option<String>) {
    for (open, close) in [('<', '>'), ('(', ')')] {
        if let (Some(o), Some(c)) = (s.find(open), s.rfind(close)) {
            if o < c {
                let contact = s[o + 1..c].trim();
                let name = s[..o].trim();
                if !contact.is_empty() {
                    let name = if name.is_empty() { s.trim() } else { name };
                    return (name.to_string(), Some(contact.to_string()));
                }
            }
        }
    }
    (s.trim().to_string(), None)
}

/// Tags worth surfacing (others are ignored to keep the panel focused).
const KNOWN_TAGS: &[&str] = &[
    "author", "authors", "date", "version", "copyright", "since", "note",
    "warning", "see", "deprecated", "bug", "todo", "license", "maintainer",
    "contact", "email",
];

/// Extract the file-level Doxygen banner from `source`, or `None` when the head
/// of the file carries no documentation.
///
/// A `/**`, `/*!`, `///`, or `//!` doc-comment at the top counts as a banner
/// even with no `@` commands (its prose is the description). A plain `/* */` /
/// `//` comment only counts when it carries a Doxygen command — so a licence
/// header is not mistaken for documentation.
pub fn extract_file_doc(source: &str) -> Option<FileDoc> {
    let (_, text) = leading_comment_blocks(source, 200)
        .into_iter()
        .find(|(is_doc, text)| *is_doc || looks_like_banner(text))?;
    let doc = parse_block(&text);
    if doc.is_empty() {
        None
    } else {
        Some(doc)
    }
}

/// Render a [`FileDoc`] as Markdown for an LSP hover / completion panel.
pub fn render_markdown(doc: &FileDoc) -> String {
    let mut out = String::new();
    if let Some(brief) = &doc.brief {
        out.push_str(brief);
        out.push_str("\n\n");
    }
    if !doc.description.is_empty() {
        out.push_str(&doc.description);
        out.push_str("\n\n");
    }
    for (name, value) in &doc.tags {
        out.push_str(&format!("*{}* — {}\n\n", title_case(name), value));
    }
    out.trim_end().to_string()
}

/// A header's resolved file-banner Markdown, ready to append to a hover, or
/// `None` if the file can't be read or has no banner.
pub fn file_doc_markdown(path: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let doc = extract_file_doc(&text)?;
    let md = render_markdown(&doc);
    if md.is_empty() {
        None
    } else {
        Some(md)
    }
}

/// Parse a file's top-of-file Doxygen banner, reading it from disk.
pub fn file_doc(path: &std::path::Path) -> Option<FileDoc> {
    let text = std::fs::read_to_string(path).ok()?;
    extract_file_doc(&text)
}

// ── internals ────────────────────────────────────────────────────────────────

/// Collect the comment blocks at the head of the file (up to `max_lines`) as
/// `(is_doc_comment, cleaned_text)`, stopping at the first line of real code.
/// `is_doc_comment` marks `/**`, `/*!`, `///`, `//!` blocks. Preprocessor lines
/// (`#pragma once`, include guards) are skipped without ending the scan.
fn leading_comment_blocks(source: &str, max_lines: usize) -> Vec<(bool, String)> {
    let mut blocks: Vec<(bool, String)> = Vec::new();
    let mut cur = String::new();
    let mut cur_is_doc = false;
    let mut in_block = false;
    let mut in_line_run = false;

    let push = |cur: &mut String, is_doc: bool, blocks: &mut Vec<(bool, String)>| {
        if !cur.trim().is_empty() {
            blocks.push((is_doc, std::mem::take(cur)));
        } else {
            cur.clear();
        }
    };

    for (i, raw) in source.lines().enumerate() {
        if i >= max_lines {
            break;
        }
        let line = raw.trim();

        if in_block {
            if let Some(end) = line.find("*/") {
                append_block_line(&mut cur, &line[..end]);
                in_block = false;
                push(&mut cur, cur_is_doc, &mut blocks);
            } else {
                append_block_line(&mut cur, line);
            }
            continue;
        }

        if line.is_empty() {
            if in_line_run {
                push(&mut cur, cur_is_doc, &mut blocks);
                in_line_run = false;
            }
            continue;
        }

        if let Some(rest) = line
            .strip_prefix("///")
            .or_else(|| line.strip_prefix("//!"))
        {
            if !in_line_run {
                cur_is_doc = true;
            }
            in_line_run = true;
            cur.push_str(rest.trim_start());
            cur.push('\n');
            continue;
        }
        if in_line_run {
            push(&mut cur, cur_is_doc, &mut blocks);
            in_line_run = false;
        }

        if let Some(rest) = line.strip_prefix("/*") {
            // A doc comment opens with `/**` or `/*!`; plain `/*` is not.
            cur_is_doc = rest.starts_with('*') || rest.starts_with('!');
            let rest = rest
                .strip_prefix('*')
                .or_else(|| rest.strip_prefix('!'))
                .unwrap_or(rest);
            if let Some(end) = rest.find("*/") {
                append_block_line(&mut cur, &rest[..end]);
                push(&mut cur, cur_is_doc, &mut blocks);
            } else {
                append_block_line(&mut cur, rest);
                in_block = true;
            }
            continue;
        }

        // `//` plain comment line — skip (not a doc comment), don't stop.
        if line.starts_with("//") {
            continue;
        }
        // Preprocessor / pragma lines may precede the banner.
        if line.starts_with('#') {
            continue;
        }
        // Anything else is code — stop scanning the head.
        break;
    }
    push(&mut cur, cur_is_doc, &mut blocks);
    blocks
}

/// Append a block-comment continuation line, stripping a leading ` * ` margin.
fn append_block_line(cur: &mut String, line: &str) {
    let l = line.trim_start();
    let l = l.strip_prefix('*').map(str::trim_start).unwrap_or(l);
    cur.push_str(l);
    cur.push('\n');
}

/// Whether a comment block carries a Doxygen command — distinguishes a real
/// banner from a licence header or prose comment.
fn looks_like_banner(block: &str) -> bool {
    block.lines().any(|l| {
        let t = l.trim_start();
        let rest = t.strip_prefix('@').or_else(|| t.strip_prefix('\\'));
        matches!(rest, Some(r) if r.chars().next().is_some_and(|c| c.is_ascii_alphabetic()))
    })
}

/// Parse a cleaned comment block into a [`FileDoc`].
fn parse_block(text: &str) -> FileDoc {
    let mut doc = FileDoc::default();
    let mut cur_tag: Option<String> = None;
    let mut cur_val = String::new();

    let flush = |doc: &mut FileDoc, tag: &mut Option<String>, val: &mut String| {
        let Some(name) = tag.take() else {
            return;
        };
        let value = val.trim().to_string();
        *val = String::new();
        match name.as_str() {
            "brief" | "short" => {
                if !value.is_empty() {
                    doc.brief = Some(value);
                }
            }
            "details" | "par" | "description" => {
                if !value.is_empty() {
                    if !doc.description.is_empty() {
                        doc.description.push_str("\n\n");
                    }
                    doc.description.push_str(&value);
                }
            }
            "file" => {} // marks the banner; the name adds nothing to the panel
            other if KNOWN_TAGS.contains(&other) && !value.is_empty() => {
                doc.tags.push((other.to_string(), value));
            }
            _ => {}
        }
    };

    for line in text.lines() {
        let l = line.trim();
        let tagged = l
            .strip_prefix('@')
            .or_else(|| l.strip_prefix('\\'))
            .filter(|r| r.chars().next().is_some_and(|c| c.is_ascii_alphabetic()));
        if let Some(rest) = tagged {
            flush(&mut doc, &mut cur_tag, &mut cur_val);
            let mut parts = rest.splitn(2, char::is_whitespace);
            let name = parts.next().unwrap_or("").to_ascii_lowercase();
            cur_val = parts.next().unwrap_or("").trim().to_string();
            cur_tag = Some(name);
        } else if cur_tag.is_some() {
            if l.is_empty() {
                flush(&mut doc, &mut cur_tag, &mut cur_val);
            } else {
                if !cur_val.is_empty() {
                    cur_val.push(' ');
                }
                cur_val.push_str(l);
            }
        } else if !l.is_empty() {
            if !doc.description.is_empty() {
                doc.description.push(' ');
            }
            doc.description.push_str(l);
        }
    }
    flush(&mut doc, &mut cur_tag, &mut cur_val);
    doc
}

fn title_case(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_ascii_uppercase().to_string() + c.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_block_banner() {
        let src = "\
/**
 * @file vec2.h
 * @brief 2D vector maths.
 * @author Jane Doe
 * @date 2024
 */
#pragma once
struct Vec2 { double x, y; };
";
        let doc = extract_file_doc(src).expect("banner");
        assert_eq!(doc.brief.as_deref(), Some("2D vector maths."));
        assert!(doc.tags.contains(&("author".into(), "Jane Doe".into())));
        assert!(doc.tags.contains(&("date".into(), "2024".into())));
    }

    #[test]
    fn line_comment_banner_after_pragma() {
        let src = "\
#pragma once
/// @brief Fast hashing.
/// @author A. Coder
/// @version 1.2
int hash(const char*);
";
        let doc = extract_file_doc(src).expect("banner");
        assert_eq!(doc.brief.as_deref(), Some("Fast hashing."));
        assert!(doc.tags.contains(&("version".into(), "1.2".into())));
    }

    #[test]
    fn licence_block_without_tags_is_skipped() {
        // A leading licence comment (no Doxygen commands) must not be mistaken
        // for the banner; the real banner follows it.
        let src = "\
/*
 * Copyright 2024 Example Corp. All rights reserved.
 * Licensed under the MIT License.
 */
/**
 * @brief The real banner.
 */
#pragma once
";
        let doc = extract_file_doc(src).expect("banner");
        assert_eq!(doc.brief.as_deref(), Some("The real banner."));
    }

    #[test]
    fn multiline_brief_and_description() {
        let src = "\
/**
 * @brief A short summary that
 *        wraps across two lines.
 *
 * A longer description paragraph
 * explaining the file.
 * @author Someone
 */
";
        let doc = extract_file_doc(src).expect("banner");
        assert_eq!(
            doc.brief.as_deref(),
            Some("A short summary that wraps across two lines.")
        );
        assert!(doc.description.contains("longer description"));
    }

    #[test]
    fn prose_only_doc_comment_is_a_banner() {
        // A `/**` doc comment with no @tags is still a banner (its prose is the
        // description) — this is the most common header-banner style.
        let src = "\
/**
 * Small numeric helpers for the widget library.
 */
#pragma once
int f();
";
        let doc = extract_file_doc(src).expect("prose banner");
        assert!(doc.description.contains("Small numeric helpers"));
    }

    #[test]
    fn line_doc_prose_is_a_banner() {
        let doc = extract_file_doc("/// A tiny string utility.\nint f();\n")
            .expect("line-doc prose banner");
        assert!(doc.description.contains("tiny string utility"));
    }

    #[test]
    fn no_banner_returns_none() {
        assert!(extract_file_doc("#pragma once\nint f();\n").is_none());
        // A plain (non-doc) comment with no Doxygen command is not a banner.
        assert!(extract_file_doc("// just a note\nint f();\n").is_none());
        assert!(extract_file_doc("/* plain license-ish text */\nint f();\n").is_none());
    }

    #[test]
    fn brief_and_author_lines() {
        let doc = extract_file_doc(
            "/**\n * @brief Does things.\n * @author Jane Doe <jane@x.com>\n */\nint f();\n",
        )
        .unwrap();
        assert_eq!(doc.brief_line().as_deref(), Some("Does things."));
        assert_eq!(doc.author_line().as_deref(), Some("Jane Doe (jane@x.com)"));

        // Separate @contact tag.
        let doc2 = extract_file_doc(
            "/**\n * @author Bob\n * @contact bob@x.com\n */\nint f();\n",
        )
        .unwrap();
        assert_eq!(doc2.author_line().as_deref(), Some("Bob (bob@x.com)"));

        // Prose-only: brief falls back to the first sentence.
        let doc3 = extract_file_doc("/// Tiny util. More detail here.\nint f();\n").unwrap();
        assert_eq!(doc3.brief_line().as_deref(), Some("Tiny util."));
        assert!(doc3.author_line().is_none());
    }

    #[test]
    fn renders_markdown() {
        let doc = FileDoc {
            brief: Some("Brief here.".into()),
            description: "Details.".into(),
            tags: vec![("author".into(), "X".into())],
        };
        let md = render_markdown(&doc);
        assert!(md.starts_with("Brief here.\n\n"));
        assert!(md.contains("Details."));
        assert!(md.contains("*Author* — X"));
    }
}
