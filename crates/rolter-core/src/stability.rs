//! How finished each subsystem is (#1385).
//!
//! rolter ships a wide surface and not all of it is equally settled. Nothing
//! else in the tree distinguishes a subsystem an operator can build a
//! deployment on from one that is a first cut, so every screen and every
//! endpoint presents with the same confidence. This module is the one place
//! that says otherwise.
//!
//! # The third axis
//!
//! Stability is deliberately separate from the two axes ADR-0031 defines, and
//! composes with both — an experimental subsystem can be perfectly available
//! and perfectly enabled:
//!
//! | Axis | Question | Set by |
//! |---|---|---|
//! | capability | *can* this deployment run it? | the build and its infrastructure |
//! | enablement | is it turned on? | the operator, through a stored flag |
//! | stability | how finished is it? | us, at build time |
//!
//! That is why the table below is a `const` and not a database row. Stability
//! is a property of the code an operator received, so it cannot be edited into
//! a claim the build does not support, and it changes only on upgrade.
//!
//! # What the marker promises
//!
//! [`Stability::Experimental`] means the subsystem **may change shape or be
//! removed in a minor release**. That exemption is the entire point: it is what
//! lets the rest of the surface make a stability promise without being held to
//! the least-finished corner of the product. See
//! `docs/development/stability-markers.md` for the full statement, including
//! what the marker deliberately does *not* say.
//!
//! # Only the exceptions are listed
//!
//! [`Stability::Stable`] is the default and is rendered nowhere. A marker on
//! every page trains an operator to ignore it, so [`SUBSYSTEMS`] carries the
//! exceptions alone and [`stability_of`] answers `Stable` for everything else.

use serde::Serialize;

/// How finished a subsystem is.
///
/// The vocabulary is a deliberate choice of one word: "beta" reads as a
/// time-boxed pre-release that graduates on a schedule, and "unstable" reads as
/// a claim about runtime reliability. Neither is what this marks. "Experimental"
/// says the design may change or be withdrawn, which is exactly the promise
/// being withheld.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stability {
    /// covered by the compatibility promise the release makes
    #[default]
    Stable,
    /// may change shape or be removed in a minor release
    Experimental,
}

impl Stability {
    /// Whether this level is worth surfacing at all.
    ///
    /// `Stable` is the absence of a marker, not a badge to render, so a caller
    /// deciding whether to draw anything asks this rather than comparing
    /// variants by hand.
    pub const fn is_marked(self) -> bool {
        !matches!(self, Stability::Stable)
    }
}

/// One subsystem whose stability differs from the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SubsystemStability {
    /// stable identifier, `snake_case`, never renamed once published — a
    /// consumer keys off it
    pub id: &'static str,
    /// how finished it is; never [`Stability::Stable`] for a listed entry
    pub stability: Stability,
    /// what specifically is unfinished, in one sentence. Rendered verbatim, so
    /// it names the gap rather than restating the level
    pub note: &'static str,
    /// dashboard nav leaf keys this subsystem surfaces on, from `NAV` in
    /// `ui/src/lib/nav.tsx`.
    ///
    /// Carried here rather than rebuilt in the dashboard so the mapping has one
    /// owner. Empty is meaningful and common: a gateway surface such as
    /// `/v1/realtime`, or a cross-cutting concept such as labels, has no nav
    /// entry of its own and is surfaced through the docs alone.
    pub nav_keys: &'static [&'static str],
}

/// Every subsystem this build ships as something other than stable.
///
/// Kept short on purpose. An entry earns its place by pointing at a gap the
/// tree can be checked against — a documented "not yet", a management surface
/// the data plane does not read, an enforcement path a request can take without
/// meeting — not by a reviewer's impression. Anything absent is
/// [`Stability::Stable`]; see the module docs.
///
/// Ordered by `id` so the wire answer and the documentation table read the same
/// way.
pub const SUBSYSTEMS: &[SubsystemStability] = &[
    SubsystemStability {
        id: "labels",
        stability: Stability::Experimental,
        note: "labels are display and filter only: no route can select its \
               targets by label yet, and making them selectable turns them into \
               configuration the data plane consumes",
        nav_keys: &[],
    },
    SubsystemStability {
        id: "mcp_settings",
        stability: Stability::Experimental,
        note: "organization MCP defaults for transport, timeout intent, retries \
               and undeclared tools are stored but not yet read by the proxy. \
               a per-server override is read (#952); a server without one \
               falls back to the deployment-level transport timeouts rather \
               than to these",
        nav_keys: &["mcp-settings"],
    },
    SubsystemStability {
        id: "mcp_tool_groups",
        stability: Stability::Experimental,
        note: "tool-group manifests are stored and published to MCP-aware \
               clients, but the proxy does not enforce group membership as an \
               access boundary",
        nav_keys: &["tool-groups"],
    },
    SubsystemStability {
        id: "plugins",
        stability: Stability::Experimental,
        note: "the webhook payload a plugin receives carries no version of its \
               own, so the dispatch contract cannot change without silently \
               breaking every endpoint already written against it",
        nav_keys: &["plugins"],
    },
    SubsystemStability {
        id: "realtime",
        stability: Stability::Experimental,
        note: "the /v1/realtime websocket relay is outside the request-path \
               subsystems: a session is admitted against process-local caps \
               only, and is not metered by budgets, rate limits, guardrails, \
               usage recording or cost attribution",
        nav_keys: &[],
    },
    SubsystemStability {
        id: "skills_repository",
        stability: Stability::Experimental,
        note: "a skill resolves only through the control-plane API; the gateway \
               serves no skill surface, so how a client addresses and fetches \
               one is not settled",
        nav_keys: &["skills-repo"],
    },
];

