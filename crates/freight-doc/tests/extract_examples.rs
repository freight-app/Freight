/// Integration tests: extract documentation from fixture source files and
/// from the existing `examples/doc-example` project.
///
/// Each test scans a real source file (or directory) and asserts that specific
/// named symbols are found with correct kinds and non-empty doc content.
use std::path::Path;

use freight_doc::extract::{extract_dir, extract_file, DocKind};

// ── helpers ──────────────────────────────────────────────────────────────────

fn fixture(rel: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(rel)
}

fn doc_example(rel: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/doc-example").join(rel)
}

fn has_item(items: &[freight_doc::extract::DocItem], name: &str) -> bool {
    items.iter().any(|i| i.name == name)
}

fn find_item<'a>(
    items: &'a [freight_doc::extract::DocItem],
    name: &str,
) -> &'a freight_doc::extract::DocItem {
    items.iter().find(|i| i.name == name)
        .unwrap_or_else(|| panic!("item '{name}' not found; got: {:?}",
            items.iter().map(|i| &i.name).collect::<Vec<_>>()))
}

// ── C fixture: buffer.h ───────────────────────────────────────────────────────

#[test]
fn c_buffer_finds_typedef() {
    let items = extract_file(&fixture("c/buffer.h"));
    let item = find_item(&items, "Buffer");
    assert!(matches!(item.kind, DocKind::Typedef | DocKind::Struct),
        "Buffer should be Typedef or Struct, got {:?}", item.kind);
    assert!(!item.brief.is_empty(), "Buffer needs a brief");
}

#[test]
fn c_buffer_finds_all_functions() {
    let items = extract_file(&fixture("c/buffer.h"));
    for name in &["buffer_init", "buffer_push", "buffer_clear", "buffer_remaining"] {
        let item = find_item(&items, name);
        assert!(matches!(item.kind, DocKind::Function),
            "{name} should be Function, got {:?}", item.kind);
        assert!(!item.brief.is_empty(), "{name} needs a brief");
    }
}

#[test]
fn c_buffer_push_has_params() {
    let items = extract_file(&fixture("c/buffer.h"));
    let push = find_item(&items, "buffer_push");
    use freight_doc::extract::TagKind;
    let params: Vec<_> = push.tags.iter().filter(|t| t.kind == TagKind::Param).collect();
    assert!(params.len() >= 3, "buffer_push should have ≥3 @param tags");
}

#[test]
fn c_buffer_finds_macro() {
    let items = extract_file(&fixture("c/buffer.h"));
    assert!(has_item(&items, "BUFFER_CAP"), "BUFFER_CAP macro should be found");
    let item = find_item(&items, "BUFFER_CAP");
    assert!(matches!(item.kind, DocKind::Macro));
}

// ── C++ fixture: geometry.hpp ─────────────────────────────────────────────────

#[test]
fn cpp_geometry_finds_namespace_items() {
    let items = extract_file(&fixture("cpp/geometry.hpp"));
    // Free functions must be qualified with the namespace.
    let tri = find_item(&items, "geometry::triangle_area");
    assert!(matches!(tri.kind, DocKind::Function));
    assert!(!tri.brief.is_empty());

    let ov = find_item(&items, "geometry::aabb_overlaps");
    assert!(matches!(ov.kind, DocKind::Function));
}

#[test]
fn cpp_geometry_finds_class_and_struct() {
    let items = extract_file(&fixture("cpp/geometry.hpp"));
    let point = find_item(&items, "geometry::Point");
    assert!(matches!(point.kind, DocKind::Struct | DocKind::Class));

    let aabb = find_item(&items, "geometry::AABB");
    assert!(matches!(aabb.kind, DocKind::Struct | DocKind::Class));
}

#[test]
fn cpp_geometry_total_item_count() {
    let items = extract_file(&fixture("cpp/geometry.hpp"));
    // At minimum: Point, AABB, triangle_area, aabb_overlaps (+ possible methods).
    assert!(items.len() >= 4, "expected ≥4 items, got {}", items.len());
}

// ── Fortran fixture: vectors.f90 ──────────────────────────────────────────────

#[test]
fn fortran_vectors_module_variables() {
    let items = extract_file(&fixture("fortran/vectors.f90"));
    let wp = find_item(&items, "wp");
    assert!(matches!(wp.kind, DocKind::Variable));
    assert!(!wp.brief.is_empty());

    let max_dim = find_item(&items, "max_dim");
    assert!(matches!(max_dim.kind, DocKind::Variable));

    let tol = find_item(&items, "norm_tol");
    assert!(matches!(tol.kind, DocKind::Variable));
}

