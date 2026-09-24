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

/// Find every environment variable name declared via clap's `env = "..."` in the binary CLI definitions.
fn binary_cli_env_vars() -> BTreeSet<String> {
    let root = workspace_root();
    let mut sources = Vec::new();
    for crate_name in ["rolter-control", "rolter-gateway"] {
        files_under(
            &root.join("crates").join(crate_name).join("src"),
            &["rs"],
            &mut sources,
        );
    }
    let mut vars = BTreeSet::new();
    for path in sources {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        for (at, _) in text.match_indices("env = \"") {
            let start = at + "env = \"".len();
            let var: String = text[start..]
                .chars()
                .take_while(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == '_')
                .collect();
            if text[start + var.len()..].starts_with('"')
                && (var.starts_with("ROLTER_") || var == "CLICKHOUSE_URL")
            {
                vars.insert(var);
            }
        }
    }
    vars
}

#[test]
fn all_binary_cli_env_vars_are_documented_in_reference() {
    let cli_vars = binary_cli_env_vars();
    let ref_path = workspace_root().join("docs/user-docs/configuration/environment-variables.mdx");
    let text = std::fs::read_to_string(&ref_path).expect("environment-variables.mdx is readable");

    let mut missing = Vec::new();
    for var in cli_vars {
        if !text.contains(&var) {
            missing.push(var);
        }
    }

    assert!(
        missing.is_empty(),
        "these CLI environment variables are read by the binaries but missing from docs/user-docs/configuration/environment-variables.mdx:\n  {}",
        missing.join("\n  ")
    );
}

/// Recursively collect all string page targets in `docs.json`.
fn extract_pages_from_json(val: &serde_json::Value, pages: &mut BTreeSet<String>) {
    match val {
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::Array(arr)) = map.get("pages") {
                for item in arr {
                    if let Some(p) = item.as_str() {
                        pages.insert(p.to_string());
                    }
                }
            }
            for v in map.values() {
                extract_pages_from_json(v, pages);
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr {
                extract_pages_from_json(v, pages);
            }
        }
        _ => {}
    }
}

#[test]
fn every_user_doc_is_listed_in_docs_json() {
    let root = workspace_root();
    let user_docs_dir = root.join("docs/user-docs");
    let docs_json_path = user_docs_dir.join("docs.json");
    let docs_json_text =
        std::fs::read_to_string(&docs_json_path).expect("docs/user-docs/docs.json is readable");
    let docs_json_val: serde_json::Value =
        serde_json::from_str(&docs_json_text).expect("docs.json parses as JSON");

    let mut listed_pages = BTreeSet::new();
    extract_pages_from_json(&docs_json_val, &mut listed_pages);

    let mut doc_files = Vec::new();
    files_under(&user_docs_dir, &["md", "mdx"], &mut doc_files);

    let mut missing = Vec::new();
    for path in doc_files {
        if path.file_name().is_some_and(|n| n == "README.md") {
            continue;
        }
        let rel_path = path
            .strip_prefix(&user_docs_dir)
            .expect("path is under docs/user-docs")
            .to_string_lossy()
            .replace('\\', "/");

        let page_key = if let Some(stripped) = rel_path.strip_suffix(".mdx") {
            stripped
        } else if let Some(stripped) = rel_path.strip_suffix(".md") {
            stripped
        } else {
            &rel_path
        };

        if !listed_pages.contains(page_key) {
            missing.push(rel_path);
        }
    }

    assert!(
        missing.is_empty(),
        "the following user documentation files exist under docs/user-docs/ but are not listed in docs.json:\n  {}\nAn unlisted page is invisible in the Mintlify navigation.",
        missing.join("\n  ")
    );
}

/// Find every field name declared in `ServerConfig` struct in `crates/rolter-core/src/config.rs`.
fn server_config_fields() -> BTreeSet<String> {
    let config_rs_path = workspace_root().join("crates/rolter-core/src/config.rs");
    let text = std::fs::read_to_string(&config_rs_path).expect("config.rs is readable");

    let start = text
        .find("pub struct ServerConfig {")
        .expect("config.rs declares `pub struct ServerConfig {`");
    let rest = &text[start..];
    // the struct closes on the first unindented brace; a bare `}` would stop at
    // one inside a doc comment and silently drop every field after it
    let end = rest
        .find("\n}")
        .expect("ServerConfig closes with an unindented `}`");
    rest[..end]
        .lines()
        .filter_map(|line| line.trim().strip_prefix("pub "))
        .filter_map(|field| field.split_once(':'))
        .map(|(name, _)| name.trim().to_string())
        .collect()
}

#[test]
fn all_server_config_fields_are_documented_in_config_file_reference() {
    let fields = server_config_fields();
    assert!(!fields.is_empty(), "no ServerConfig fields were found");

    let ref_path = workspace_root().join("docs/user-docs/configuration/config-file.mdx");
    let text = std::fs::read_to_string(&ref_path).expect("config-file.mdx is readable");

    let mut missing = Vec::new();
    for field in fields {
        // a `<ParamField>` entry, not a passing mention in prose or an example
        if !text.contains(&format!("<ParamField path=\"{field}\"")) {
            missing.push(field);
        }
    }

    assert!(
        missing.is_empty(),
        "these ServerConfig fields are defined in crates/rolter-core/src/config.rs but missing from docs/user-docs/configuration/config-file.mdx:\n  {}",
        missing.join("\n  ")
    );
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

#[test]
fn every_dev_doc_is_listed_in_summary() {
    let root = workspace_root();
    let dev_docs_dir = root.join("docs/dev-docs");
    let summary_path = dev_docs_dir.join("SUMMARY.md");
    let summary_text =
        std::fs::read_to_string(&summary_path).expect("docs/dev-docs/SUMMARY.md is readable");

    let mut doc_files = Vec::new();
    files_under(&dev_docs_dir, &["md"], &mut doc_files);

    let mut missing = Vec::new();
    for path in doc_files {
        if path == summary_path {
            continue;
        }
        let rel_path = path
            .strip_prefix(&dev_docs_dir)
            .expect("path is under docs/dev-docs")
            .to_string_lossy()
            .replace('\\', "/");

        if !summary_text.contains(&rel_path) {
            missing.push(rel_path);
        }
    }

    assert!(
        missing.is_empty(),
        "the following developer documentation files exist under docs/dev-docs/ but are not listed in SUMMARY.md:\n  {}\nAn unlisted page is invisible in the mdBook navigation.",
        missing.join("\n  ")
    );
}