/// The stability of one subsystem by id.
///
/// An unknown id answers [`Stability::Stable`] rather than failing: the table
/// lists exceptions, so "not listed" and "stable" are the same statement, and a
/// consumer asking about a subsystem this build does not have should not have
/// to special-case the answer.
pub fn stability_of(id: &str) -> Stability {
    subsystem(id).map_or(Stability::Stable, |entry| entry.stability)
}

/// The full entry for a subsystem, when it has one.
pub fn subsystem(id: &str) -> Option<&'static SubsystemStability> {
    SUBSYSTEMS.iter().find(|entry| entry.id == id)
}

/// The stability of whatever a dashboard nav leaf renders.
///
/// The dashboard asks this per nav entry. A key no subsystem claims is
/// [`Stability::Stable`], which is also the answer for every nav entry in a
/// build whose table is empty.
pub fn stability_of_nav_key(key: &str) -> Stability {
    SUBSYSTEMS
        .iter()
        .find(|entry| entry.nav_keys.contains(&key))
        .map_or(Stability::Stable, |entry| entry.stability)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file from the workspace root, read at run time.
    ///
    /// Not `include_str!`: both files live outside this crate's package, and an
    /// embedded path that escapes it fails `cargo package` on every release.
    /// The checks below therefore skip in an unpacked published crate, where
    /// there is no workspace to check against, and run in every checkout CI has.
    fn workspace_file(relative: &str) -> Option<String> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(relative);
        std::fs::read_to_string(path).ok()
    }

    /// the documentation table, so a marker cannot be added without the page
    /// that explains what it exempts the subsystem from
    fn doc() -> Option<String> {
        workspace_file("docs/development/stability-markers.md")
    }

    /// the dashboard nav, so a `nav_keys` entry cannot rot into a key no screen
    /// answers to
    fn nav() -> Option<String> {
        workspace_file("ui/src/lib/nav.tsx")
    }

    #[test]
    fn absence_is_the_signal() {
        assert_eq!(Stability::default(), Stability::Stable);
        assert!(!Stability::Stable.is_marked());
        assert!(Stability::Experimental.is_marked());
        assert_eq!(stability_of("providers"), Stability::Stable);
        assert_eq!(stability_of_nav_key("providers"), Stability::Stable);
        assert_eq!(subsystem("providers"), None);
    }

    #[test]
    fn a_listed_subsystem_is_never_stable() {
        for entry in SUBSYSTEMS {
            assert!(
                entry.stability.is_marked(),
                "{} is listed but stable; drop the row instead",
                entry.id
            );
            assert_eq!(stability_of(entry.id), entry.stability);
        }
    }

    #[test]
    fn ids_are_unique_sorted_and_snake_case() {
        let mut previous = "";
        for entry in SUBSYSTEMS {
            assert!(
                entry.id > previous,
                "{} is out of order or duplicated; keep SUBSYSTEMS sorted by id",
                entry.id
            );
            assert!(
                entry
                    .id
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                "{} is not snake_case",
                entry.id
            );
            previous = entry.id;
        }
    }

    #[test]
    fn every_note_names_the_gap() {
        for entry in SUBSYSTEMS {
            assert!(
                entry.note.len() > 40,
                "{}: the note is what an operator reads instead of the level; \
                 say what is unfinished",
                entry.id
            );
            assert!(
                !entry.note.to_lowercase().contains("experimental"),
                "{}: the level is already carried by `stability`",
                entry.id
            );
        }
    }

    #[test]
    fn every_nav_key_is_claimed_once_and_exists_in_the_dashboard_nav() {
        let Some(nav) = nav() else { return };
        let mut seen: Vec<&str> = Vec::new();
        for entry in SUBSYSTEMS {
            for key in entry.nav_keys {
                assert!(
                    !seen.contains(key),
                    "{key} is claimed by two subsystems; the dashboard renders one marker per entry"
                );
                seen.push(key);
                assert!(
                    nav.contains(&format!("key: \"{key}\"")),
                    "{key} is not a nav leaf in ui/src/lib/nav.tsx"
                );
                assert_eq!(stability_of_nav_key(key), entry.stability);
            }
        }
    }

    /// The docs page and the table are the same list. `docs/` is the only place
    /// that states what the marker exempts a subsystem from, so a row added
    /// here without a row there ships a badge nobody can look up.
    #[test]
    fn the_docs_page_lists_exactly_these_subsystems() {
        // the page has several tables; only the one under this heading is the
        // list, so the section is bounded before any row is read
        let Some(doc) = doc() else { return };
        let section = doc
            .split_once("## What is experimental in this build")
            .expect("the page states what this build marks")
            .1;
        let section = section
            .split_once("\n## ")
            .map_or(section, |(head, _)| head);
        let documented: Vec<&str> = section
            .lines()
            .filter_map(|line| line.strip_prefix("| `"))
            .filter_map(|rest| rest.split('`').next())
            .collect();
        let listed: Vec<&str> = SUBSYSTEMS.iter().map(|entry| entry.id).collect();
        assert_eq!(
            documented, listed,
            "docs/development/stability-markers.md must list every experimental \
             subsystem, in the same order, one per table row"
        );
    }
}