#[test]
fn fortran_vectors_procedures() {
    let items = extract_file(&fixture("fortran/vectors.f90"));
    for name in &["dot3", "cross3", "norm3"] {
        let item = find_item(&items, name);
        assert!(matches!(item.kind, DocKind::Function),
            "{name} should be Function");
    }
    let norm = find_item(&items, "normalise");
    assert!(matches!(norm.kind, DocKind::Subroutine));
}

#[test]
fn fortran_vectors_total_item_count() {
    let items = extract_file(&fixture("fortran/vectors.f90"));
    assert!(items.len() >= 7, "expected ≥7 items (3 vars + 4 procs), got {}", items.len());
}

// ── Rust fixture: strings.rs ──────────────────────────────────────────────────

#[test]
fn rust_strings_finds_functions() {
    let items = extract_file(&fixture("rust/strings.rs"));
    for name in &["char_count", "truncate", "repeat_str", "centre"] {
        let item = find_item(&items, name);
        assert!(matches!(item.kind, DocKind::Function),
            "{name} should be Function");
        assert!(!item.brief.is_empty(), "{name} needs a brief");
    }
}

#[test]
fn rust_strings_finds_struct_and_methods() {
    let items = extract_file(&fixture("rust/strings.rs"));
    let builder = find_item(&items, "ListBuilder");
    assert!(matches!(builder.kind, DocKind::Struct));

    // impl methods: new, push, finish
    for name in &["new", "push", "finish"] {
        assert!(has_item(&items, name), "method '{name}' not found");
    }
}

#[test]
fn rust_char_count_has_example_in_body() {
    let items = extract_file(&fixture("rust/strings.rs"));
    let item = find_item(&items, "char_count");
    // The ```…``` block lands in the body.
    assert!(item.body.contains("assert_eq") || item.tags.iter().any(|t| {
        use freight_doc::extract::TagKind;
        t.kind == TagKind::Example && t.text.contains("assert_eq")
    }), "char_count should capture the example block");
}

// ── Ada fixture: matrix.ads ───────────────────────────────────────────────────

#[test]
fn ada_matrix_finds_functions() {
    let items = extract_file(&fixture("ada/matrix.ads"));
    for name in &["Mul", "Add", "Det", "Transpose"] {
        let item = find_item(&items, name);
        assert!(matches!(item.kind, DocKind::Subroutine | DocKind::Function),
            "{name} should be Subroutine or Function");
        assert!(!item.brief.is_empty(), "{name} needs a brief");
    }
}

#[test]
fn ada_matrix_item_count() {
    let items = extract_file(&fixture("ada/matrix.ads"));
    assert!(items.len() >= 4, "expected ≥4 items, got {}", items.len());
}

// ── D fixture: parser.d ───────────────────────────────────────────────────────

#[test]
fn d_parser_finds_enum_and_struct() {
    let items = extract_file(&fixture("d/parser.d"));
    let tok_kind = find_item(&items, "TokenKind");
    assert!(matches!(tok_kind.kind, DocKind::Enum));

    let token = find_item(&items, "Token");
    assert!(matches!(token.kind, DocKind::Struct));
}

#[test]
fn d_parser_finds_functions() {
    let items = extract_file(&fixture("d/parser.d"));
    let tok = find_item(&items, "tokenise");
    assert!(matches!(tok.kind, DocKind::Function));

    let eval = find_item(&items, "evaluate");
    assert!(matches!(eval.kind, DocKind::Function));
    assert!(!eval.brief.is_empty());
}

// ── doc-example: stats (C++) ──────────────────────────────────────────────────

#[test]
fn doc_example_stats_finds_namespace_functions() {
    let set = extract_dir(&doc_example("libs/stats/src"));
    let items = &set.items;
    assert!(!items.is_empty(), "stats lib should yield items");
    for name in &["stats::mean", "stats::variance", "stats::stddev", "stats::pearson"] {
        let item = find_item(items, name);
        assert!(matches!(item.kind, DocKind::Function),
            "{name} should be Function");
        assert!(!item.brief.is_empty(), "{name} needs a brief");
    }
}

#[test]
fn doc_example_stats_finds_class() {
    let set = extract_dir(&doc_example("libs/stats/src"));
    let cls = find_item(&set.items, "stats::OrderStatistics");
    assert!(matches!(cls.kind, DocKind::Class | DocKind::Struct));
    assert!(!cls.brief.is_empty());
}

