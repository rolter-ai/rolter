//! Drift guard for the metrics page (#1888).
//!
//! `docs/user-docs/observability/metrics.mdx` once listed nine event counters
//! the gateway never exported (`rolter_retry_total` for the real
//! `rolter_retries_total`, and so on), so an alert built from the page matched
//! nothing. This fails when the page names a `rolter_*` series that no source
//! file of the gateway spells as a metric name. The same shape of guard as
//! `the_docs_page_lists_exactly_these_subsystems` in `rolter-core`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const PAGE: &str = "../../docs/user-docs/observability/metrics.mdx";

/// Every `.rs` file under the crate's `src/`, concatenated.
fn gateway_source() -> String {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("src/ is readable") {
            let path = entry.expect("readable dir entry").path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    walk(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    files.sort();
    files
        .iter()
        .map(|path| std::fs::read_to_string(path).expect("source file is utf-8"))
        .collect()
}

/// Every `rolter_*` identifier in `text`. A pattern such as `rolter_*_total`
/// names a family rather than a series and is skipped.
fn rolter_names(text: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let bytes = text.as_bytes();
    let mut from = 0;
    while let Some(found) = text[from..].find("rolter_") {
        let start = from + found;
        let mut end = start;
        while end < bytes.len()
            && (bytes[end].is_ascii_lowercase()
                || bytes[end].is_ascii_digit()
                || bytes[end] == b'_')
        {
            end += 1;
        }
        from = end.max(start + 1);
        let preceded =
            start > 0 && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_');
        let name = &text[start..end];
        if !preceded && bytes.get(end) != Some(&b'*') && !name.ends_with('_') {
            names.insert(name.to_string());
        }
    }
    names
}

/// A histogram is exported as its base name plus `_bucket`, `_sum` and
/// `_count`, so the page may name any of the four.
fn base(name: &str) -> &str {
    ["_bucket", "_sum", "_count"]
        .iter()
        .find_map(|suffix| name.strip_suffix(suffix))
        .unwrap_or(name)
}

#[test]
fn the_metrics_page_names_only_series_the_gateway_exports() {
    let page = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(PAGE))
        .expect("the metrics page is readable");
    let source = gateway_source();
    let documented = rolter_names(&page);
    assert!(
        documented.len() > 30,
        "the scan found only {} names on the page",
        documented.len()
    );
    let unknown: Vec<&String> = documented
        .iter()
        .filter(|name| {
            let quoted = format!("\"{}", base(name));
            !source.contains(&quoted) && !source.contains(name.as_str())
        })
        .collect();
    assert!(
        unknown.is_empty(),
        "docs/user-docs/observability/metrics.mdx names series the gateway does not render: \
         {unknown:?}. Use the name `crates/rolter-gateway/src/metrics.rs` renders, or drop the row"
    );
}

#[test]
fn the_scan_tells_a_series_from_a_family_pattern() {
    let names = rolter_names("`rolter_retries_total` and every `rolter_*_total`, not xrolter_a");
    assert_eq!(
        names.into_iter().collect::<Vec<_>>(),
        vec!["rolter_retries_total".to_string()]
    );
    assert_eq!(
        base("rolter_request_latency_ms_bucket"),
        "rolter_request_latency_ms"
    );
    assert_eq!(base("rolter_retries_total"), "rolter_retries_total");
}
