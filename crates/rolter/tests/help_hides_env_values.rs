//! `rolter <command> --help` must not print the secrets in its environment (#2793).
//!
//! clap renders `[env: NAME=value]` under every flag backed by a variable, and
//! `--help` is what an operator runs, and pastes into an issue, when something
//! does not start. The launcher's unit test walks the clap `Command` and fails
//! on any secret-looking flag that does not hide its value; this one runs the
//! real binary with the variables set, so the claim is checked on the output an
//! operator would actually paste. The variables go on the child's own
//! environment, never the test process's, so no other test can see them.
//!
//! The variable list is the one place here that needs extending by hand, and
//! the walk is what catches a flag that is not on it.

use std::process::Command;

/// Environment variables that carry a secret, or a url a password can be
/// embedded in, and that a launcher subcommand reads.
const SECRET_VARS: &[&str] = &[
    "ROLTER_ADMIN_TOKEN",
    "ROLTER_INTERNAL_TOKEN",
    "ROLTER_DATABASE_URL",
    "ROLTER_REDIS_URL",
    "CLICKHOUSE_URL",
    "ROLTER_SNAPSHOT_URL",
    "ROLTER_ADMIN_URL",
    "ROLTER_GATEWAY_URL",
    "ROLTER_UI_DOCS_BASE_URL",
];

/// A setting that is not a secret and that `easy-up` and `control` print, which
/// proves the child's environment reaches clap's help renderer. Without it, a
/// sentinel's absence could only mean the variables never arrived.
const VISIBLE_VAR: &str = "ROLTER_UI_DIR";

/// The subcommands whose `--help` is rendered. The postgres-only ones exist only
/// in a build with that feature
fn commands() -> Vec<Vec<&'static str>> {
    let mut commands = vec![vec!["gateway"], vec!["control"], vec!["easy-up"]];
    if cfg!(feature = "postgres") {
        commands.extend([vec!["kek"], vec!["config", "export"], vec!["mfa", "reset"]]);
    }
    commands
}

/// The text `rolter <command> --help` writes, with every variable in `env`
/// exported to the child.
fn help(command: &[&str], env: &[(String, String)]) -> String {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rolter"));
    child.args(command).arg("--help");
    for (name, value) in env {
        child.env(name, value);
    }
    let out = child.output().expect("the rolter binary starts");
    assert!(
        out.status.success(),
        "`rolter {} --help` exited with {}",
        command.join(" "),
        out.status
    );
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    text
}

#[test]
fn no_subcommand_prints_a_secret_from_the_environment() {
    let run = uuid::Uuid::new_v4().simple().to_string();
    // the sentinels are kept apart from the names they are exported under, and
    // a failure reports an index into `SECRET_VARS` rather than any text that
    // came from a secret-bearing variable
    let sentinels: Vec<String> = SECRET_VARS
        .iter()
        .map(|name| format!("sentinel-{run}-{}", name.to_lowercase()))
        .collect();
    let visible = format!("visible-{run}");
    let mut env: Vec<(String, String)> = SECRET_VARS
        .iter()
        .zip(&sentinels)
        .map(|(name, sentinel)| (name.to_string(), sentinel.clone()))
        .collect();
    env.push((VISIBLE_VAR.to_string(), visible.clone()));

    let mut saw_visible = false;
    for command in commands() {
        let text = help(&command, &env);
        saw_visible |= text.contains(&visible);
        let leaked: Vec<usize> = sentinels
            .iter()
            .enumerate()
            .filter(|(_, sentinel)| text.contains(sentinel.as_str()))
            .map(|(index, _)| index)
            .collect();
        assert!(
            leaked.is_empty(),
            "`rolter {} --help` printed the environment value of SECRET_VARS entries {leaked:?}",
            command.join(" ")
        );
    }
    assert!(
        saw_visible,
        "no --help showed {VISIBLE_VAR}, so the sentinels never reached clap's renderer"
    );
}

/// Hiding the value must not hide the variable: an operator reads the name off
/// `--help` to know what to export.
#[test]
fn the_variable_names_are_still_listed() {
    let text = help(&["easy-up"], &[]);
    for name in ["ROLTER_ADMIN_TOKEN", "ROLTER_REDIS_URL", "CLICKHOUSE_URL"] {
        assert!(
            text.contains(name),
            "`rolter easy-up --help` no longer names {name}"
        );
    }
}
