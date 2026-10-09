//! Which of `request_logs`' hand-applied columns the ClickHouse in front of
//! the control plane actually has (#2903).
//!
//! ClickHouse only runs the files under `clickhouse/` when its data directory
//! is first created, so on an existing deployment an operator applies each one
//! by hand, possibly after the control plane that reads the column has been
//! upgraded. A select that names a column the table lacks fails the whole
//! statement, which used to mean a control plane newer than `015` or `016`
//! could not list a single request (#2807, #2836), and is why the 1 hour
//! cache-write share (`017`) went unread for a release.
//!
//! The list below names those columns and the value a row reads as without
//! them. The invocation list probes `system.columns` for them, selects the
//! ones that exist, and stands the default in for the rest, so applying a
//! migration is something an operator does for the data and never a condition
//! of upgrading the control plane. The default is exactly what ClickHouse
//! fills in for a row written before the column existed, so a table without
//! the column and a table with it, applied late, read the same for old rows.
//!
//! The probe is cached on the client: one `system.columns` read per window
//! rather than one per request. A column that is missing is looked for again
//! soon (an operator who applies the file expects the next page to show it,
//! without a restart); a table that has every column is only re-read now and
//! then, in case it was recreated.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde_json::Value;

/// A `request_logs` column added by a migration that existing deployments
/// apply by hand.
pub(crate) struct OptionalColumn {
    pub(crate) name: &'static str,
    /// The SQL expression a select reads in the column's place when the table
    /// lacks it, typed as the column is so the answer's shape does not depend
    /// on whether the migration ran.
    pub(crate) default: &'static str,
}

/// Every optional column, with the migration that adds it in the comment.
///
/// Add a column here when a new `clickhouse/NNN_*.sql` adds one the control
/// plane's invocation list should return; the list query names it through
/// [`Columns::select`] so the two cannot drift apart (a test checks that each
/// entry is selected). Older columns (`log_id`, `unpriced`, the attribution
/// ids) predate this list and stay required.
pub(crate) const OPTIONAL_COLUMNS: &[OptionalColumn] = &[
    // 015_upstream_attempts.sql
    OptionalColumn {
        name: "upstream_status",
        default: "toUInt16(0)",
    },
    // 015_upstream_attempts.sql
    OptionalColumn {
        name: "attempts",
        default: "toUInt8(0)",
    },
    // 016_lifecycle_operation.sql
    OptionalColumn {
        name: "lifecycle_operation",
        default: "''",
    },
    // 017_cache_write_1h.sql
    OptionalColumn {
        name: "cache_write_1h_tokens",
        default: "toUInt32(0)",
    },
];

/// The probe: every column `request_logs` has in the database the client reads
/// from. `currentDatabase()` is the one an unqualified `from request_logs`
/// resolves in, so the answer is about the table the list query reads.
pub(crate) const PROBE_SQL: &str = "select name from system.columns \
     where database = currentDatabase() and table = 'request_logs' format JSON";

/// How long an answer in which every optional column was present is kept.
/// Nothing is left to find, so it only needs refreshing against a table that
/// was dropped and recreated.
pub(crate) const SETTLED_FOR: Duration = Duration::from_secs(600);

/// How long an answer in which an optional column was missing, or that could
/// not be read, is kept. Short so that applying the migration shows on the
/// next page within a minute, without a control-plane restart.
pub(crate) const UNSETTLED_FOR: Duration = Duration::from_secs(60);

/// Which optional columns the table lacks. The default is none missing, which
/// is also what a select assumes when the probe could not tell: that is the
/// behaviour before this list existed, so an unreadable `system.columns` is no
/// worse than it was.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Columns {
    missing: Vec<&'static str>,
}

