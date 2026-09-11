//! Does the configured KEK still open what the store already holds? (#923)
//!
//! A `pg_dump` is trivially restorable; the KEK is not part of it. Restore a
//! dump onto a deployment whose `ROLTER_KEK` differs by one character and every
//! row here is permanently unreadable — but nothing fails at restore time. The
//! process boots, the dashboard renders, and the first symptom is an upstream
//! 401 hours later, attributed to the provider rather than to the restore.
//!
//! So the store offers a cheap read-only audit: sample the sealed columns and
//! try to open them. It is the difference between "the KEK is set" (which
//! [`super::crypto`] and `rolter check` already know) and "the KEK is the *same
//! one that sealed this database*", which only the data can answer.

use sqlx::PgPool;

use rolter_core::Result;

use super::crypto::Kek;
use super::store_err;

/// One `(ciphertext, nonce)` pair in the schema.
///
/// Sealed secrets are spread across features rather than centralised, so this
/// list is the inventory. A new sealed column that is not added here is not
/// audited — see the maintenance-matrix row in `AGENTS.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SealedColumn {
    /// table holding the pair
    pub table: &'static str,
    /// `bytea` column holding the AES-256-GCM ciphertext
    pub ciphertext: &'static str,
    /// `bytea` column holding that ciphertext's 96-bit nonce
    pub nonce: &'static str,
    /// what an operator loses when this column cannot be opened
    pub holds: &'static str,
}

/// Every sealed column in the schema, in the order an operator feels the loss.
pub const SEALED_COLUMNS: &[SealedColumn] = &[
    SealedColumn {
        table: "provider_keys",
        ciphertext: "ciphertext",
        nonce: "nonce",
        holds: "upstream provider credentials",
    },
    SealedColumn {
        table: "sso_providers",
        ciphertext: "secret_ciphertext",
        nonce: "secret_nonce",
        holds: "SSO client secrets",
    },
    SealedColumn {
        table: "alert_channels",
        ciphertext: "secret_ciphertext",
        nonce: "secret_nonce",
        holds: "alert channel webhook secrets",
    },
    SealedColumn {
        table: "security_settings",
        ciphertext: "dashboard_credential_ciphertext",
        nonce: "dashboard_credential_nonce",
        holds: "the dashboard's own upstream credential",
    },
    SealedColumn {
        table: "observability_connectors",
        ciphertext: "auth_ciphertext",
        nonce: "auth_nonce",
        holds: "observability connector credentials",
    },
    SealedColumn {
        table: "user_totp_factors",
        ciphertext: "secret_ciphertext",
        nonce: "secret_nonce",
        holds: "TOTP second-factor shared secrets",
    },
    SealedColumn {
        table: "mcp_servers",
        ciphertext: "client_secret_ciphertext",
        nonce: "client_secret_nonce",
        holds: "MCP OAuth client secrets",
    },
    SealedColumn {
        table: "mcp_servers",
        ciphertext: "credential_ciphertext",
        nonce: "credential_nonce",
        holds: "MCP static bearer tokens and header api keys",
    },
    SealedColumn {
        table: "mcp_oauth_sessions",
        ciphertext: "access_ciphertext",
        nonce: "access_nonce",
        holds: "MCP access tokens",
    },
    SealedColumn {
        table: "mcp_oauth_login_states",
        ciphertext: "verifier_ciphertext",
        nonce: "verifier_nonce",
        holds: "in-flight MCP OAuth PKCE verifiers",
    },
    SealedColumn {
        table: "mcp_oauth_sessions",
        ciphertext: "refresh_ciphertext",
        nonce: "refresh_nonce",
        holds: "MCP refresh tokens",
    },
];

/// Rows sampled per column. The audit answers a yes/no question — a KEK either
/// sealed this database or it did not — so reading every row would buy nothing
/// and would make the check's cost scale with the deployment it gates.
pub const SAMPLE_LIMIT: i64 = 25;

/// What one sealed column looked like when sampled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnAudit {
    /// the column that was sampled
    pub column: SealedColumn,
    /// rows read
    pub sampled: usize,
    /// rows the KEK opened
    pub opened: usize,
}

