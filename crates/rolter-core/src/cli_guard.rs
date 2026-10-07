//! Keeping secrets out of `--help` (#2793).
//!
//! clap prints `[env: NAME=value]` under every argument backed by an
//! environment variable, current value included, unless the argument sets
//! `hide_env_values = true`. `--help` is the first thing an operator runs, and
//! pastes into an issue, when something does not start, so an admin token or a
//! datastore url with a password in it leaked through the one door the startup
//! log redaction of #2406 did not cover.
//!
//! Each binary's tests hand its clap `Command` to [`assert_secret_envs_hidden`].
//! The check walks the whole tree, every subcommand included, rather than
//! checking a list somebody has to remember to extend, so a new flag is caught
//! on the pull request that adds it. The rule is on the *variable name*, which
//! is what the author picks and a reviewer sees: see [`is_secret_env_name`].
//!
//! Test support only, behind the `cli-guard` feature the binary crates enable
//! in their dev-dependencies.

use clap::Command;

/// Fragments that mark an environment variable as carrying a secret or a
/// credential-bearing url, matched case-insensitively anywhere in the name.
///
/// `_URL` is on the list because a datastore url routinely carries its
/// password as userinfo (`redis://:pw@host`). The trailing entries are the
/// spellings a future flag is most likely to use for the same thing.
const SECRET_NAME_MARKERS: &[&str] = &[
    "TOKEN",
    "KEY",
    "PEPPER",
    "KEK",
    "SECRET",
    "PASSWORD",
    "_URL",
    "CREDENTIAL",
    "DSN",
    "HEADERS",
];

/// Whether an argument reading `name` from the environment must hide the value
/// in `--help`.
pub fn is_secret_env_name(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    SECRET_NAME_MARKERS
        .iter()
        .any(|marker| name.contains(marker))
}

/// One argument that reads a secret-looking environment variable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretEnvArg {
    /// The subcommand path, e.g. `rolter kek`.
    pub command: String,
    /// How the argument is spelled on the command line, e.g. `--database-url`;
    /// its id when it has no long form.
    pub arg: String,
    /// The environment variable it reads. Never its value.
    pub env: String,
    /// Whether `--help` omits the variable's value.
    pub hidden: bool,
}

/// Every argument under `command` (itself and all subcommands, recursively)
/// that reads an environment variable whose name [`is_secret_env_name`].
pub fn secret_env_args(command: &Command) -> Vec<SecretEnvArg> {
    let mut found = Vec::new();
    collect(command, command.get_name().to_owned(), &mut found);
    found
}

fn collect(command: &Command, path: String, found: &mut Vec<SecretEnvArg>) {
    for arg in command.get_arguments() {
        let Some(env) = arg.get_env().map(|name| name.to_string_lossy()) else {
            continue;
        };
        if is_secret_env_name(&env) {
            found.push(SecretEnvArg {
                command: path.clone(),
                arg: arg
                    .get_long()
                    .map_or_else(|| arg.get_id().to_string(), |long| format!("--{long}")),
                env: env.into_owned(),
                hidden: arg.is_hide_env_values_set(),
            });
        }
    }
    for sub in command.get_subcommands() {
        collect(sub, format!("{path} {}", sub.get_name()), found);
    }
}