#[test]
fn doc_example_stats_mean_has_param_and_return() {
    let set = extract_dir(&doc_example("libs/stats/src"));
    let mean = find_item(&set.items, "stats::mean");
    use freight_doc::extract::TagKind;
    assert!(mean.tags.iter().any(|t| t.kind == TagKind::Param), "mean needs @param");
    assert!(mean.tags.iter().any(|t| t.kind == TagKind::Return), "mean needs @return");
}

// ── doc-example: mathlib (C) ──────────────────────────────────────────────────

#[test]
fn doc_example_mathlib_finds_functions() {
    let set = extract_dir(&doc_example("libs/mathlib/src"));
    let items = &set.items;
    assert!(!items.is_empty(), "mathlib should yield items");
    // mathlib is C without namespace — simple names.
    for name in &["clamp", "lerp"] {
        assert!(has_item(items, name), "'{name}' not found in mathlib");
        let item = find_item(items, name);
        assert!(!item.brief.is_empty(), "{name} needs a brief");
    }
}

// ── doc-example: linalg (Fortran) ────────────────────────────────────────────

#[test]
fn doc_example_linalg_finds_module_variables() {
    let set = extract_dir(&doc_example("libs/linalg/src"));
    let items = &set.items;
    for name in &["pi", "default_lda", "singular_tol"] {
        let item = find_item(items, name);
        assert!(matches!(item.kind, DocKind::Variable),
            "{name} should be Variable, got {:?}", item.kind);
        assert!(!item.brief.is_empty(), "{name} needs a brief");
    }
}

#[test]
fn doc_example_linalg_finds_procedures() {
    let set = extract_dir(&doc_example("libs/linalg/src"));
    let dot = find_item(&set.items, "dot");
    assert!(matches!(dot.kind, DocKind::Function));

    let scale = find_item(&set.items, "scale");
    assert!(matches!(scale.kind, DocKind::Subroutine));

    let solve2 = find_item(&set.items, "solve2");
    assert!(matches!(solve2.kind, DocKind::Function));
}

// ── clang path: same fixtures ─────────────────────────────────────────────────

#[cfg(feature = "clang")]
mod clang_tests {
    use super::*;
    use freight_doc::extract_clang::extract_file_clang;

    #[test]
    fn clang_c_buffer_finds_functions() {
        let items = extract_file_clang(&fixture("c/buffer.h"));
        for name in &["buffer_init", "buffer_push", "buffer_clear", "buffer_remaining"] {
            let item = find_item(&items, name);
            assert!(matches!(item.kind, DocKind::Function));
        }
    }

    #[test]
    fn clang_cpp_geometry_namespace_items() {
        let items = extract_file_clang(&fixture("cpp/geometry.hpp"));
        let tri = find_item(&items, "geometry::triangle_area");
        assert!(matches!(tri.kind, DocKind::Function));

        let point = find_item(&items, "geometry::Point");
        assert!(matches!(point.kind, DocKind::Struct | DocKind::Class));

        let aabb = find_item(&items, "geometry::AABB");
        assert!(matches!(aabb.kind, DocKind::Struct | DocKind::Class));
    }

    #[test]
    fn clang_cpp_geometry_class_members() {
        let items = extract_file_clang(&fixture("cpp/geometry.hpp"));
        // With libclang, class members get fully-qualified names.
        let length = find_item(&items, "geometry::Point::length");
        assert!(matches!(length.kind, DocKind::Function));
        assert!(length.meta.parent.as_deref() == Some("Point"),
            "length's parent should be Point");

        let contains = find_item(&items, "geometry::AABB::contains");
        assert!(matches!(contains.kind, DocKind::Function));
    }

    #[test]
    fn clang_cpp_geometry_constructor() {
        let items = extract_file_clang(&fixture("cpp/geometry.hpp"));
        // AABB has an explicit constructor with a doc comment.
        let ctor = items.iter().find(|i| {
            i.name.contains("AABB") && i.meta.attrs.iter().any(|a| a == "constructor")
        });
        assert!(ctor.is_some(), "AABB constructor not found");
    }

    #[test]
    fn clang_doc_example_stats_class_members() {
        let set = extract_dir(&doc_example("libs/stats/src"));
        // With clang, OrderStatistics methods get fully-qualified names.
        let median = find_item(&set.items, "stats::OrderStatistics::median");
        assert!(matches!(median.kind, DocKind::Function));
        assert_eq!(median.meta.parent.as_deref(), Some("OrderStatistics"));
    }
}