impl ColumnAudit {
    /// Whether every sampled row opened.
    pub fn is_intact(&self) -> bool {
        self.opened == self.sampled
    }
}

/// The whole audit. Columns with no sealed rows are omitted: an empty column
/// is not evidence either way, and reporting it as "intact" would let a restore
/// with no secrets at all look verified.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KekAudit {
    /// per-column results, only for columns that held at least one row
    pub columns: Vec<ColumnAudit>,
}

impl KekAudit {
    /// Sealed rows read across every column.
    pub fn sampled(&self) -> usize {
        self.columns.iter().map(|c| c.sampled).sum()
    }

    /// Sealed rows the configured KEK opened.
    pub fn opened(&self) -> usize {
        self.columns.iter().map(|c| c.opened).sum()
    }

    /// Columns that held rows the KEK could not open.
    pub fn damaged(&self) -> Vec<&ColumnAudit> {
        self.columns.iter().filter(|c| !c.is_intact()).collect()
    }

    /// Whether the KEK opened everything it was shown. Vacuously true for a
    /// store that holds no secrets — see [`Self::sampled`] to tell the two
    /// apart before reporting a restore as verified.
    pub fn is_intact(&self) -> bool {
        self.damaged().is_empty()
    }

    /// One line per damaged column, naming what the operator has lost.
    pub fn damage_report(&self) -> Vec<String> {
        self.damaged()
            .iter()
            .map(|c| {
                format!(
                    "{}.{} holds {} — {} of {} sampled rows could not be decrypted",
                    c.column.table,
                    c.column.ciphertext,
                    c.column.holds,
                    c.sampled - c.opened,
                    c.sampled
                )
            })
            .collect()
    }
}

/// Sample the sealed columns and report which ones `kek` opens.
///
/// Read-only and cheap by construction: at most [`SAMPLE_LIMIT`] rows per
/// column, decrypted in process and dropped. Nothing is logged and no plaintext
/// leaves this function — the result carries counts, never secrets.
///
/// A column whose table or column does not exist is skipped rather than
/// failing. That is deliberate: this runs *before* boot, which is exactly when
/// a restored database may be a migration or two behind the binary reading it.
pub async fn audit_kek(pool: &PgPool, kek: &Kek) -> Result<KekAudit> {
    let mut columns = Vec::new();
    for column in SEALED_COLUMNS {
        if !column_exists(pool, column).await? {
            continue;
        }
        let sql = format!(
            "select {}, {} from {} where {} is not null and {} is not null limit {SAMPLE_LIMIT}",
            column.ciphertext, column.nonce, column.table, column.ciphertext, column.nonce
        );
        let rows: Vec<(Vec<u8>, Vec<u8>)> = sqlx::query_as(&sql)
            .fetch_all(pool)
            .await
            .map_err(store_err)?;
        if rows.is_empty() {
            continue;
        }
        let opened = rows
            .iter()
            .filter(|(ciphertext, nonce)| kek.decrypt(ciphertext, nonce).is_ok())
            .count();
        columns.push(ColumnAudit {
            column: *column,
            sampled: rows.len(),
            opened,
        });
    }
    Ok(KekAudit { columns })
}

/// Open a one-connection pool at `database_url` and run [`audit_kek`] on it.
///
/// The convenience the pre-boot caller needs: `rolter check` gates a process
/// that has not started, so it has no pool of its own to borrow and no reason
/// to build the full [`super::PostgresConfigStore`] — which runs migrations,
/// something a check must never do to a database it was only asked to inspect.
pub async fn audit_kek_at(
    database_url: &str,
    kek: &Kek,
    timeout: std::time::Duration,
) -> Result<KekAudit> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(timeout)
        .connect(database_url)
        .await
        .map_err(store_err)?;
    let result = audit_kek(&pool, kek).await;
    pool.close().await;
    result
}

/// What a rotation did, per column.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KekRotation {
    /// `(table, ciphertext column, rows resealed)`, only for columns that held
    /// something to reseal
    pub resealed: Vec<(&'static str, &'static str, usize)>,
}

impl KekRotation {
    /// Rows resealed across every column.
    pub fn total(&self) -> usize {
        self.resealed.iter().map(|(_, _, n)| n).sum()
    }
}

