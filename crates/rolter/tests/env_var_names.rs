//! Drift guard for the environment variable names the docs hand operators (#1771).
//!
//! The binaries read their settings under a `ROLTER_` prefix — clap's `env =
//! "ROLTER_REDIS_URL"` — and the docs, the Helm chart and `.env.example` kept
//! naming the same settings without it. Nothing reads the unprefixed name, so
//! an operator who followed them got a gateway with no Redis: budgets and rate
//! limits failed open and applied per replica. The chart itself shipped that
//! way. Every failure of this kind is silent, since an unread variable is not
//! an error.
//!
//! The rule: for every `ROLTER_<NAME>` the sources name, the bare `<NAME>` must
//! not appear in the operator-facing files. Only multi-part names are checked
//! (`DATABASE_URL`, `ADMIN_TOKEN`); `PORT` or `KEK` alone is ordinary prose. A
//! shell variable (`$ADMIN_TOKEN` in a curl example) is the reader's own and is
//! not a claim about what rolter reads, so it is left alone, and so is a name
//! carrying a prefix of its own (`$STAGING_DATABASE_URL`). `CLICKHOUSE_URL` is
//! read unprefixed on purpose and never enters the set.
//!
//! Same shape as `rolter-gateway`'s `lock_discipline.rs`: a scan over the
//! workspace, so the drift fails on the PR that writes it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The workspace root, two levels above this crate.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("the workspace root is readable")
}

/// Every file under `dir` whose extension is one of `exts`, build output skipped.
fn files_under(dir: &Path, exts: &[&str], out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path
                .file_name()
                .is_some_and(|n| n == "target" || n == "node_modules" || n == "book")
            {
                continue;
            }
            files_under(&path, exts, out);
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| exts.contains(&e))
        {
            out.push(path);
        }
    }
}

/// Every `"ROLTER_…"` string literal in the crates' sources: the names the
/// binaries read, whether through clap's `env =`, `std::env::var` or a const.
fn prefixed_names() -> BTreeSet<String> {
    let mut sources = Vec::new();
    for entry in std::fs::read_dir(workspace_root().join("crates"))
        .expect("crates/ is readable")
        .flatten()
    {
        files_under(&entry.path().join("src"), &["rs"], &mut sources);
    }
    let mut names = BTreeSet::new();
    for path in sources {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        for (at, _) in text.match_indices("\"ROLTER_") {
            let name: String = text[at + 1..]
                .chars()
                .take_while(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == '_')
                .collect();
            if text[at + 1 + name.len()..].starts_with('"') {
                names.insert(name);
            }
        }
    }
    names
}

/// The unprefixed spellings worth checking: multi-part only.
fn bare_names(prefixed: &BTreeSet<String>) -> BTreeSet<String> {
    prefixed
        .iter()
        .filter_map(|n| n.strip_prefix("ROLTER_"))
        .filter(|n| n.contains('_'))
        .map(str::to_owned)
        .collect()
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// The bare names `line` mentions as a name in its own right: not inside a
/// longer name (`ROLTER_REDIS_URL`, `STAGING_DATABASE_URL`) and not as a shell
/// variable (`$ADMIN_TOKEN`, `${ADMIN_TOKEN}`).
fn bare_mentions(line: &str, bare: &BTreeSet<String>) -> Vec<String> {
    let mut found = Vec::new();
    for name in bare {
        for (at, _) in line.match_indices(name.as_str()) {
            let before = &line[..at];
            let after = &line[at + name.len()..];
            let glued_before = before.chars().next_back().is_some_and(is_name_char);
            let shell = before.ends_with('$') || before.ends_with("${");
            let glued_after = after.chars().next().is_some_and(is_name_char);
            if !glued_before && !shell && !glued_after {
                found.push(name.clone());
            }
        }
    }
    found
}

/// What an operator reads to configure a deployment.
fn operator_files() -> Vec<PathBuf> {
    let root = workspace_root();
    let mut out = Vec::new();
    files_under(&root.join("docs/user-docs"), &["md", "mdx"], &mut out);
    files_under(&root.join("docs/dev-docs"), &["md"], &mut out);
    files_under(
        &root.join("charts"),
        &["yaml", "yml", "tpl", "txt", "md"],
        &mut out,
    );
    files_under(&root.join("docker"), &["yml", "yaml"], &mut out);
    for file in ["README.md", ".env.example", "rolter.example.toml"] {
        out.push(root.join(file));
    }
    out.retain(|p| p.is_file());
    out.sort();
    out
}

#[test]
fn operator_facing_files_never_name_a_rolter_setting_without_its_prefix() {
    let bare = bare_names(&prefixed_names());
    let root = workspace_root();
    let mut offenders = Vec::new();
    for path in operator_files() {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        for (n, line) in text.lines().enumerate() {
            for name in bare_mentions(line, &bare) {
                let rel = path.strip_prefix(&root).unwrap_or(&path).display();
                offenders.push(format!(
                    "{rel}:{}: `{name}` (read as `ROLTER_{name}`)",
                    n + 1
                ));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "these name a setting the binaries only read with the ROLTER_ prefix, so an \
         operator following them sets a variable nothing reads (#1771):\n  {}",
        offenders.join("\n  ")
    );
}

#[test]
fn the_scan_finds_the_names_the_binaries_read() {
    // guards the guard: a walk that found nothing would pass the rule above
    let prefixed = prefixed_names();
    for name in [
        "ROLTER_DATABASE_URL",
        "ROLTER_REDIS_URL",
        "ROLTER_KEY_PEPPER",
    ] {
        assert!(prefixed.contains(name), "{name} missing from {prefixed:?}");
    }
    assert!(!prefixed.contains("CLICKHOUSE_URL"));
    let files = operator_files();
    assert!(files.iter().any(|p| p.ends_with(".env.example")));
    assert!(files.iter().any(|p| p.ends_with("_helpers.tpl")));
}

#[test]
fn a_bare_setting_name_is_reported_and_its_lookalikes_are_not() {
    let bare: BTreeSet<String> = ["REDIS_URL", "DATABASE_URL"].map(String::from).into();
    for line in [
        "Supply `REDIS_URL` as an environment variable",
        "- name: REDIS_URL",
        "DATABASE_URL=postgres://localhost/rolter",
    ] {
        assert_eq!(bare_mentions(line, &bare).len(), 1, "{line}");
    }
    for line in [
        "Supply `ROLTER_REDIS_URL` as an environment variable",
        "- name: ROLTER_REDIS_URL",
        "psql \"$STAGING_DATABASE_URL\"",
        "curl -H \"Authorization: Bearer $REDIS_URL\"",
        "echo ${DATABASE_URL}",
        "CLICKHOUSE_URL=http://localhost:8123",
    ] {
        assert!(bare_mentions(line, &bare).is_empty(), "{line}");
    }
}