/// Panic, naming each offender, unless every secret-looking environment
/// variable under `command` is hidden from `--help`. Returns how many such
/// arguments it checked, so a caller can tell a pass from a walk that found
/// nothing to look at.
///
/// The message names variables, never their values.
pub fn assert_secret_envs_hidden(command: &Command) -> usize {
    let args = secret_env_args(command);
    let leaking: Vec<String> = args
        .iter()
        .filter(|arg| !arg.hidden)
        .map(|arg| format!("`{} {}` (reads {})", arg.command, arg.arg, arg.env))
        .collect();
    assert!(
        leaking.is_empty(),
        "these arguments print a secret-looking environment variable's value in --help; \
         add `hide_env_values = true` to their #[arg(...)]: {}",
        leaking.join(", ")
    );
    args.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Arg;

    #[test]
    fn names_that_can_carry_a_secret_are_recognised() {
        for name in [
            "ADMIN_TOKEN",
            "INTERNAL_TOKEN",
            "DATABASE_URL",
            "REDIS_URL",
            "CLICKHOUSE_URL",
            "KEK",
            "KEK_OLD",
            "API_KEY",
            "PASSWORD_PEPPER",
            "SESSION_SECRET",
            "DB_PASSWORD",
            "OTEL_EXPORTER_OTLP_HEADERS",
            // case does not decide it
            "rolter_admin_token",
        ] {
            assert!(is_secret_env_name(name), "{name} should need hiding");
        }
    }

    #[test]
    fn plain_settings_are_not_flagged() {
        for name in [
            "CONFIG",
            "HOST",
            "PORT",
            "UI_DIR",
            "DB_MAX_CONNECTIONS",
            "LOGIN_MAX_FAILURES",
            "ALLOW_OPEN_MODE",
            "UI_OTEL_ENDPOINT",
        ] {
            assert!(!is_secret_env_name(name), "{name} is an ordinary setting");
        }
    }

    fn sample(hide_token: bool, hide_nested_url: bool) -> Command {
        let token = Arg::new("admin_token")
            .long("admin-token")
            .env("CLI_GUARD_SAMPLE_TOKEN")
            .hide_env_values(hide_token);
        let nested = Arg::new("database_url")
            .long("database-url")
            .env("CLI_GUARD_SAMPLE_DATABASE_URL")
            .hide_env_values(hide_nested_url);
        let plain = Arg::new("port").long("port").env("CLI_GUARD_SAMPLE_PORT");
        Command::new("sample")
            .arg(token)
            .arg(plain)
            .subcommand(Command::new("inner").subcommand(Command::new("deep").arg(nested)))
    }

    #[test]
    fn the_walk_reaches_nested_subcommands_and_skips_plain_settings() {
        let found = secret_env_args(&sample(true, true));
        let reads: Vec<(&str, &str)> = found
            .iter()
            .map(|arg| (arg.command.as_str(), arg.env.as_str()))
            .collect();
        assert_eq!(
            reads,
            [
                ("sample", "CLI_GUARD_SAMPLE_TOKEN"),
                ("sample inner deep", "CLI_GUARD_SAMPLE_DATABASE_URL"),
            ]
        );
        assert!(found.iter().all(|arg| arg.hidden));
        assert_eq!(assert_secret_envs_hidden(&sample(true, true)), 2);
    }

    #[test]
    #[should_panic(expected = "`sample --admin-token` (reads CLI_GUARD_SAMPLE_TOKEN)")]
    fn an_unhidden_top_level_secret_fails_the_check() {
        assert_secret_envs_hidden(&sample(false, true));
    }

    #[test]
    #[should_panic(expected = "`sample inner deep --database-url`")]
    fn an_unhidden_secret_in_a_subcommand_fails_the_check() {
        assert_secret_envs_hidden(&sample(true, false));
    }

    /// The premise the walk rests on: clap reads the variable when it renders
    /// help, prints it unless told not to, and `hide_env_values` is what stops
    /// it. Each argument reads a variable named for this process and this run,
    /// so setting it cannot reach a test that reads a real one.
    #[test]
    fn hiding_an_env_value_keeps_it_out_of_the_rendered_help() {
        let unique = format!("{}_{}", std::process::id(), uuid::Uuid::new_v4().simple());
        // clap keeps the name as a `'static` string, so the test leaks its two
        let shown_var: &'static str =
            Box::leak(format!("CLI_GUARD_SHOWN_TOKEN_{unique}").into_boxed_str());
        let hidden_var: &'static str =
            Box::leak(format!("CLI_GUARD_HIDDEN_TOKEN_{unique}").into_boxed_str());
        let shown_value = format!("shown-{}", uuid::Uuid::new_v4().simple());
        let hidden_value = format!("hidden-{}", uuid::Uuid::new_v4().simple());

        // clap reads the variable when `.env(..)` is called, not when it renders,
        // so the value has to be in the environment before the argument is built
        std::env::set_var(shown_var, &shown_value);
        std::env::set_var(hidden_var, &hidden_value);
        let mut command = Command::new("sample")
            .arg(Arg::new("shown").long("shown").env(shown_var))
            .arg(
                Arg::new("hidden")
                    .long("hidden")
                    .env(hidden_var)
                    .hide_env_values(true),
            );
        let help = command.render_long_help().to_string();
        std::env::remove_var(shown_var);
        std::env::remove_var(hidden_var);

        // the control: without it a pass could mean clap never read the env
        assert!(
            help.contains(&shown_value),
            "an argument that does not hide its env value should print it"
        );
        assert!(
            !help.contains(&hidden_value),
            "hide_env_values left the value in --help"
        );
        assert!(
            help.contains(hidden_var),
            "the variable's name should still be listed"
        );
    }
}