/// Reseal every sealed secret in the store from `from` to `to`.
///
/// Rotation is a whole-database operation or it is nothing: a partial rotation
/// leaves a store no single KEK can open, which is strictly worse than either
/// key alone. So the entire pass runs in one transaction, rows are locked as
/// they are read, and a single row that `from` cannot open aborts everything —
/// that row is the evidence that `from` is not the KEK this store was sealed
/// with, and resealing the rest around it would manufacture exactly the split
/// this function exists to avoid.
///
/// The caller is expected to have stopped the writers first; the row locks stop
/// a concurrent write from being resealed twice, not from being lost.
pub async fn rotate_kek(pool: &PgPool, from: &Kek, to: &Kek) -> Result<KekRotation> {
    let mut tx = pool.begin().await.map_err(store_err)?;
    let mut resealed = Vec::new();

    for column in SEALED_COLUMNS {
        if !column_exists(pool, column).await? {
            continue;
        }
        // `ctid` addresses a row without assuming a primary key, which the
        // sealed tables do not share the shape of. It is only stable while the
        // row is locked, which `for update` inside this transaction guarantees
        let sql = format!(
            "select ctid::text, {}, {} from {} where {} is not null and {} is not null for update",
            column.ciphertext, column.nonce, column.table, column.ciphertext, column.nonce
        );
        let rows: Vec<(String, Vec<u8>, Vec<u8>)> = sqlx::query_as(&sql)
            .fetch_all(&mut *tx)
            .await
            .map_err(store_err)?;
        if rows.is_empty() {
            continue;
        }
        for (ctid, ciphertext, nonce) in &rows {
            let plaintext = from.decrypt(ciphertext, nonce).map_err(|_| {
                rolter_core::Error::Store(format!(
                    "the old KEK does not open {}.{}; nothing was rotated. Rotation is \
                     all-or-nothing on purpose — a half-rotated store is readable by neither \
                     key",
                    column.table, column.ciphertext
                ))
            })?;
            let (new_ciphertext, new_nonce) = to.encrypt(&plaintext)?;
            let update = format!(
                "update {} set {} = $1, {} = $2 where ctid = $3::tid",
                column.table, column.ciphertext, column.nonce
            );
            sqlx::query(&update)
                .bind(&new_ciphertext)
                .bind(&new_nonce)
                .bind(ctid)
                .execute(&mut *tx)
                .await
                .map_err(store_err)?;
        }
        resealed.push((column.table, column.ciphertext, rows.len()));
    }

    tx.commit().await.map_err(store_err)?;
    Ok(KekRotation { resealed })
}

