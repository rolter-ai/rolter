//! Source-level drift guard for how postgres tests reach a database (#1430, #1444).
//!
//! Three rules hold the isolation together, and each has already been broken
//! once by a test that looked entirely reasonable on its own:
//!
//! - the database comes from `test_database::url()`, never from
//!   `ROLTER_TEST_DATABASE_URL` directly, or that test runs against the shared
//!   database every other worktree is also using
//! - the schema comes from `TestSchema`, never from a hand-rolled
//!   `create schema`, or it leaks one schema per run (#1364)
//! - no test installs a process-wide environment value only it wants, because
//!   the coverage job runs plain `cargo test` and every test in a binary shares
//!   one environment
//!
//! Same shape as `rolter-gateway`'s `lock_discipline.rs`: a scan over the
//! workspace sources, so the rule fails on the PR that breaks it rather than as
//! a flake weeks later.

use std::path::{Path, PathBuf};

/// The workspace root, two levels above this crate.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("the workspace root is readable")
}

/// This file names every pattern it forbids, so it is never its own offender.
const SELF: &str = "db_test_isolation.rs";

/// Every `.rs` file under `crates/`, excluding build output.
fn sources() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n == "target") {
                    continue;
                }
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(&workspace_root().join("crates"), &mut out);
    out.retain(|path| file_name(path) != SELF);
    out.sort();
    out
}

/// Lines that are code rather than prose, as `(1-based line number, text)`.
fn code_lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.lines()
        .enumerate()
        .map(|(n, line)| (n + 1, line.trim()))
        .filter(|(_, line)| !line.starts_with("//"))
}

fn file_name(path: &Path) -> &str {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
}

/// A test reading the env var itself gets the *shared* database, which is what
/// the derivation exists to avoid — one worktree's suite then applies its
/// migration set to the database another worktree is reading (#1430).
#[test]
fn the_test_database_is_only_ever_resolved_through_test_database_url() {
    let mut offenders = Vec::new();
    for path in sources() {
        // the module that owns the variable is the one place that names it
        if file_name(&path) == "test_database.rs" {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("source file is utf-8");
        for (n, line) in code_lines(&text) {
            if line.contains("\"ROLTER_TEST_DATABASE_URL\"") {
                offenders.push(format!("{}:{n}: {line}", path.display()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "read the test database through rolter_store::postgres::test_database::url(), \
         which derives a database per worktree, rather than the raw env var:\n{}",
        offenders.join("\n")
    );
}

/// A hand-rolled schema is never dropped, so it survives the test that made it
/// and the database grows until Postgres degrades (#1364).
#[test]
fn test_schemas_are_only_ever_created_through_the_guard() {
    let mut offenders = Vec::new();
    for path in sources() {
        if file_name(&path) == "test_schema.rs" {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("source file is utf-8");
        for (n, line) in code_lines(&text) {
            let lowered = line.to_ascii_lowercase();
            if lowered.contains("create schema") || lowered.contains("drop schema") {
                offenders.push(format!("{}:{n}: {line}", path.display()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "build the schema through rolter_store::postgres::test_schema::TestSchema, \
         which drops it when the test finishes:\n{}",
        offenders.join("\n")
    );
}

/// `Kek::from_env()` is read at *request* time, so a test installing a KEK of
/// its own is read by another test's in-flight request under plain `cargo test`
/// — and the failure lands on whichever unrelated seal-then-open test was
/// mid-flight, never on the test that caused it (#1351, #1444).
#[test]
fn the_postgres_suites_install_one_shared_kek_and_no_other_process_wide_value() {
    let mut offenders = Vec::new();
    for path in sources() {
        let text = std::fs::read_to_string(&path).expect("source file is utf-8");
        if !text.contains("rolter_store::postgres::test_schema")
            && !text.contains("super::test_schema")
            && !text.contains("test_schema::TestSchema")
        {
            continue;
        }
        for (n, line) in code_lines(&text) {
            if !line.contains("env::set_var(") {
                continue;
            }
            // the one shared value every call site installs, so the race has
            // nothing to observe
            if line.contains("\"ROLTER_KEK\", TEST_KEK") {
                continue;
            }
            offenders.push(format!("{}:{n}: {line}", path.display()));
        }
    }
    assert!(
        offenders.is_empty(),
        "the environment is process-wide and the coverage job runs every test in a binary \
         as a thread; pass the value into the app under test instead:\n{}",
        offenders.join("\n")
    );
}
