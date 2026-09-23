## 2026-09-23 - Binary CLI environment variables drifting from reference docs

**Learning:** CLI environment variables declared via `clap` `env = "..."` attributes in `crates/rolter-control` and `crates/rolter-gateway` can quietly drift from `docs/user-docs/configuration/environment-variables.mdx`.
**Action:** Guard CLI environment variable documentation completeness using `all_binary_cli_env_vars_are_documented_in_reference` in `crates/rolter/tests/env_var_names.rs`.
