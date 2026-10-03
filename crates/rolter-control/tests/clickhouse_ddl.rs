//! The statement splitter behind the ClickHouse test schema, and a guard that
//! every shipped migration survives it (#2495). Needs no server.

#[path = "common/clickhouse_ddl.rs"]
mod clickhouse_ddl;

use clickhouse_ddl::{migration_files, split_statements};

#[test]
fn splits_on_semicolons_and_drops_empties() {
    let got = split_statements("select 1;\n\n  select 2 ;;").unwrap();
    assert_eq!(got, ["select 1", "select 2"]);
}

#[test]
fn ignores_semicolons_in_line_comments() {
    let got = split_statements("-- a; b\nselect 1; -- tail; more\nselect 2").unwrap();
    assert_eq!(got, ["select 1", "select 2"]);
}

#[test]
fn ignores_semicolons_in_block_comments() {
    let got = split_statements("select /* a; b\n; c */ 1; select 2").unwrap();
    assert_eq!(got, ["select   1", "select 2"]);
}

#[test]
fn ignores_semicolons_in_quoted_strings() {
    let got = split_statements("select 'a;b'; select \"c;d\"; select `e;f`").unwrap();
    assert_eq!(got, ["select 'a;b'", "select \"c;d\"", "select `e;f`"]);
}

#[test]
fn comment_markers_inside_strings_are_text() {
    let got = split_statements("select '--x; /* y'; select 2").unwrap();
    assert_eq!(got, ["select '--x; /* y'", "select 2"]);
}

#[test]
fn escaped_quotes_do_not_end_a_string() {
    let got =
        split_statements(r"select 'it''s; fine'; select 'back\'slash; ok'; select 3").unwrap();
    assert_eq!(
        got,
        [
            "select 'it''s; fine'",
            r"select 'back\'slash; ok'",
            "select 3"
        ]
    );
}

#[test]
fn unterminated_input_is_an_error() {
    assert!(split_statements("select 'oops; select 2").is_err());
    assert!(split_statements("select 1 /* oops; select 2").is_err());
}

#[test]
fn every_shipped_migration_splits_into_statements() {
    let files = migration_files();
    assert!(!files.is_empty(), "no clickhouse migrations found");
    for path in files {
        let ddl = std::fs::read_to_string(&path).expect("read shipped DDL");
        let statements =
            split_statements(&ddl).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
        assert!(
            !statements.is_empty(),
            "{} has no statements",
            path.display()
        );
    }
}
