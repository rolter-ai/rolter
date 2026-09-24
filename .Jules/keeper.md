## 2026-09-22 - Unlisted mdBook Developer Documentation Pages

**Learning:** When adding new developer documentation `.md` files under `docs/dev-docs/`, authors sometimes forget to add them to `docs/dev-docs/SUMMARY.md`. Without an entry in `SUMMARY.md`, mdBook will not render the pages in its navigation sidebar, making them effectively hidden to readers.

**Action:** Maintain an automated workspace drift guard test `every_dev_doc_is_listed_in_summary` in `crates/rolter/tests/env_var_names.rs` that scans `docs/dev-docs/` for `.md` files and asserts that `SUMMARY.md` lists every relative path.

## 2026-09-23 - Binary CLI environment variables drifting from reference docs

**Learning:** CLI environment variables declared via `clap` `env = "..."` attributes in `crates/rolter-control` and `crates/rolter-gateway` can quietly drift from `docs/user-docs/configuration/environment-variables.mdx`.
**Action:** Guard CLI environment variable documentation completeness using `all_binary_cli_env_vars_are_documented_in_reference` in `crates/rolter/tests/env_var_names.rs`.

## 2026-09-24 - ServerConfig fields drifting from config-file reference docs

**Learning:** Configuration fields on `ServerConfig` in `crates/rolter-core/src/config.rs` (`max_body_bytes`, `require_auth`) can quietly drift from `docs/user-docs/configuration/config-file.mdx`.
**Action:** Guard `ServerConfig` documentation completeness using `all_server_config_fields_are_documented_in_config_file_reference` in `crates/rolter/tests/env_var_names.rs`.
