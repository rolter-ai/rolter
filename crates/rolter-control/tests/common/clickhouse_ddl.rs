//! Loading the shipped `clickhouse/*.sql` migrations into a test server (#2495).
//!
//! The HTTP interface takes one statement per request, so a file has to be cut
//! into statements first. Cutting on every `;` breaks as soon as a comment or a
//! string literal holds one, and several migrations do. [`split_statements`]
//! walks the text instead and only splits on a `;` that is real syntax.
#![allow(dead_code)]

use std::path::PathBuf;

/// Every shipped migration, in the order they apply.
pub fn migration_files() -> Vec<PathBuf> {
    let dir = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../clickhouse"));
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .expect("read the clickhouse migration directory")
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            (path.extension()? == "sql").then_some(path)
        })
        .collect();
    files.sort();
    files
}

/// Cut `sql` into trimmed, non-empty statements, dropping `--` and `/* */`
/// comments. A `;` inside a comment or a quoted string (`'`, `"` or a backtick,
/// with a doubled quote or a backslash as the escape) does not end a statement.
pub fn split_statements(sql: &str) -> Result<Vec<String>, String> {
    let mut statements = Vec::new();
    let mut current = String::new();
    let mut chars = sql.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '-' if chars.peek() == Some(&'-') => {
                // keep the newline so tokens either side of the comment stay apart
                for next in chars.by_ref() {
                    if next == '\n' {
                        current.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut closed = false;
                let mut prev = '\0';
                for next in chars.by_ref() {
                    if prev == '*' && next == '/' {
                        closed = true;
                        break;
                    }
                    prev = next;
                }
                if !closed {
                    return Err("unterminated block comment".into());
                }
                current.push(' ');
            }
            '\'' | '"' | '`' => {
                current.push(c);
                let mut closed = false;
                while let Some(next) = chars.next() {
                    current.push(next);
                    if next == '\\' {
                        if let Some(escaped) = chars.next() {
                            current.push(escaped);
                        }
                    } else if next == c {
                        if chars.peek() == Some(&c) {
                            current.extend(chars.next());
                        } else {
                            closed = true;
                            break;
                        }
                    }
                }
                if !closed {
                    return Err(format!("unterminated {c} quoted string"));
                }
            }
            ';' => push_statement(&mut statements, &mut current),
            _ => current.push(c),
        }
    }
    push_statement(&mut statements, &mut current);
    Ok(statements)
}

fn push_statement(statements: &mut Vec<String>, current: &mut String) {
    let statement = current.trim();
    if !statement.is_empty() {
        statements.push(statement.to_string());
    }
    current.clear();
}

/// Apply every shipped ClickHouse migration. All of them are idempotent — the
/// same property `ux-capture.sh apply-schema` relies on — so a shared server
/// that already has the tables is left as it was.
pub async fn apply_schema(client: &reqwest::Client, base: &str) {
    for path in migration_files() {
        let ddl = std::fs::read_to_string(&path).expect("read shipped DDL");
        let statements = split_statements(&ddl)
            .unwrap_or_else(|err| panic!("{}: cannot split: {err}", path.display()));
        for statement in statements {
            let response = client
                .post(format!("{base}/"))
                .body(statement)
                .send()
                .await
                .expect("reach clickhouse");
            assert!(
                response.status().is_success(),
                "{}: {}",
                path.display(),
                response.text().await.unwrap_or_default()
            );
        }
    }
}