impl Columns {
    /// The optional columns, `names` excepted, are present.
    #[cfg(test)]
    pub(crate) fn lacking(names: &[&'static str]) -> Self {
        Self {
            missing: names.to_vec(),
        }
    }

    /// Read the probe's rows (`{"name": ...}` per column). `None` when the
    /// answer names no column at all: a table the probe cannot see, such as one
    /// the user holds no grant on, is not the same as a table without the
    /// columns, and reading it as that would blank data that is there.
    pub(crate) fn from_rows(rows: &[Value]) -> Option<Self> {
        let names: HashSet<&str> = rows
            .iter()
            .filter_map(|row| row.get("name").and_then(Value::as_str))
            .collect();
        if names.is_empty() {
            return None;
        }
        Some(Self {
            missing: OPTIONAL_COLUMNS
                .iter()
                .map(|column| column.name)
                .filter(|name| !names.contains(name))
                .collect(),
        })
    }

    /// Whether every optional column is there.
    pub(crate) fn complete(&self) -> bool {
        self.missing.is_empty()
    }

    /// The optional columns the table lacks, for the log line that says so.
    pub(crate) fn missing(&self) -> &[&'static str] {
        &self.missing
    }

    /// The select-list term for `name`: the column itself when the table has
    /// it, otherwise its default under the same name, so the row's shape is
    /// the same either way.
    pub(crate) fn select(&self, name: &str) -> String {
        match OPTIONAL_COLUMNS
            .iter()
            .find(|column| column.name == name && self.missing.contains(&column.name))
        {
            Some(column) => format!("{} as {}", column.default, column.name),
            None => name.to_string(),
        }
    }
}

/// The client's memory of its last probe.
///
/// Reads take a `parking_lot` mutex held for a copy of a handful of names and
/// never across an await. The control plane is not the data plane's hot path,
/// and two requests that both miss at once simply probe twice.
pub(crate) struct ColumnCache {
    entry: Mutex<Option<Entry>>,
    settled_for: Duration,
    unsettled_for: Duration,
}

struct Entry {
    columns: Columns,
    at: Instant,
    /// every optional column was found, so there is nothing to look again for
    settled: bool,
}

impl ColumnCache {
    pub(crate) fn new() -> Self {
        Self::with_lifetimes(SETTLED_FOR, UNSETTLED_FOR)
    }

    /// [`ColumnCache::new`] with explicit lifetimes, so a test can cross them
    /// without waiting out the production values.
    pub(crate) fn with_lifetimes(settled_for: Duration, unsettled_for: Duration) -> Self {
        Self {
            entry: Mutex::new(None),
            settled_for,
            unsettled_for,
        }
    }

    /// The remembered answer, unless it has outlived its lifetime at `now`.
    pub(crate) fn fresh(&self, now: Instant) -> Option<Columns> {
        let guard = self.entry.lock();
        let entry = guard.as_ref()?;
        let lifetime = if entry.settled {
            self.settled_for
        } else {
            self.unsettled_for
        };
        (now.saturating_duration_since(entry.at) < lifetime).then(|| entry.columns.clone())
    }

    /// Remember what a probe at `now` found. `None` is a probe that could not
    /// tell (see [`Columns::from_rows`]): it is remembered as "assume present"
    /// for the short lifetime, so an unreadable `system.columns` costs one
    /// failed read a minute rather than one per request.
    pub(crate) fn store(&self, now: Instant, found: Option<Columns>) -> Columns {
        let settled = found.as_ref().is_some_and(Columns::complete);
        let columns = found.unwrap_or_default();
        *self.entry.lock() = Some(Entry {
            columns: columns.clone(),
            at: now,
            settled,
        });
        columns
    }

    /// Drop the remembered answer, so the next read probes again. A list query
    /// that fails calls this: the table may have changed under the answer.
    pub(crate) fn forget(&self) {
        *self.entry.lock() = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rows(names: &[&str]) -> Vec<Value> {
        names.iter().map(|name| json!({ "name": name })).collect()
    }

    fn every_name() -> Vec<&'static str> {
        OPTIONAL_COLUMNS.iter().map(|column| column.name).collect()
    }

    #[test]
    fn a_table_with_every_column_misses_none() {
        let mut names = vec!["ts", "request_id"];
        names.extend(every_name());
        let columns = Columns::from_rows(&rows(&names)).expect("the table is visible");
        assert!(columns.complete());
        for name in every_name() {
            assert_eq!(columns.select(name), name);
        }
    }