/// Whether the table and column both exist in the current `search_path`.
async fn column_exists(pool: &PgPool, column: &SealedColumn) -> Result<bool> {
    sqlx::query_scalar::<_, bool>(
        "select exists(
             select 1 from information_schema.columns
             where table_schema = any(current_schemas(false))
               and table_name = $1 and column_name = $2
         )",
    )
    .bind(column.table)
    .bind(column.ciphertext)
    .fetch_one(pool)
    .await
    .map_err(store_err)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn audit(sampled: usize, opened: usize) -> ColumnAudit {
        ColumnAudit {
            column: SEALED_COLUMNS[0],
            sampled,
            opened,
        }
    }

    #[test]
    fn a_store_that_holds_no_secrets_is_intact_but_proves_nothing() {
        let empty = KekAudit::default();
        assert!(empty.is_intact());
        assert_eq!(
            empty.sampled(),
            0,
            "callers must be able to tell the two apart"
        );
        assert!(empty.damage_report().is_empty());
    }

    #[test]
    fn one_unreadable_row_damages_the_whole_column() {
        let result = KekAudit {
            columns: vec![audit(25, 24)],
        };
        assert!(!result.is_intact());
        assert_eq!(result.sampled(), 25);
        assert_eq!(result.opened(), 24);
        let report = result.damage_report();
        assert_eq!(report.len(), 1);
        assert!(
            report[0].contains("provider_keys.ciphertext")
                && report[0].contains("upstream provider credentials")
                && report[0].contains("1 of 25"),
            "the report must name the column and what was lost: {}",
            report[0]
        );
    }

    #[test]
    fn every_sealed_column_names_what_it_holds() {
        for column in SEALED_COLUMNS {
            assert!(
                !column.holds.is_empty(),
                "{} has no description",
                column.table
            );
            assert!(
                column.nonce.contains("nonce"),
                "{}.{} does not look like a nonce column",
                column.table,
                column.nonce
            );
        }
    }

    // --- against a real database ---------------------------------------
    //
    // These are the ones that answer #923: the point is not that AES works, it
    // is that ciphertext survives a move between databases and that the *only*
    // thing that decides whether it is still readable travelled separately.

    use sqlx::PgPool;

    fn database_url() -> Option<String> {
        std::env::var("ROLTER_TEST_DATABASE_URL").ok()
    }

    /// Seal `secret` for a freshly created provider, as the dashboard does.
    async fn seed_sealed_provider(pool: &PgPool, kek: &Kek, slug: &str, secret: &str) {
        let org_id: uuid::Uuid =
            sqlx::query_scalar("insert into orgs (name, slug) values ($1, $1) returning id")
                .bind(slug)
                .fetch_one(pool)
                .await
                .expect("seed org");
        let provider_id: uuid::Uuid = sqlx::query_scalar(
            "insert into providers (org_id, name, slug, kind, api_base)
             values ($1, $2, $2, 'openai', 'https://api.openai.com') returning id",
        )
        .bind(org_id)
        .bind(slug)
        .fetch_one(pool)
        .await
        .expect("seed provider");
        let (ciphertext, nonce) = kek.encrypt(secret).expect("seal");
        sqlx::query(
            "insert into provider_keys (provider_id, ciphertext, nonce) values ($1, $2, $3)",
        )
        .bind(provider_id)
        .bind(&ciphertext)
        .bind(&nonce)
        .execute(pool)
        .await
        .expect("store sealed key");
    }

    /// The restore, at the level that matters here: sealed rows move into a
    /// database that was migrated from scratch and has never seen the KEK.
    async fn copy_provider_keys(from: &PgPool, into: &PgPool) {
        let rows: Vec<(uuid::Uuid, Vec<u8>, Vec<u8>)> =
            sqlx::query_as("select provider_id, ciphertext, nonce from provider_keys")
                .fetch_all(from)
                .await
                .expect("read sealed rows");
        assert!(!rows.is_empty(), "nothing to restore");
        for (provider_id, ciphertext, nonce) in rows {
            let org_id: uuid::Uuid = sqlx::query_scalar(
                "insert into orgs (name, slug) values ('restored', 'restored') returning id",
            )
            .fetch_one(into)
            .await
            .expect("restore org");
            sqlx::query(
                "insert into providers (id, org_id, name, slug, kind, api_base)
                 values ($1, $2, 'openai', 'openai', 'openai', 'https://api.openai.com')",
            )
            .bind(provider_id)
            .bind(org_id)
            .execute(into)
            .await
            .expect("restore provider");
            sqlx::query(
                "insert into provider_keys (provider_id, ciphertext, nonce) values ($1, $2, $3)",
            )
            .bind(provider_id)
            .bind(&ciphertext)
            .bind(&nonce)
            .execute(into)
            .await
            .expect("restore sealed row");
        }
    }

    #[tokio::test]
    async fn a_restore_carrying_its_kek_still_opens_every_secret() {
        let Some(url) = database_url() else {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
            return;
        };
        let kek = Kek::from_secret("the-kek-that-sealed-this-database");
        let original_db = crate::postgres::test_schema::TestSchema::migrated(&url).await;
        let original = original_db.pool().clone();
        seed_sealed_provider(&original, &kek, "acme", "sk-upstream-secret").await;

        let restored_db = crate::postgres::test_schema::TestSchema::migrated(&url).await;
        let restored = restored_db.pool().clone();
        copy_provider_keys(&original, &restored).await;

        let audit = audit_kek(&restored, &kek).await.expect("audit");
        assert!(audit.is_intact(), "{:?}", audit.damage_report());
        assert_eq!(
            audit.sampled(),
            1,
            "the audit must actually have read a row"
        );
        assert_eq!(audit.opened(), 1);

        // and the secret itself, not only the count
        let (ciphertext, nonce): (Vec<u8>, Vec<u8>) =
            sqlx::query_as("select ciphertext, nonce from provider_keys")
                .fetch_one(&restored)
                .await
                .expect("read restored row");
        assert_eq!(
            kek.decrypt(&ciphertext, &nonce).unwrap(),
            "sk-upstream-secret"
        );
    }

    #[tokio::test]
    async fn a_restore_onto_the_wrong_kek_is_caught_and_names_what_was_lost() {
        let Some(url) = database_url() else {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
            return;
        };
        let sealed_with = Kek::from_secret("the-kek-that-sealed-this-database");
        let original_db = crate::postgres::test_schema::TestSchema::migrated(&url).await;
        let original = original_db.pool().clone();
        seed_sealed_provider(&original, &sealed_with, "acme", "sk-upstream-secret").await;

        let restored_db = crate::postgres::test_schema::TestSchema::migrated(&url).await;
        let restored = restored_db.pool().clone();
        copy_provider_keys(&original, &restored).await;

        // the whole failure mode of #923: the dump restored cleanly, the KEK
        // did not travel with it, and nothing else in the system notices
        let wrong = Kek::from_secret("a-different-deployments-kek");
        let audit = audit_kek(&restored, &wrong).await.expect("audit");
        assert!(!audit.is_intact(), "a wrong KEK must not read as verified");
        assert_eq!(audit.sampled(), 1);
        assert_eq!(audit.opened(), 0);
        let report = audit.damage_report().join("\n");
        assert!(
            report.contains("provider_keys") && report.contains("upstream provider credentials"),
            "the report must name the table and the loss, got: {report}"
        );
    }

    #[tokio::test]
    async fn a_store_with_nothing_sealed_yet_reports_nothing_sampled() {
        let Some(url) = database_url() else {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
            return;
        };
        let pool_db = crate::postgres::test_schema::TestSchema::migrated(&url).await;
        let pool = pool_db.pool().clone();
        let audit = audit_kek(&pool, &Kek::from_secret("any-kek-at-all"))
            .await
            .expect("audit");
        assert!(audit.is_intact());
        assert_eq!(
            audit.sampled(),
            0,
            "an empty store must not let any KEK look verified"
        );
    }

    #[tokio::test]
    async fn every_column_in_the_inventory_exists_in_the_schema() {
        let Some(url) = database_url() else {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
            return;
        };
        let pool_db = crate::postgres::test_schema::TestSchema::migrated(&url).await;
        let pool = pool_db.pool().clone();
        for column in SEALED_COLUMNS {
            assert!(
                column_exists(&pool, column).await.expect("probe column"),
                "{}.{} is in the inventory but not in the schema — a sealed column was \
                 renamed or dropped without updating SEALED_COLUMNS",
                column.table,
                column.ciphertext
            );
            let nonce_present: bool = sqlx::query_scalar(
                "select exists(
                     select 1 from information_schema.columns
                     where table_schema = any(current_schemas(false))
                       and table_name = $1 and column_name = $2
                 )",
            )
            .bind(column.table)
            .bind(column.nonce)
            .fetch_one(&pool)
            .await
            .expect("probe nonce column");
            assert!(
                nonce_present,
                "{}.{} is missing its nonce column {}",
                column.table, column.ciphertext, column.nonce
            );
        }
    }

    /// Whether `pg_dump` and `psql` are both on PATH.
    fn dump_tools_available() -> bool {
        ["pg_dump", "psql"].iter().all(|bin| {
            std::process::Command::new(bin)
                .arg("--version")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        })
    }

    /// The real thing, when the client tools are installed: a `pg_dump` of a
    /// seeded store, replayed into a database migrated from scratch.
    ///
    /// The logical copy above proves the property; this proves the *procedure*
    /// the runbook tells operators to run. It self-skips rather than failing
    /// where `pg_dump` is absent, because the property is already covered and a
    /// missing client binary is not a defect in rolter.
    #[tokio::test]
    async fn a_pg_dump_restored_into_a_fresh_database_keeps_its_secrets_readable() {
        let Some(url) = database_url() else {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
            return;
        };
        if !dump_tools_available() {
            eprintln!("skipping: pg_dump/psql not on PATH");
            return;
        }
        let kek = Kek::from_secret("the-kek-that-sealed-this-database");
        let original_db = crate::postgres::test_schema::TestSchema::migrated(&url).await;
        let original = original_db.pool().clone();
        let source_schema = original_db.schema().to_string();
        seed_sealed_provider(&original, &kek, "acme", "sk-upstream-secret").await;

        let restored_db = crate::postgres::test_schema::TestSchema::migrated(&url).await;
        let restored = restored_db.pool().clone();
        let target_schema = restored_db.schema().to_string();
        // a data-only dump: the target was migrated from scratch, which is what
        // the runbook says to do so a restore cannot resurrect an old schema
        let mut dump = std::process::Command::new("pg_dump");
        dump.arg(&url).arg("--data-only");
        // only the three tables this drill is about. A whole-schema data dump
        // would also carry `_sqlx_migrations`, whose rows the freshly migrated
        // target already has — the restore would abort on a duplicate key and
        // say nothing about KEKs
        for table in ["orgs", "providers", "provider_keys"] {
            dump.arg("--table").arg(format!("{source_schema}.{table}"));
        }
        let dump = dump.output().expect("run pg_dump");
        if !dump.status.success() {
            let stderr = String::from_utf8_lossy(&dump.stderr);
            // a client older than the server refuses outright. That is a
            // property of the runner, not of rolter, and the logical copy above
            // already covers the behaviour — so skip rather than fail red
            if stderr.contains("server version") {
                eprintln!("skipping: {}", stderr.trim());
                return;
            }
            panic!("pg_dump failed: {stderr}");
        }
        // schema names are unique per process and call, so this rewrite cannot
        // collide with anything else in the dump
        let sql = String::from_utf8(dump.stdout)
            .expect("dump is utf-8")
            .replace(&format!("{source_schema}."), &format!("{target_schema}."))
            // pg_dump empties the search_path so its own object names are
            // unambiguous. A real restore into a real database is unaffected,
            // but these tables live in an isolated schema, and the
            // bump_config_version() trigger they fire looks `config_version` up
            // by bare name — point it at the target
            .replace(
                "set_config('search_path', '', false)",
                &format!("set_config('search_path', '{target_schema}', false)"),
            );

        let mut psql = std::process::Command::new("psql")
            .arg(&url)
            .arg("--quiet")
            .arg("--no-psqlrc")
            .arg("-v")
            .arg("ON_ERROR_STOP=1")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("spawn psql");
        {
            use std::io::Write as _;
            psql.stdin
                .as_mut()
                .expect("psql stdin")
                .write_all(sql.as_bytes())
                .expect("feed psql");
        }
        let status = psql.wait().expect("await psql");
        assert!(status.success(), "restoring the dump failed");

        let audit = audit_kek(&restored, &kek).await.expect("audit");
        assert!(audit.is_intact(), "{:?}", audit.damage_report());
        assert_eq!(audit.sampled(), 1, "the dump carried no sealed rows");

        let wrong = Kek::from_secret("a-different-deployments-kek");
        let audit = audit_kek(&restored, &wrong).await.expect("audit");
        assert!(
            !audit.is_intact(),
            "a dump restored under a different KEK must be reported, not booted into"
        );
    }

    #[tokio::test]
    async fn rotation_hands_the_store_to_the_new_kek_and_takes_it_from_the_old() {
        let Some(url) = database_url() else {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
            return;
        };
        let old = Kek::from_secret("the-kek-that-sealed-this-database");
        let new = Kek::from_secret("the-kek-we-are-rotating-to");
        let pool_db = crate::postgres::test_schema::TestSchema::migrated(&url).await;
        let pool = pool_db.pool().clone();
        seed_sealed_provider(&pool, &old, "acme", "sk-upstream-secret").await;
        seed_sealed_provider(&pool, &old, "globex", "sk-second-secret").await;

        let rotation = rotate_kek(&pool, &old, &new).await.expect("rotate");
        assert_eq!(rotation.total(), 2, "both sealed rows must be resealed");
        assert!(rotation
            .resealed
            .iter()
            .any(|(table, _, n)| *table == "provider_keys" && *n == 2));

        assert!(audit_kek(&pool, &new).await.unwrap().is_intact());
        let after_old = audit_kek(&pool, &old).await.unwrap();
        assert!(
            !after_old.is_intact(),
            "the old KEK must stop working, or the rotation bought nothing"
        );

        // the plaintext survived the round trip, which is the whole point
        let secrets: Vec<(Vec<u8>, Vec<u8>)> =
            sqlx::query_as("select ciphertext, nonce from provider_keys")
                .fetch_all(&pool)
                .await
                .unwrap();
        let mut opened: Vec<String> = secrets
            .iter()
            .map(|(c, n)| new.decrypt(c, n).expect("new KEK opens it"))
            .collect();
        opened.sort();
        assert_eq!(opened, vec!["sk-second-secret", "sk-upstream-secret"]);
    }

    #[tokio::test]
    async fn a_rotation_from_the_wrong_old_kek_changes_nothing_at_all() {
        let Some(url) = database_url() else {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
            return;
        };
        let sealed_with = Kek::from_secret("the-kek-that-sealed-this-database");
        let pool_db = crate::postgres::test_schema::TestSchema::migrated(&url).await;
        let pool = pool_db.pool().clone();
        seed_sealed_provider(&pool, &sealed_with, "acme", "sk-upstream-secret").await;
        let before: (Vec<u8>, Vec<u8>) =
            sqlx::query_as("select ciphertext, nonce from provider_keys")
                .fetch_one(&pool)
                .await
                .unwrap();

        let wrong_old = Kek::from_secret("a-different-deployments-kek");
        let new = Kek::from_secret("the-kek-we-are-rotating-to");
        let error = rotate_kek(&pool, &wrong_old, &new)
            .await
            .expect_err("rotating from a KEK that does not open the store must fail");
        assert!(
            error.to_string().contains("provider_keys"),
            "the error must name where it stopped, got: {error}"
        );

        // the transaction rolled back: the store is exactly as it was, still
        // readable by the KEK that sealed it
        let after: (Vec<u8>, Vec<u8>) =
            sqlx::query_as("select ciphertext, nonce from provider_keys")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            before, after,
            "a failed rotation must not touch a single row"
        );
        assert!(audit_kek(&pool, &sealed_with).await.unwrap().is_intact());
    }

    /// The inventory is only as good as its coverage, so ask the schema
    /// directly: any `*_ciphertext`-shaped column the audit does not know about
    /// is a secret a restore would silently lose without warning.
    #[tokio::test]
    async fn the_schema_holds_no_sealed_column_the_audit_does_not_know_about() {
        let Some(url) = database_url() else {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
            return;
        };
        let pool_db = crate::postgres::test_schema::TestSchema::migrated(&url).await;
        let pool = pool_db.pool().clone();
        let found: Vec<(String, String)> = sqlx::query_as(
            "select table_name, column_name from information_schema.columns
             where table_schema = any(current_schemas(false))
               and data_type = 'bytea'
               and (column_name = 'ciphertext' or column_name like '%\\_ciphertext')
             order by table_name, column_name",
        )
        .fetch_all(&pool)
        .await
        .expect("list sealed columns");
        let unknown: Vec<String> = found
            .iter()
            .filter(|(table, column)| {
                !SEALED_COLUMNS
                    .iter()
                    .any(|known| known.table == table && known.ciphertext == column)
            })
            .map(|(table, column)| format!("{table}.{column}"))
            .collect();
        assert!(
            unknown.is_empty(),
            "sealed columns missing from SEALED_COLUMNS: {unknown:?}. Add them, or a KEK \
             mismatch will go unreported for whatever they hold."
        );
    }

    #[test]
    fn the_inventory_lists_each_pair_once() {
        let mut seen: Vec<(&str, &str)> = SEALED_COLUMNS
            .iter()
            .map(|c| (c.table, c.ciphertext))
            .collect();
        let before = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(before, seen.len(), "a sealed column is listed twice");
    }
}
