//! marker crate for the rolter dashboard (vite/react app under `ui/`)
//!
//! this crate ships no rust code. it exists solely so release-plz treats `ui/`
//! as a workspace package and attributes `feat(ui)`/`fix(ui)` commits to a
//! changelog, which the gateway release aggregates via `changelog_include`