    #[test]
    fn a_table_from_before_the_migrations_misses_each_optional_column() {
        let columns = Columns::from_rows(&rows(&["ts", "request_id", "status"]))
            .expect("the table is visible");
        assert!(!columns.complete());
        assert_eq!(columns.missing(), every_name());
        // typed defaults under the column's own name, so the row reads alike
        assert_eq!(
            columns.select("upstream_status"),
            "toUInt16(0) as upstream_status"
        );
        assert_eq!(columns.select("attempts"), "toUInt8(0) as attempts");
        assert_eq!(
            columns.select("lifecycle_operation"),
            "'' as lifecycle_operation"
        );
        assert_eq!(
            columns.select("cache_write_1h_tokens"),
            "toUInt32(0) as cache_write_1h_tokens"
        );
    }

    #[test]
    fn only_the_missing_column_is_replaced() {
        let columns = Columns::from_rows(&rows(&[
            "ts",
            "upstream_status",
            "attempts",
            "lifecycle_operation",
        ]))
        .expect("the table is visible");
        assert_eq!(columns.missing(), ["cache_write_1h_tokens"]);
        assert_eq!(columns.select("attempts"), "attempts");
        assert_eq!(columns.select("lifecycle_operation"), "lifecycle_operation");
        assert_eq!(
            columns.select("cache_write_1h_tokens"),
            "toUInt32(0) as cache_write_1h_tokens"
        );
    }

    #[test]
    fn a_column_outside_the_list_is_never_replaced() {
        let columns = Columns::from_rows(&rows(&["ts"])).expect("the table is visible");
        assert_eq!(columns.select("log_id"), "log_id");
        assert_eq!(columns.select("cache_write_tokens"), "cache_write_tokens");
    }

    #[test]
    fn a_table_the_probe_cannot_see_is_not_a_table_without_the_columns() {
        // no grant, no table, or a stand-in server that answers nothing: the
        // honest reading is "unknown", which selects as before
        assert_eq!(Columns::from_rows(&[]), None);
        assert_eq!(Columns::from_rows(&[json!({"other": 1})]), None);
    }

    #[test]
    fn the_defaults_are_typed_and_distinct_per_column() {
        for column in OPTIONAL_COLUMNS {
            assert!(!column.default.is_empty(), "{}", column.name);
        }
        let mut names = every_name();
        names.sort_unstable();
        names.dedup();
        assert_eq!(
            names.len(),
            OPTIONAL_COLUMNS.len(),
            "a column is listed twice"
        );
    }

    #[test]
    fn an_answer_is_kept_until_its_lifetime_ends() {
        let cache = ColumnCache::with_lifetimes(Duration::from_secs(600), Duration::from_secs(60));
        let start = Instant::now();
        assert_eq!(cache.fresh(start), None, "nothing is remembered yet");

        cache.store(start, Some(Columns::lacking(&["cache_write_1h_tokens"])));
        assert_eq!(
            cache.fresh(start + Duration::from_secs(59)),
            Some(Columns::lacking(&["cache_write_1h_tokens"]))
        );
        // a missing column is looked for again within a minute, so applying
        // the file shows without a restart
        assert_eq!(cache.fresh(start + Duration::from_secs(60)), None);
    }

    #[test]
    fn a_complete_answer_is_kept_longer_than_an_incomplete_one() {
        let cache = ColumnCache::with_lifetimes(Duration::from_secs(600), Duration::from_secs(60));
        let start = Instant::now();
        cache.store(start, Some(Columns::default()));
        assert!(cache.fresh(start + Duration::from_secs(599)).is_some());
        assert!(cache.fresh(start + Duration::from_secs(600)).is_none());
    }

    #[test]
    fn an_unreadable_probe_is_remembered_briefly_as_assume_present() {
        let cache = ColumnCache::with_lifetimes(Duration::from_secs(600), Duration::from_secs(60));
        let start = Instant::now();
        let stored = cache.store(start, None);
        assert_eq!(stored, Columns::default());
        assert!(cache.fresh(start + Duration::from_secs(30)).is_some());
        // and not for the long lifetime, since nothing was settled
        assert!(cache.fresh(start + Duration::from_secs(61)).is_none());
    }

    #[test]
    fn forgetting_drops_the_answer() {
        let cache = ColumnCache::new();
        let start = Instant::now();
        cache.store(start, Some(Columns::default()));
        assert!(cache.fresh(start).is_some());
        cache.forget();
        assert!(cache.fresh(start).is_none());
    }
}
