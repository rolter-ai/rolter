use super::*;
use rolter_core::Error;
use uuid::Uuid;

use crate::postgres::models::RouteTarget;
// the mcp domain took the only non-test use of `Duration` with it
use chrono::{Duration, Utc};

/// An isolated, migrated schema of this test's own. Bind the guard for the
/// whole test: the schema is dropped with it.
async fn fresh_db() -> super::super::test_schema::TestSchema {
    let url = super::super::test_database::url()
        .await
        .expect("ROLTER_TEST_DATABASE_URL not set; skipping");
    super::super::test_schema::TestSchema::migrated(&url).await
}

#[tokio::test]
async fn crud_roundtrip_across_the_tenancy_and_routing_tables() {
    if !super::super::test_database::is_configured() {
        eprintln!("skipping: {} not set", super::super::test_database::URL_ENV);
        return;
    }
    let db = fresh_db().await;
    let pool = db.pool().clone();

    let orgs = OrgRepo(&pool);
    let org = orgs.create("acme", "acme").await.unwrap();
    assert_eq!(orgs.get(org.id).await.unwrap().slug, "acme");
    assert_eq!(orgs.list().await.unwrap().len(), 1);

    let teams = TeamRepo(&pool);
    let team = teams.create(org.id, "core").await.unwrap();
    assert_eq!(teams.list(org.id).await.unwrap().len(), 1);

    let projects = ProjectRepo(&pool);
    let project = projects.create(team.id, "default").await.unwrap();
    assert_eq!(projects.list(team.id).await.unwrap().len(), 1);

    let providers = ProviderRepo(&pool);
    let provider = providers
        .create(
            org.id,
            NewProvider {
                name: "openai",
                slug: "openai",
                kind: "openai",
                api_base: "https://api.openai.com",
                api_key_env: Some("OPENAI_API_KEY"),
                egress_proxy: None,
                egress_proxies: &[],
                kv_events: None,
                lmcache: None,
                project_id: None,
                allow_custom_api_base: false,
            },
        )
        .await
        .unwrap();
    assert_eq!(providers.list(org.id).await.unwrap().len(), 1);

    let routes = RouteRepo(&pool);
    let route = routes
        .create(project.id, "gpt-4o", "power_of_two")
        .await
        .unwrap();
    assert!(route.enabled);
    let disabled = routes.set_enabled(route.id, false).await.unwrap();
    assert!(!disabled.enabled);

    let targets = RouteTargetRepo(&pool);
    let target = targets
        .create(route.id, provider.id, Some("gpt-4o-2024-08-06"), 2, &[])
        .await
        .unwrap();
    assert_eq!(targets.list(route.id).await.unwrap().len(), 1);
    targets.delete(target.id).await.unwrap();
    assert!(targets.list(route.id).await.unwrap().is_empty());

    let keys = VirtualKeyRepo(&pool);
    let vk = keys
        .create(
            project.id,
            NewVirtualKey {
                key_hash: "hash123",
                key_prefix: "sk-abc",
                name: Some("ci key"),
                models: &["gpt-4o".to_string()],
                providers: &[],
                cache_enabled: None,
                created_by: None,
                expires_at: Some(Utc::now() + Duration::days(30)),
                purpose: None,
            },
        )
        .await
        .unwrap();
    // defaults to inherit-the-route (NULL) on create
    assert_eq!(vk.cache_enabled, None);
    // the ttl the caller chose reaches the column the gateway reads, so a
    // minted key is not immortal by construction (#945)
    assert!(vk.expires_at.is_some());
    assert_eq!(
        keys.find_by_hash("hash123").await.unwrap().map(|k| k.id),
        Some(vk.id)
    );
    let disabled_key = keys.set_disabled(vk.id, true).await.unwrap();
    assert!(disabled_key.disabled);
    // the cache override round-trips: force off, then clear back to inherit
    let off = keys.set_cache(vk.id, Some(false)).await.unwrap();
    assert_eq!(off.cache_enabled, Some(false));
    let cleared = keys.set_cache(vk.id, None).await.unwrap();
    assert_eq!(cleared.cache_enabled, None);

    let budgets = BudgetRepo(&pool);
    let budget = budgets
        .create(
            "project",
            project.id,
            "100.5000",
            "30d",
            None,
            Within::default(),
        )
        .await
        .unwrap();
    assert_eq!(budget.limit_usd, "100.5000");
    // no override means "inherit the deployment-wide unpriced policy", and
    // it round-trips as null rather than as a guessed default (#996)
    assert_eq!(budget.unpriced_policy, None);
    let strict = budgets
        .create(
            "project",
            project.id,
            "10.0000",
            "30d",
            Some("block"),
            Within::default(),
        )
        .await
        .unwrap();
    assert_eq!(strict.unpriced_policy, Some("block".to_string()));
    assert_eq!(
        budgets.get(strict.id).await.unwrap().unpriced_policy,
        Some("block".to_string())
    );
    // both budgets belong to the project scope, the inherited one and the
    // strict one, so the scope lists two
    assert_eq!(
        budgets
            .list_for_scope("project", project.id)
            .await
            .unwrap()
            .len(),
        2
    );

    let limits = RateLimitRepo(&pool);
    let limit = limits
        .create(
            "project",
            project.id,
            Some(60),
            Some(100_000),
            Within::default(),
        )
        .await
        .unwrap();
    assert_eq!(limit.rpm, Some(60));
    // a limit with no positive cap admits everything, so it is never
    // stored, whichever way the caps are missing (#1903)
    for (rpm, tpm) in [
        (None, None),
        (Some(0), None),
        (None, Some(-5)),
        (Some(0), Some(0)),
    ] {
        assert!(
            matches!(
                limits
                    .create("project", project.id, rpm, tpm, Within::default())
                    .await,
                Err(Error::Config(_))
            ),
            "{rpm:?} / {tpm:?} should be refused"
        );
    }
    assert_eq!(
        limits
            .list_for_scope("project", project.id)
            .await
            .unwrap()
            .len(),
        1
    );

    let prices = ModelPriceRepo(&pool);
    let price = prices
        .upsert(NewModelPrice {
            model: "gpt-4o",
            input_per_mtok: "2.500000",
            output_per_mtok: "10.000000",
            cached_input_per_mtok: None,
            cache_write_per_mtok: None,
            cache_write_1h_per_mtok: None,
            currency: "USD",
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(price.input_per_mtok, "2.500000");
    let updated = prices
        .upsert(NewModelPrice {
            model: "gpt-4o",
            input_per_mtok: "3.000000",
            output_per_mtok: "10.000000",
            cached_input_per_mtok: None,
            cache_write_per_mtok: None,
            cache_write_1h_per_mtok: None,
            currency: "USD",
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(updated.input_per_mtok, "3.000000");
    assert_eq!(prices.list().await.unwrap().len(), 1);

    // deletes cascade top-down; exercise the not-found error path too
    orgs.delete(org.id).await.unwrap();
    assert!(matches!(orgs.get(org.id).await, Err(Error::NotFound(_))));
}

/// The cache-write rate (#2876) is the one price column an upsert does not
/// replace by default: a client that predates it sends a body without it,
/// and that must not wipe a rate set since.
#[tokio::test]
async fn a_price_keeps_its_cache_write_rate_unless_told_otherwise() {
    if !super::super::test_database::is_configured() {
        eprintln!("skipping: {} not set", super::super::test_database::URL_ENV);
        return;
    }
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let prices = ModelPriceRepo(&pool);

    // a new row with no rate has none, which prices writes as input
    let bare = prices
        .upsert(NewModelPrice {
            model: "claude",
            input_per_mtok: "3",
            output_per_mtok: "15",
            cached_input_per_mtok: Some("0.3"),
            cache_write_per_mtok: None,
            cache_write_1h_per_mtok: None,
            currency: "USD",
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(bare.cache_write_per_mtok, None);

    // setting it
    let set = prices
        .upsert(NewModelPrice {
            model: "claude",
            input_per_mtok: "3",
            output_per_mtok: "15",
            cached_input_per_mtok: Some("0.3"),
            cache_write_per_mtok: Some(Some("3.75")),
            cache_write_1h_per_mtok: None,
            currency: "USD",
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(set.cache_write_per_mtok.as_deref(), Some("3.750000"));
    assert_eq!(set.id, bare.id, "an upsert edits the row in place");

    // replacing the rest of the row without naming it keeps the rate
    let kept = prices
        .upsert(NewModelPrice {
            model: "claude",
            input_per_mtok: "4",
            output_per_mtok: "20",
            cached_input_per_mtok: None,
            cache_write_per_mtok: None,
            cache_write_1h_per_mtok: None,
            currency: "USD",
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(kept.input_per_mtok, "4.000000");
    assert_eq!(kept.cached_input_per_mtok, None, "the others still replace");
    assert_eq!(kept.cache_write_per_mtok.as_deref(), Some("3.750000"));

    // and it is what a read and the snapshot see
    let listed = prices.list().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].cache_write_per_mtok.as_deref(), Some("3.750000"));
    use crate::ConfigStore as _;
    let config = crate::postgres::PostgresConfigStore::new(pool.clone())
        .load()
        .await
        .unwrap();
    assert_eq!(
        config.model_prices[0].cache_write_per_mtok,
        Some("3.75".parse().unwrap())
    );

    // clearing is its own, explicit request
    let cleared = prices
        .upsert(NewModelPrice {
            model: "claude",
            input_per_mtok: "4",
            output_per_mtok: "20",
            cached_input_per_mtok: None,
            cache_write_per_mtok: Some(None),
            cache_write_1h_per_mtok: None,
            currency: "USD",
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(cleared.cache_write_per_mtok, None);
    let config = crate::postgres::PostgresConfigStore::new(pool)
        .load()
        .await
        .unwrap();
    assert_eq!(config.model_prices[0].cache_write_per_mtok, None);
}

/// The 1 hour rate (#2891) follows the same rule as the 5 minute one, and
/// the two are independent: naming one leaves the other where it was.
#[tokio::test]
async fn a_price_keeps_its_one_hour_cache_write_rate_unless_told_otherwise() {
    if !super::super::test_database::is_configured() {
        eprintln!("skipping: {} not set", super::super::test_database::URL_ENV);
        return;
    }
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let prices = ModelPriceRepo(&pool);

    let bare = prices
        .upsert(NewModelPrice {
            model: "claude",
            input_per_mtok: "3",
            output_per_mtok: "15",
            cached_input_per_mtok: None,
            cache_write_per_mtok: None,
            cache_write_1h_per_mtok: None,
            currency: "USD",
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(bare.cache_write_1h_per_mtok, None);

    let set = prices
        .upsert(NewModelPrice {
            model: "claude",
            input_per_mtok: "3",
            output_per_mtok: "15",
            cached_input_per_mtok: None,
            cache_write_per_mtok: Some(Some("3.75")),
            cache_write_1h_per_mtok: Some(Some("6")),
            currency: "USD",
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(set.cache_write_per_mtok.as_deref(), Some("3.750000"));
    assert_eq!(set.cache_write_1h_per_mtok.as_deref(), Some("6.000000"));

    // omitted keeps both
    let kept = prices
        .upsert(NewModelPrice {
            model: "claude",
            input_per_mtok: "4",
            output_per_mtok: "20",
            cached_input_per_mtok: None,
            cache_write_per_mtok: None,
            cache_write_1h_per_mtok: None,
            currency: "USD",
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(kept.input_per_mtok, "4.000000");
    assert_eq!(kept.cache_write_per_mtok.as_deref(), Some("3.750000"));
    assert_eq!(kept.cache_write_1h_per_mtok.as_deref(), Some("6.000000"));

    // naming the 5 minute rate leaves the 1 hour one alone, and back
    let only_5m = prices
        .upsert(NewModelPrice {
            model: "claude",
            input_per_mtok: "4",
            output_per_mtok: "20",
            cached_input_per_mtok: None,
            cache_write_per_mtok: Some(Some("5")),
            cache_write_1h_per_mtok: None,
            currency: "USD",
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(only_5m.cache_write_per_mtok.as_deref(), Some("5.000000"));
    assert_eq!(only_5m.cache_write_1h_per_mtok.as_deref(), Some("6.000000"));
    let only_1h = prices
        .upsert(NewModelPrice {
            model: "claude",
            input_per_mtok: "4",
            output_per_mtok: "20",
            cached_input_per_mtok: None,
            cache_write_per_mtok: None,
            cache_write_1h_per_mtok: Some(Some("8")),
            currency: "USD",
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(only_1h.cache_write_per_mtok.as_deref(), Some("5.000000"));
    assert_eq!(only_1h.cache_write_1h_per_mtok.as_deref(), Some("8.000000"));

    // what a read and the snapshot see
    assert_eq!(
        prices.list().await.unwrap()[0]
            .cache_write_1h_per_mtok
            .as_deref(),
        Some("8.000000")
    );
    use crate::ConfigStore as _;
    let config = crate::postgres::PostgresConfigStore::new(pool.clone())
        .load()
        .await
        .unwrap();
    assert_eq!(
        config.model_prices[0].cache_write_1h_per_mtok,
        Some("8".parse().unwrap())
    );

    // clearing is its own, explicit request, and only for the rate named
    let cleared = prices
        .upsert(NewModelPrice {
            model: "claude",
            input_per_mtok: "4",
            output_per_mtok: "20",
            cached_input_per_mtok: None,
            cache_write_per_mtok: None,
            cache_write_1h_per_mtok: Some(None),
            currency: "USD",
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(cleared.cache_write_1h_per_mtok, None);
    assert_eq!(cleared.cache_write_per_mtok.as_deref(), Some("5.000000"));
    let config = crate::postgres::PostgresConfigStore::new(pool)
        .load()
        .await
        .unwrap();
    assert_eq!(config.model_prices[0].cache_write_1h_per_mtok, None);
}

/// The unit rates (#2937) are kept by an upsert that does not name them, set
/// and cleared one at a time by one that does, and a negative one is refused
/// by the database.
#[tokio::test]
async fn a_price_keeps_its_unit_rates_unless_told_otherwise() {
    if !super::super::test_database::is_configured() {
        eprintln!("skipping: {} not set", super::super::test_database::URL_ENV);
        return;
    }
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let prices = ModelPriceRepo(&pool);

    let set = prices
        .upsert(NewModelPrice {
            model: "media",
            input_per_mtok: "0",
            output_per_mtok: "0",
            cached_input_per_mtok: None,
            cache_write_per_mtok: None,
            cache_write_1h_per_mtok: None,
            units: UnitRates {
                image_per_unit: Some(Some("0.04")),
                audio_input_per_minute: Some(Some("0.006")),
                speech_per_mchar: Some(Some("15")),
                ..UnitRates::default()
            },
            currency: "USD",
        })
        .await
        .unwrap();
    assert_eq!(set.image_per_unit.as_deref(), Some("0.040000"));
    assert_eq!(set.speech_per_mchar.as_deref(), Some("15.000000"));

    // the plain upsert names none of them
    let kept = prices
        .upsert(NewModelPrice {
            model: "media",
            input_per_mtok: "1",
            output_per_mtok: "2",
            cached_input_per_mtok: None,
            cache_write_per_mtok: None,
            cache_write_1h_per_mtok: None,
            currency: "USD",
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(kept.input_per_mtok, "1.000000");
    assert_eq!(kept.image_per_unit.as_deref(), Some("0.040000"));
    assert_eq!(kept.audio_input_per_minute.as_deref(), Some("0.006000"));
    assert_eq!(kept.speech_per_mchar.as_deref(), Some("15.000000"));

    let cleared = prices
        .upsert(NewModelPrice {
            model: "media",
            input_per_mtok: "1",
            output_per_mtok: "2",
            cached_input_per_mtok: None,
            cache_write_per_mtok: None,
            cache_write_1h_per_mtok: None,
            units: UnitRates {
                image_per_unit: Some(None),
                ..UnitRates::default()
            },
            currency: "USD",
        })
        .await
        .unwrap();
    assert_eq!(cleared.image_per_unit, None);
    assert_eq!(cleared.speech_per_mchar.as_deref(), Some("15.000000"));

    let negative = prices
        .upsert(NewModelPrice {
            model: "media",
            input_per_mtok: "1",
            output_per_mtok: "2",
            cached_input_per_mtok: None,
            cache_write_per_mtok: None,
            cache_write_1h_per_mtok: None,
            units: UnitRates {
                speech_per_mchar: Some(Some("-1")),
                ..UnitRates::default()
            },
            currency: "USD",
        })
        .await;
    assert!(negative.is_err(), "a negative rate must be refused");
}

/// The audio token rates (#1882) are kept by an upsert that does not name
/// them, set and cleared one at a time by one that does, and a negative one
/// is refused by the database.
#[tokio::test]
async fn a_price_keeps_its_audio_rates_unless_told_otherwise() {
    if !super::super::test_database::is_configured() {
        eprintln!("skipping: {} not set", super::super::test_database::URL_ENV);
        return;
    }
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let prices = ModelPriceRepo(&pool);

    let set = prices
        .upsert(NewModelPrice {
            model: "voice",
            input_per_mtok: "4",
            output_per_mtok: "16",
            cached_input_per_mtok: Some("0.4"),
            cache_write_per_mtok: None,
            cache_write_1h_per_mtok: None,
            units: UnitRates {
                audio_input_per_mtok: Some(Some("32")),
                audio_output_per_mtok: Some(Some("64")),
                cached_audio_input_per_mtok: Some(Some("0.4")),
                ..UnitRates::default()
            },
            currency: "USD",
        })
        .await
        .unwrap();
    assert_eq!(set.audio_input_per_mtok.as_deref(), Some("32.000000"));
    assert_eq!(set.audio_output_per_mtok.as_deref(), Some("64.000000"));

    // a client that predates the fields replaces the row without them
    let kept = prices
        .upsert(NewModelPrice {
            model: "voice",
            input_per_mtok: "5",
            output_per_mtok: "20",
            cached_input_per_mtok: None,
            cache_write_per_mtok: None,
            cache_write_1h_per_mtok: None,
            currency: "USD",
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(kept.input_per_mtok, "5.000000");
    assert_eq!(kept.audio_input_per_mtok.as_deref(), Some("32.000000"));
    assert_eq!(kept.audio_output_per_mtok.as_deref(), Some("64.000000"));
    assert_eq!(
        kept.cached_audio_input_per_mtok.as_deref(),
        Some("0.400000")
    );

    let cleared = prices
        .upsert(NewModelPrice {
            model: "voice",
            input_per_mtok: "5",
            output_per_mtok: "20",
            cached_input_per_mtok: None,
            cache_write_per_mtok: None,
            cache_write_1h_per_mtok: None,
            units: UnitRates {
                audio_output_per_mtok: Some(None),
                ..UnitRates::default()
            },
            currency: "USD",
        })
        .await
        .unwrap();
    assert_eq!(cleared.audio_output_per_mtok, None);
    assert_eq!(cleared.audio_input_per_mtok.as_deref(), Some("32.000000"));

    let negative = prices
        .upsert(NewModelPrice {
            model: "voice",
            input_per_mtok: "5",
            output_per_mtok: "20",
            cached_input_per_mtok: None,
            cache_write_per_mtok: None,
            cache_write_1h_per_mtok: None,
            units: UnitRates {
                cached_audio_input_per_mtok: Some(Some("-1")),
                ..UnitRates::default()
            },
            currency: "USD",
        })
        .await;
    assert!(negative.is_err(), "a negative rate must be refused");
}

/// The column carries the same floor as the others (0084): a negative rate
/// would be a credit against a budget, and `NaN` sorts above every number,
/// so a row written around the API is refused by the database itself.
#[tokio::test]
async fn a_negative_one_hour_rate_is_refused_by_the_database() {
    if !super::super::test_database::is_configured() {
        eprintln!("skipping: {} not set", super::super::test_database::URL_ENV);
        return;
    }
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let prices = ModelPriceRepo(&pool);
    for bad in ["-1", "NaN"] {
        let refused = prices
            .upsert(NewModelPrice {
                model: "claude",
                input_per_mtok: "3",
                output_per_mtok: "15",
                cached_input_per_mtok: None,
                cache_write_per_mtok: None,
                cache_write_1h_per_mtok: Some(Some(bad)),
                currency: "USD",
                ..Default::default()
            })
            .await;
        assert!(refused.is_err(), "{bad}: {refused:?}");
    }
    assert!(prices.list().await.unwrap().is_empty());
}

#[tokio::test]
async fn prompt_template_versions_publish_and_scope_round_trip() {
    if !super::super::test_database::is_configured() {
        eprintln!("skipping: {} not set", super::super::test_database::URL_ENV);
        return;
    }
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let org = OrgRepo(&pool).create("acme", "acme").await.unwrap();

    let repo = PromptTemplateRepo(&pool);
    let template = repo
        .create_template(
            org.id,
            "support assistant",
            "support-assistant",
            Some("tier 1 support guardrails"),
        )
        .await
        .unwrap();
    assert_eq!(template.published_version, None);
    assert_eq!(repo.list_templates(org.id).await.unwrap().len(), 1);

    let v1 = repo
        .create_version(template.id, &serde_json::json!([]), &serde_json::json!([]))
        .await
        .unwrap();
    let v2 = repo
        .create_version(
            template.id,
            &serde_json::json!([{"name":"tier","required":true}]),
            &serde_json::json!([{"role":"system","position":"prepend","content":"{{ tier }} mode"}]),
        )
        .await
        .unwrap();
    assert_eq!(v1.version, 1);
    assert_eq!(v2.version, 2);
    let empty_variables = serde_json::json!([]);
    let empty_decorators = serde_json::json!([]);
    let (v3, v4) = tokio::join!(
        repo.create_version(template.id, &empty_variables, &empty_decorators),
        repo.create_version(template.id, &empty_variables, &empty_decorators)
    );
    let mut concurrent_versions = [v3.unwrap().version, v4.unwrap().version];
    concurrent_versions.sort();
    assert_eq!(concurrent_versions, [3, 4]);
    let versions = repo.list_versions(template.id).await.unwrap();
    assert_eq!(versions.len(), 4);
    assert_eq!(versions[0].version, 4);
    assert_eq!(versions[3].version, 1);

    let project = ProjectRepo(&pool)
        .create(
            TeamRepo(&pool).create(org.id, "core").await.unwrap().id,
            "gateway",
        )
        .await
        .unwrap();
    repo.set_scopes(
        template.id,
        2,
        &[
            ("org".to_string(), org.id),
            ("project".to_string(), project.id),
        ],
    )
    .await
    .unwrap();
    let scopes = repo.list_scopes(template.id, 2).await.unwrap();
    assert_eq!(scopes.len(), 2);
    assert_eq!(scopes[0].scope_type, "org");
    assert_eq!(scopes[1].scope_type, "project");

    let published = repo.publish_version(template.id, v2.version).await.unwrap();
    assert_eq!(published.published_version, Some(2));
    assert!(repo
        .set_scopes(template.id, 2, &[("org".to_string(), org.id)])
        .await
        .is_err());

    repo.delete_template(template.id).await.unwrap();
    assert!(matches!(
        repo.get_template(template.id).await,
        Err(Error::NotFound(_))
    ));
}

#[tokio::test]
async fn skill_versions_publish_and_retire_round_trip() {
    if !super::super::test_database::is_configured() {
        eprintln!("skipping: {} not set", super::super::test_database::URL_ENV);
        return;
    }
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let org = OrgRepo(&pool).create("acme", "acme").await.unwrap();

    let repo = SkillRepo(&pool);
    let skill = repo
        .create_skill(
            org.id,
            "classification baseline",
            "classification-baseline",
            Some("first skill"),
            &[],
            "viewer",
        )
        .await
        .unwrap();
    assert_eq!(skill.published_version, None);
    assert_eq!(repo.list_skills(org.id).await.unwrap().len(), 1);

    let v1 = repo
        .create_version(
            skill.id,
            Some("content-v1"),
            None,
            &serde_json::json!({"author":"ops"}),
        )
        .await
        .unwrap();
    let v2 = repo
        .create_version(
            skill.id,
            None,
            Some("oci://registry.example/skills/classification@sha256:abc"),
            &serde_json::json!({"author":"ops"}),
        )
        .await
        .unwrap();
    assert_eq!(v1.version, 1);
    assert_eq!(v2.version, 2);
    let metadata = serde_json::json!({"author":"ops"});
    let (v3, v4) = tokio::join!(
        repo.create_version(skill.id, Some("content-v3"), None, &metadata),
        repo.create_version(skill.id, Some("content-v4"), None, &metadata)
    );
    let mut concurrent_versions = [v3.unwrap().version, v4.unwrap().version];
    concurrent_versions.sort();
    assert_eq!(concurrent_versions, [3, 4]);
    let versions = repo.list_versions(skill.id).await.unwrap();
    assert_eq!(versions.len(), 4);
    assert_eq!(versions[0].version, 4);
    assert_eq!(versions[3].version, 1);

    let published = repo.publish_version(skill.id, 2).await.unwrap();
    assert_eq!(published.published_version, Some(2));
    let resolved = repo.resolve_published(skill.id).await.unwrap();
    assert_eq!(resolved.version, 2);
    assert!(resolved.content.is_none());
    assert_eq!(
        resolved.content_ref.as_deref(),
        Some("oci://registry.example/skills/classification@sha256:abc")
    );

    let retired = repo
        .update_skill(
            skill.id,
            Some("classification baseline"),
            None,
            Some(true),
            None,
            None,
        )
        .await
        .unwrap();
    assert!(retired.retired_at.is_some());
    assert!(repo.resolve_published(skill.id).await.is_err());

    repo.delete_skill(skill.id).await.unwrap();
    assert!(matches!(
        repo.get_skill(skill.id).await,
        Err(Error::NotFound(_))
    ));
}

#[tokio::test]
async fn plugin_instances_round_trip_at_org_and_project_scope() {
    if !super::super::test_database::is_configured() {
        eprintln!("skipping: {} not set", super::super::test_database::URL_ENV);
        return;
    }
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let org = OrgRepo(&pool).create("plugins", "plugins").await.unwrap();
    let team = TeamRepo(&pool).create(org.id, "platform").await.unwrap();
    let project = ProjectRepo(&pool).create(team.id, "gateway").await.unwrap();
    let repo = PluginRepo(&pool);
    let org_plugin = repo
        .create(
            org.id,
            NewPlugin {
                project_id: None,
                name: "audit",
                slug: "audit",
                description: "org audit",
                kind: "webhook",
                stage: "post_response",
                enabled: false,
                position: 20,
                failure_mode: "fail_open",
                endpoint: "https://plugins.internal/audit",
                secret_env: None,
                config: &serde_json::json!({}),
            },
        )
        .await
        .unwrap();
    let project_plugin = repo
        .create(
            org.id,
            NewPlugin {
                project_id: Some(project.id),
                name: "audit override",
                slug: "audit",
                description: "project audit",
                kind: "webhook",
                stage: "pre_upstream",
                enabled: true,
                position: 10,
                failure_mode: "fail_closed",
                endpoint: "https://plugins.internal/project-audit",
                secret_env: Some("PLUGIN_TOKEN"),
                config: &serde_json::json!({"sample": 1.0}),
            },
        )
        .await
        .unwrap();
    assert_eq!(repo.list(org.id).await.unwrap().len(), 2);
    let updated = repo
        .update(
            project_plugin.id,
            PluginUpdate {
                project_id: Some(project.id),
                name: "audit override",
                description: "updated",
                stage: "pre_route",
                enabled: false,
                position: 5,
                failure_mode: "fail_open",
                endpoint: "https://plugins.internal/project-audit",
                secret_env: None,
                config: &serde_json::json!({"sample": 0.5}),
            },
        )
        .await
        .unwrap();
    assert_eq!(updated.stage, "pre_route");
    assert_eq!(updated.position, 5);
    repo.delete(org_plugin.id).await.unwrap();
    assert_eq!(repo.list(org.id).await.unwrap().len(), 1);
}

#[tokio::test]
async fn audit_log_keyset_pages_filter_without_shifting_boundaries() {
    if !super::super::test_database::is_configured() {
        eprintln!("skipping: {} not set", super::super::test_database::URL_ENV);
        return;
    }
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let org = OrgRepo(&pool).create("audit", "audit").await.unwrap();
    let actor = UserRepo(&pool)
        .create("audit@example.com", None, false)
        .await
        .unwrap();
    let repo = AuditLogRepo(&pool);
    let now = Utc::now();
    for (offset, action) in [
        (3, "route.create"),
        (2, "route.create"),
        (1, "route.delete"),
    ] {
        let entry = repo
            .create(
                Some(org.id),
                Some(actor.id),
                action,
                Some("route"),
                Some(Uuid::new_v4()),
                None,
            )
            .await
            .unwrap();
        sqlx::query("update audit_log set at = $2 where id = $1")
            .bind(entry.id)
            .bind(now - chrono::Duration::seconds(offset))
            .execute(&pool)
            .await
            .unwrap();
    }
    let filter = AuditLogFilter {
        actor_user_id: Some(actor.id),
        action: Some("route.create".to_string()),
        target_type: Some("route".to_string()),
        ..Default::default()
    };
    let first = repo.list_page(org.id, &filter, 1).await.unwrap();
    assert_eq!(first.entries.len(), 1);
    assert!(first.has_more);
    let boundary = first.entries[0].clone();
    let second = repo
        .list_page(
            org.id,
            &AuditLogFilter {
                cursor: Some(AuditLogCursor {
                    at: boundary.at,
                    id: boundary.id,
                }),
                ..filter.clone()
            },
            1,
        )
        .await
        .unwrap();
    assert_eq!(second.entries.len(), 1);
    assert_ne!(second.entries[0].id, boundary.id);
    let previous = repo
        .list_page(
            org.id,
            &AuditLogFilter {
                cursor: Some(AuditLogCursor {
                    at: second.entries[0].at,
                    id: second.entries[0].id,
                }),
                direction: AuditLogDirection::Previous,
                ..filter.clone()
            },
            1,
        )
        .await
        .unwrap();
    assert_eq!(previous.entries[0].id, boundary.id);
    assert_eq!(repo.count(org.id, &filter).await.unwrap(), 2);
}

#[tokio::test]
async fn audit_log_end_at_is_exclusive_in_both_directions() {
    if !super::super::test_database::is_configured() {
        eprintln!("skipping: {} not set", super::super::test_database::URL_ENV);
        return;
    }
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let org = OrgRepo(&pool).create("bounds", "bounds").await.unwrap();
    let repo = AuditLogRepo(&pool);
    let end = Utc::now() - chrono::Duration::hours(1);
    let mut inside = Vec::new();
    for offset in [2, 1, 0] {
        let entry = repo
            .create(Some(org.id), None, "route.create", None, None, None)
            .await
            .unwrap();
        sqlx::query("update audit_log set at = $2 where id = $1")
            .bind(entry.id)
            .bind(end - chrono::Duration::seconds(offset))
            .execute(&pool)
            .await
            .unwrap();
        if offset > 0 {
            inside.push(entry.id);
        }
    }
    let filter = AuditLogFilter {
        start_at: Some(end - chrono::Duration::seconds(2)),
        end_at: Some(end),
        ..Default::default()
    };
    let ids = |page: AuditLogPage| -> Vec<Uuid> {
        let mut ids: Vec<Uuid> = page.entries.iter().map(|entry| entry.id).collect();
        ids.sort();
        ids
    };
    inside.sort();
    for direction in [AuditLogDirection::Next, AuditLogDirection::Previous] {
        let filter = AuditLogFilter {
            direction,
            ..filter.clone()
        };
        let page = repo.list_page(org.id, &filter, 10).await.unwrap();
        assert_eq!(ids(page), inside, "{direction:?}");
        let page = repo.list_page_all(&filter, 10).await.unwrap();
        assert_eq!(ids(page), inside, "{direction:?}");
    }
    assert_eq!(repo.count(org.id, &filter).await.unwrap(), 2);
    assert_eq!(repo.count_all(&filter).await.unwrap(), 2);
}

#[tokio::test]
async fn sso_sweeps_drop_expired_rows_nobody_used_and_keep_live_ones() {
    if !super::super::test_database::is_configured() {
        eprintln!("skipping: {} not set", super::super::test_database::URL_ENV);
        return;
    }
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let org = OrgRepo(&pool).create("sso", "sso").await.unwrap();
    let user = UserRepo(&pool)
        .create("sso@example.com", None, false)
        .await
        .unwrap();
    let repo = SsoRepo(&pool);
    let provider = repo
        .create_provider(
            org.id,
            NewSsoProvider {
                name: "idp",
                slug: "idp",
                issuer: "https://idp.example.com",
                client_id: "client",
                secret: None,
                scopes: &[],
                group_claim: "groups",
                default_role: None,
            },
        )
        .await
        .unwrap();

    // the oidc nonce is generated, never a literal: a fixed value here reads
    // to codeql as a hard-coded cryptographic nonce
    let nonce = Uuid::new_v4().to_string();
    // three abandoned logins, backdated past the ttl, and one in flight
    for state in ["old-1", "old-2", "old-3", "live"] {
        repo.start_login(state, provider.id, "verifier", &nonce, "https://cb")
            .await
            .unwrap();
    }
    sqlx::query(
        "update sso_login_states set created_at = now() - interval '1 hour' \
         where state like 'old-%'",
    )
    .execute(&pool)
    .await
    .unwrap();
    // two codes nobody redeemed, one already expired
    repo.issue_exchange("expired-code", user.id, provider.id, &[], 60)
        .await
        .unwrap();
    repo.issue_exchange("live-code", user.id, provider.id, &[], 60)
        .await
        .unwrap();
    sqlx::query(
        "update sso_exchange_codes set expires_at = now() - interval '1 second' \
         where code_hash = 'expired-code'",
    )
    .execute(&pool)
    .await
    .unwrap();

    let states = || async {
        sqlx::query_scalar::<_, String>("select state from sso_login_states order by state")
            .fetch_all(&pool)
            .await
            .unwrap()
    };
    // the batch bound holds: one pass of two leaves the third expired row
    assert_eq!(repo.sweep_login_states(600, 2).await.unwrap(), 2);
    assert_eq!(states().await.len(), 2);
    assert_eq!(repo.sweep_login_states(600, 2).await.unwrap(), 1);
    assert_eq!(states().await, vec!["live".to_string()]);
    assert_eq!(repo.sweep_login_states(600, 2).await.unwrap(), 0);

    assert_eq!(repo.sweep_exchange_codes(10).await.unwrap(), 1);
    let codes: Vec<String> = sqlx::query_scalar("select code_hash from sso_exchange_codes")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(codes, vec!["live-code".to_string()]);

    // the live rows are still usable after a sweep
    assert!(repo.consume_login("live", 600).await.unwrap().is_some());
    assert!(repo.redeem_exchange("live-code").await.unwrap().is_some());

    // an expired state is refused even when the bounded sweep stops short
    // of it: a full batch of older rows sits in front, and the row is
    // spent all the same
    repo.start_login("stale", provider.id, "verifier", &nonce, "https://cb")
        .await
        .unwrap();
    sqlx::query("update sso_login_states set created_at = now() - interval '1 hour'")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "insert into sso_login_states (state, provider_id, code_verifier, nonce, redirect_uri, created_at) \
         select 'backlog-' || n, $1, 'v', 'n', 'https://cb', now() - interval '2 hours' \
         from generate_series(1, $2) as n",
    )
    .bind(provider.id)
    .bind(SSO_SWEEP_BATCH as i32)
    .execute(&pool)
    .await
    .unwrap();
    assert!(repo.consume_login("stale", 600).await.unwrap().is_none());
    assert!(states().await.is_empty());
}

#[tokio::test]
async fn mfa_and_mcp_sweeps_drop_expired_rows_nobody_used_and_keep_live_ones() {
    if !super::super::test_database::is_configured() {
        eprintln!("skipping: {} not set", super::super::test_database::URL_ENV);
        return;
    }
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let org = OrgRepo(&pool).create("sweep", "sweep").await.unwrap();
    let user = UserRepo(&pool)
        .create("sweep@example.com", None, false)
        .await
        .unwrap();

    // two challenges for logins abandoned at the prompt, one still live
    let mfa = MfaRepo(&pool);
    for (hash, ttl) in [("old-1", -60), ("old-2", -30), ("live", 600)] {
        mfa.create_challenge(
            user.id,
            hash,
            Utc::now() + Duration::seconds(ttl),
            ChallengePurpose::Verify,
            false,
        )
        .await
        .unwrap();
    }
    // a batch of one leaves the second expired row for the next pass
    assert_eq!(mfa.sweep_expired_challenges(1).await.unwrap(), 1);
    assert_eq!(mfa.sweep_expired_challenges(10).await.unwrap(), 1);
    assert_eq!(mfa.sweep_expired_challenges(10).await.unwrap(), 0);
    let left: Vec<String> = sqlx::query_scalar("select token_hash from mfa_challenges")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(left, vec!["live".to_string()]);

    // the same for consents abandoned at an mcp authorization server
    let server = McpServerRepo(&pool)
        .create(NewMcpServer {
            org_id: org.id,
            name: "mcp",
            slug: "mcp",
            url: "https://mcp.example.com",
            transport: "streamable_http",
            description: "",
            enabled: true,
            tools: &[],
            source: "custom",
            required_scopes: &[],
        })
        .await
        .unwrap();
    for state in ["old-1", "old-2", "live"] {
        sqlx::query(
            "insert into mcp_oauth_login_states \
                 (state, server_id, user_id, verifier_ciphertext, verifier_nonce, redirect_uri) \
             values ($1, $2, $3, '\\x00', '\\x00', 'https://cb')",
        )
        .bind(state)
        .bind(server.id)
        .bind(user.id)
        .execute(&pool)
        .await
        .unwrap();
    }
    sqlx::query(
        "update mcp_oauth_login_states set created_at = now() - interval '1 hour' \
         where state like 'old-%'",
    )
    .execute(&pool)
    .await
    .unwrap();
    let mcp = McpOAuthRepo(&pool);
    let cutoff = Utc::now() - Duration::seconds(600);
    assert_eq!(mcp.sweep_login_states(cutoff, 1).await.unwrap(), 1);
    assert_eq!(mcp.sweep_login_states(cutoff, 10).await.unwrap(), 1);
    assert_eq!(mcp.sweep_login_states(cutoff, 10).await.unwrap(), 0);
    let left: Vec<String> = sqlx::query_scalar("select state from mcp_oauth_login_states")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(left, vec!["live".to_string()]);
}

fn stored_target(provider_id: Uuid, upstream_model: Option<&str>, weight: i32) -> RouteTarget {
    RouteTarget {
        id: Uuid::new_v4(),
        route_id: Uuid::nil(),
        provider_id,
        upstream_model: upstream_model.map(str::to_string),
        weight,
        takes_over: Vec::new(),
        created_at: Utc::now(),
    }
}

fn spec(provider_id: Uuid, upstream_model: Option<&str>, weight: i32) -> RouteTargetSpec {
    RouteTargetSpec {
        provider_id,
        upstream_model: upstream_model.map(str::to_string),
        weight,
        takes_over: Vec::new(),
    }
}

/// #1584: a changed `takes_over` set is a change to the matched target, like
/// a changed weight, rather than a different target.
#[test]
fn target_plan_updates_a_changed_takes_over_set_in_place() {
    let a = Uuid::new_v4();
    let stored = vec![stored_target(a, None, 1)];
    let mut restricted = spec(a, None, 1);
    restricted.takes_over = vec!["refusal".to_string()];
    let desired = vec![restricted];
    let plan = plan_target_changes(&stored, &desired);
    assert_eq!(plan.reweight.len(), 1);
    assert_eq!(plan.reweight[0].0.id, stored[0].id);
    assert_eq!(plan.reweight[0].1.takes_over, vec!["refusal".to_string()]);
    assert!(plan.insert.is_empty());
    assert!(plan.delete.is_empty());
}

/// #2235: a target is matched by provider and upstream model, so a weight
/// edit keeps the row, a changed upstream model is a different target, and
/// an unchanged entry is neither written nor reported.
#[test]
fn target_plan_matches_on_provider_and_upstream_model() {
    let (a, b, c) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let stored = vec![
        stored_target(a, None, 1),
        stored_target(b, Some("gpt-4o"), 1),
        stored_target(c, None, 3),
    ];
    let desired = vec![
        // same key, new weight: kept and reweighted
        spec(a, None, 5),
        // same provider, other upstream model: a new target, and the old
        // one goes
        spec(b, Some("gpt-4o-mini"), 1),
        // identical: left alone
        spec(c, None, 3),
    ];
    let plan = plan_target_changes(&stored, &desired);
    assert_eq!(
        plan.reweight
            .iter()
            .map(|(t, s)| (t.id, s.weight))
            .collect::<Vec<_>>(),
        vec![(stored[0].id, 5)]
    );
    assert_eq!(plan.insert, vec![&desired[1]]);
    assert_eq!(
        plan.delete.iter().map(|t| t.id).collect::<Vec<_>>(),
        vec![stored[1].id]
    );
}

/// Each entry claims at most one stored row, so duplicates are counted
/// rather than collapsed: naming a twice-held key once drops one of them,
/// and naming it three times adds one.
#[test]
fn target_plan_counts_duplicate_keys() {
    let a = Uuid::new_v4();
    let stored = vec![stored_target(a, None, 1), stored_target(a, None, 2)];

    let once = vec![spec(a, None, 2)];
    let plan = plan_target_changes(&stored, &once);
    // the first stored row is claimed and reweighted, the second one goes
    assert_eq!(plan.reweight.len(), 1);
    assert_eq!(plan.reweight[0].0.id, stored[0].id);
    assert!(plan.insert.is_empty());
    assert_eq!(plan.delete.len(), 1);
    assert_eq!(plan.delete[0].id, stored[1].id);

    let thrice = vec![spec(a, None, 1), spec(a, None, 2), spec(a, None, 1)];
    let plan = plan_target_changes(&stored, &thrice);
    assert!(plan.reweight.is_empty());
    assert_eq!(plan.insert, vec![&thrice[2]]);
    assert!(plan.delete.is_empty());
}

/// #2946: a row whose `egress_proxies` holds a non-string used to fail every
/// read of it, so the provider could neither be listed nor repaired.
#[tokio::test]
async fn a_provider_with_a_non_string_proxy_entry_lists_and_can_be_rewritten() {
    if !super::super::test_database::is_configured() {
        eprintln!("skipping: {} not set", super::super::test_database::URL_ENV);
        return;
    }
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let org = OrgRepo(&pool).create("acme", "acme").await.unwrap();
    sqlx::query("alter table providers drop constraint providers_egress_proxies_strings")
        .execute(&pool)
        .await
        .unwrap();
    let id: Uuid = sqlx::query_scalar(
        "insert into providers (org_id, name, slug, kind, api_base, egress_proxies)
         values ($1, 'odd', 'odd', 'openai', 'https://api.openai.com',
                 '[\"http://proxy.example.com:3128\", 7]'::jsonb)
         returning id",
    )
    .bind(org.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let repo = ProviderRepo(&pool);

    let listed = repo.list(org.id).await.expect("the listing decodes");
    assert_eq!(listed.len(), 1);
    assert_eq!(
        listed[0].egress_proxies.0,
        ["http://proxy.example.com:3128"],
        "the string entries are kept"
    );
    assert_eq!(repo.get(id).await.unwrap().egress_proxies.0.len(), 1);

    // an update that leaves the pool alone still answers
    let kept = repo
        .update(
            id,
            ProviderUpdate {
                slug: None,
                kind: None,
                api_base: None,
                api_key_env: None,
                egress_proxy: None,
                egress_proxies: None,
                kv_events: None,
                lmcache: None,
                project_id: None,
                allow_custom_api_base: None,
            },
        )
        .await
        .expect("an update that omits the pool succeeds");
    assert_eq!(kept.egress_proxies.0.len(), 1);

    let pool_now = vec!["http://other.example.com:3128".to_string()];
    let rewritten = repo
        .update(
            id,
            ProviderUpdate {
                slug: None,
                kind: None,
                api_base: None,
                api_key_env: None,
                egress_proxy: None,
                egress_proxies: Some(&pool_now),
                kv_events: None,
                lmcache: None,
                project_id: None,
                allow_custom_api_base: None,
            },
        )
        .await
        .expect("a write that sets the pool succeeds");
    assert_eq!(rewritten.egress_proxies.0, pool_now);
    let stored: serde_json::Value =
        sqlx::query_scalar("select egress_proxies from providers where id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(stored, serde_json::json!(pool_now), "the row is repaired");
}

/// #2235: the strategy and the target list change in one transaction, the
/// targets that survive keep their ids, and a write that fails part way
/// leaves the route exactly as it was.
#[tokio::test]
async fn route_update_is_one_transaction() {
    if !super::super::test_database::is_configured() {
        eprintln!("skipping: {} not set", super::super::test_database::URL_ENV);
        return;
    }
    let db = fresh_db().await;
    let pool = db.pool().clone();

    let org = OrgRepo(&pool).create("acme", "acme").await.unwrap();
    let team = TeamRepo(&pool).create(org.id, "core").await.unwrap();
    let project = ProjectRepo(&pool).create(team.id, "default").await.unwrap();
    let mut provider_ids = Vec::new();
    for name in ["primary", "secondary"] {
        let provider = ProviderRepo(&pool)
            .create(
                org.id,
                NewProvider {
                    name,
                    slug: name,
                    kind: "openai",
                    api_base: "https://api.openai.com",
                    api_key_env: Some("OPENAI_API_KEY"),
                    egress_proxy: None,
                    egress_proxies: &[],
                    kv_events: None,
                    lmcache: None,
                    project_id: None,
                    allow_custom_api_base: false,
                },
            )
            .await
            .unwrap();
        provider_ids.push(provider.id);
    }
    let (primary, secondary) = (provider_ids[0], provider_ids[1]);
    let routes = RouteRepo(&pool);
    let route = routes
        .create(project.id, "gpt-4o", "round_robin")
        .await
        .unwrap();
    let targets = RouteTargetRepo(&pool);
    let kept = targets
        .create(route.id, primary, None, 1, &[])
        .await
        .unwrap();
    let dropped = targets
        .create(route.id, primary, Some("gpt-4o-mini"), 1, &[])
        .await
        .unwrap();

    let desired = vec![spec(primary, None, 3), spec(secondary, None, 1)];
    let (updated, changes) = routes
        .update(
            route.id,
            &RouteUpdate {
                strategy: Some("weighted"),
                targets: Some(&desired),
                ..RouteUpdate::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(updated.strategy, "weighted");
    assert!(updated.enabled, "an omitted enabled flag is kept");
    assert_eq!(changes.reweighted.len(), 1);
    assert_eq!(changes.reweighted[0].target.id, kept.id);
    assert_eq!(changes.reweighted[0].previous_weight, 1);
    assert_eq!(changes.added.len(), 1);
    assert_eq!(changes.added[0].provider_id, secondary);
    assert_eq!(
        changes.removed.iter().map(|t| t.id).collect::<Vec<_>>(),
        vec![dropped.id]
    );
    let listed = targets.list(route.id).await.unwrap();
    assert_eq!(
        listed.iter().map(|t| (t.id, t.weight)).collect::<Vec<_>>(),
        vec![(kept.id, 3), (changes.added[0].id, 1)],
        "the kept target keeps its id and its place"
    );

    // the second entry names a provider that does not exist, so its
    // insert fails after the strategy write and the delete already ran in
    // the same transaction: none of it may survive
    let before: i64 = sqlx::query_scalar("select version from config_version where id = 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    let broken = vec![spec(secondary, None, 1), spec(Uuid::new_v4(), None, 1)];
    routes
        .update(
            route.id,
            &RouteUpdate {
                enabled: Some(false),
                strategy: Some("random"),
                targets: Some(&broken),
            },
        )
        .await
        .expect_err("a target on an unknown provider fails the foreign key");
    let after: i64 = sqlx::query_scalar("select version from config_version where id = 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(after, before, "a rolled-back update bumps nothing");
    let route_now = routes.get(route.id).await.unwrap();
    assert_eq!(route_now.strategy, "weighted");
    assert!(route_now.enabled);
    assert_eq!(
        targets
            .list(route.id)
            .await
            .unwrap()
            .iter()
            .map(|t| t.id)
            .collect::<Vec<_>>(),
        listed.iter().map(|t| t.id).collect::<Vec<_>>()
    );

    // an update naming nothing changes nothing and reports nothing
    let (same, changes) = routes
        .update(route.id, &RouteUpdate::default())
        .await
        .unwrap();
    assert_eq!(same.strategy, "weighted");
    assert!(changes.is_empty());

    let missing = routes
        .update(Uuid::new_v4(), &RouteUpdate::default())
        .await
        .expect_err("unknown route");
    assert!(matches!(missing, Error::NotFound(_)), "{missing:?}");
}

/// #984: a provider's cache-reporting verdict is stored with its time, replaced
/// by the next probe, and listed only for its own org.
#[tokio::test]
async fn a_cache_probe_verdict_is_replaced_by_the_next_probe() {
    if !super::super::test_database::is_configured() {
        eprintln!("skipping: {} not set", super::super::test_database::URL_ENV);
        return;
    }
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let org = OrgRepo(&pool).create("acme", "acme").await.unwrap();
    let other = OrgRepo(&pool).create("other", "other").await.unwrap();
    let insert = |org_id: Uuid, name: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, Uuid>(
                "insert into providers (org_id, name, slug, kind, api_base)
                 values ($1, $2, $2, 'openai', 'https://api.openai.com') returning id",
            )
            .bind(org_id)
            .bind(name)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    let mine = insert(org.id, "mine").await;
    let theirs = insert(other.id, "theirs").await;
    let repo = ProviderCacheProbeRepo(&pool);
    assert!(repo.list(org.id).await.unwrap().is_empty(), "never probed");

    let first = repo.record(mine, false, None, "gpt-x").await.unwrap();
    assert!(!first.reports);
    repo.record(theirs, true, Some("/cached_tokens"), "gpt-y")
        .await
        .unwrap();
    let second = repo
        .record(
            mine,
            true,
            Some("/prompt_tokens_details/cached_tokens"),
            "gpt-x",
        )
        .await
        .unwrap();
    assert!(second.reports && second.probed_at >= first.probed_at);

    let listed = repo.list(org.id).await.unwrap();
    assert_eq!(listed.len(), 1, "the other org's verdict is not listed");
    assert_eq!(listed[0].provider_id, mine);
    assert_eq!(
        listed[0].spelling.as_deref(),
        Some("/prompt_tokens_details/cached_tokens")
    );

    sqlx::query("delete from providers where id = $1")
        .bind(mine)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        repo.list(org.id).await.unwrap().is_empty(),
        "deleted with the provider"
    );
}

#[tokio::test]
async fn mcp_tool_discovery_keeps_the_manifest_when_a_later_attempt_fails() {
    if !super::super::test_database::is_configured() {
        eprintln!("skipping: {} not set", super::super::test_database::URL_ENV);
        return;
    }
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let org = OrgRepo(&pool).create("acme", "acme").await.unwrap();
    let repo = McpServerRepo(&pool);
    let server = repo
        .create(NewMcpServer {
            org_id: org.id,
            name: "docs",
            slug: "docs",
            url: "https://mcp.example.com",
            transport: "streamable_http",
            description: "",
            enabled: true,
            tools: &["old".to_string()],
            source: "custom",
            required_scopes: &[],
        })
        .await
        .unwrap();
    assert_eq!(server.tools_discovery_status, "pending");

    // a failure before any success stays "not discovered yet", with the reason
    let failed = repo
        .record_tool_discovery_failure(server.id, "unreachable")
        .await
        .unwrap();
    assert_eq!(failed.tools_discovery_status, "pending");
    assert_eq!(failed.tools_discovery_error.as_deref(), Some("unreachable"));
    assert!(failed.tools_discovered_at.is_none());
    assert!(failed.tools_discovery_attempted_at.is_some());
    assert_eq!(failed.tools, ["old"]);

    let schemas = serde_json::json!([{"name": "new", "description": "d", "input_schema": {}}]);
    let found = repo
        .record_tool_discovery(
            server.id,
            McpToolDiscovery {
                tools: &["new".to_string(), "old".to_string()],
                stale: &["old".to_string()],
                schemas: &schemas,
            },
        )
        .await
        .unwrap();
    assert_eq!(found.tools_discovery_status, "discovered");
    assert_eq!(found.tools_discovery_error, None);
    assert_eq!(found.tools_stale, ["old"]);
    assert_eq!(repo.tool_schemas(server.id).await.unwrap(), schemas);

    // a later failure marks the row failed but keeps what was discovered
    let later = repo
        .record_tool_discovery_failure(server.id, "auth_rejected")
        .await
        .unwrap();
    assert_eq!(later.tools_discovery_status, "failed");
    assert_eq!(later.tools, ["new", "old"]);
    assert_eq!(later.tools_stale, ["old"]);
    assert_eq!(later.tools_discovered_at, found.tools_discovered_at);

    // an operator edit that removes a flagged tool drops its flag
    let edited = repo
        .update(
            server.id,
            McpServerUpdate {
                name: "docs",
                url: "https://mcp.example.com",
                transport: "streamable_http",
                description: "",
                enabled: true,
                tools: &["new".to_string()],
                required_scopes: &[],
                connect_timeout_ms: None,
                request_timeout_ms: None,
                max_retries: None,
            },
        )
        .await
        .unwrap();
    assert!(edited.tools_stale.is_empty());
}

/// Provider health follows the routes a project or team uses (#1833): only the
/// providers its routes target come back, by `(org, name)`, once each.
#[tokio::test]
async fn targeted_providers_are_those_the_scopes_routes_use() {
    if !super::super::test_database::is_configured() {
        eprintln!("skipping: {} not set", super::super::test_database::URL_ENV);
        return;
    }
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let org = OrgRepo(&pool).create("acme", "acme").await.unwrap();
    let team = TeamRepo(&pool).create(org.id, "core").await.unwrap();
    let used = ProjectRepo(&pool).create(team.id, "used").await.unwrap();
    let idle = ProjectRepo(&pool).create(team.id, "idle").await.unwrap();
    let other_team = TeamRepo(&pool).create(org.id, "other").await.unwrap();
    let elsewhere = ProjectRepo(&pool)
        .create(other_team.id, "elsewhere")
        .await
        .unwrap();
    let mut provider_ids = Vec::new();
    for name in ["alpha", "beta", "unused"] {
        let id: Uuid = sqlx::query_scalar(
            "insert into providers (org_id, name, slug, kind, api_base) \
             values ($1, $2, $2, 'openai', 'http://127.0.0.1:9') returning id",
        )
        .bind(org.id)
        .bind(name)
        .fetch_one(&pool)
        .await
        .unwrap();
        provider_ids.push(id);
    }
    // `used` routes to alpha twice (two routes) and `elsewhere` to beta
    for (project, model, provider) in [
        (used.id, "m1", provider_ids[0]),
        (used.id, "m2", provider_ids[0]),
        (elsewhere.id, "m3", provider_ids[1]),
    ] {
        let route: Uuid = sqlx::query_scalar(
            "insert into routes (project_id, model) values ($1, $2) returning id",
        )
        .bind(project)
        .bind(model)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query("insert into route_targets (route_id, provider_id) values ($1, $2)")
            .bind(route)
            .bind(provider)
            .execute(&pool)
            .await
            .unwrap();
    }
    let repo = RouteTargetRepo(&pool);
    let by_project = repo.targeted_providers(&[], &[used.id]).await.unwrap();
    assert_eq!(by_project, vec![(org.id, "alpha".to_string())]);
    // a project with no routes, or no scope at all, reaches nothing
    assert!(repo
        .targeted_providers(&[], &[idle.id])
        .await
        .unwrap()
        .is_empty());
    assert!(repo.targeted_providers(&[], &[]).await.unwrap().is_empty());
    // a team reaches the providers of every project under it
    let by_team = repo
        .targeted_providers(&[other_team.id], &[])
        .await
        .unwrap();
    assert_eq!(by_team, vec![(org.id, "beta".to_string())]);
}

/// Two orgs may each hold a `gpt-4o` route and an `openai` provider, and the
/// store composes both into the config the snapshot is built from, each
/// naming its own org's provider (#1857). The write-time checks answer per org.
#[tokio::test]
async fn two_orgs_each_hold_the_same_route_and_provider_names() {
    if !super::super::test_database::is_configured() {
        eprintln!("skipping: {} not set", super::super::test_database::URL_ENV);
        return;
    }
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let providers = ProviderRepo(&pool);
    let routes = RouteRepo(&pool);
    let mut orgs = Vec::new();
    for slug in ["org-a", "org-b"] {
        let org = OrgRepo(&pool).create(slug, slug).await.unwrap();
        let team = TeamRepo(&pool).create(org.id, "core").await.unwrap();
        let project = ProjectRepo(&pool).create(team.id, "app").await.unwrap();
        let provider = providers
            .create(
                org.id,
                NewProvider {
                    name: "openai",
                    slug: "openai",
                    kind: "openai_compatible",
                    api_base: "http://127.0.0.1:9/v1",
                    api_key_env: None,
                    egress_proxy: None,
                    egress_proxies: &[],
                    kv_events: None,
                    lmcache: None,
                    project_id: None,
                    allow_custom_api_base: false,
                },
            )
            .await
            .unwrap();
        let route = routes
            .create(project.id, "gpt-4o", "round_robin")
            .await
            .unwrap();
        RouteTargetRepo(&pool)
            .create(route.id, provider.id, None, 1, &[])
            .await
            .unwrap();
        orgs.push(org.id);
    }
    let other = OrgRepo(&pool).create("org-c", "org-c").await.unwrap();

    // held in each org, free in a third, and held somewhere when asked broadly
    for org in orgs.iter().copied() {
        assert!(providers.name_in_use("openai", Some(org)).await.unwrap());
        assert!(routes.model_in_use("gpt-4o", Some(org)).await.unwrap());
        assert!(providers
            .address_slug_in_use("openai", Some(org), None)
            .await
            .unwrap());
    }
    assert!(!providers
        .name_in_use("openai", Some(other.id))
        .await
        .unwrap());
    assert!(!routes.model_in_use("gpt-4o", Some(other.id)).await.unwrap());
    assert!(!providers
        .address_slug_in_use("openai", Some(other.id), None)
        .await
        .unwrap());
    assert!(routes.model_in_use("gpt-4o", None).await.unwrap());

    use crate::ConfigStore as _;
    let config = crate::postgres::PostgresConfigStore::new(pool.clone())
        .load()
        .await
        .unwrap();
    let owners = |names: Vec<(&str, Option<&rolter_core::Tenancy>)>, name: &str| {
        let mut owners: Vec<String> = names
            .into_iter()
            .filter(|(n, _)| *n == name)
            .map(|(_, t)| t.map(|t| t.org_id.clone()).unwrap_or_default())
            .collect();
        owners.sort();
        owners
    };
    let mut both: Vec<String> = orgs.iter().map(Uuid::to_string).collect();
    both.sort();
    assert_eq!(
        owners(
            config
                .routes
                .iter()
                .map(|r| (r.model.as_str(), r.tenancy.as_ref()))
                .collect(),
            "gpt-4o"
        ),
        both
    );
    assert_eq!(
        owners(
            config
                .providers
                .iter()
                .map(|p| (p.name.as_str(), p.tenancy.as_ref()))
                .collect(),
            "openai"
        ),
        both
    );
    // each route names `openai`, which resolves in its own org
    let mut config = config;
    assert!(config.sanitize_for_snapshot().is_empty());
    config.validate_snapshot().unwrap();
}

/// One route name per org is held by `routes_org_model_unique` (#3459), not
/// by a check-then-insert: two projects of one org cannot both hold a name,
/// even when both creates race, while another org keeps its own.
#[tokio::test]
async fn the_database_holds_one_route_name_per_org() {
    if !super::super::test_database::is_configured() {
        eprintln!("skipping: {} not set", super::super::test_database::URL_ENV);
        return;
    }
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let org_a = OrgRepo(&pool).create("org-a", "org-a").await.unwrap();
    let org_b = OrgRepo(&pool).create("org-b", "org-b").await.unwrap();
    let team_a = TeamRepo(&pool).create(org_a.id, "platform").await.unwrap();
    let team_b = TeamRepo(&pool).create(org_b.id, "core").await.unwrap();
    let projects = ProjectRepo(&pool);
    let a1 = projects.create(team_a.id, "one").await.unwrap();
    let a2 = projects.create(team_a.id, "two").await.unwrap();
    let b1 = projects.create(team_b.id, "one").await.unwrap();
    let routes = RouteRepo(&pool);

    // two concurrent creates of one name in two projects of one org: one wins
    let (first, second) = tokio::join!(
        routes.create(a1.id, "gpt-4o", "round_robin"),
        routes.create(a2.id, "gpt-4o", "round_robin"),
    );
    let outcomes = [first, second];
    assert_eq!(
        outcomes.iter().filter(|r| r.is_ok()).count(),
        1,
        "{outcomes:?}"
    );
    assert!(
        outcomes
            .iter()
            .any(|r| matches!(r, Err(Error::AlreadyExists(_)))),
        "{outcomes:?}"
    );
    // the other org's namesake is its own
    routes.create(b1.id, "gpt-4o", "round_robin").await.unwrap();
    let held: Vec<(Uuid, String)> =
        sqlx::query_as("select org_id, model from routes order by org_id")
            .fetch_all(&pool)
            .await
            .unwrap();
    let mut expected = vec![
        (org_a.id, "gpt-4o".to_string()),
        (org_b.id, "gpt-4o".to_string()),
    ];
    expected.sort();
    assert_eq!(held, expected, "each org holds the name once");

    // the copy of the org follows the project, whatever a write says
    let route = routes.create(a1.id, "claude", "round_robin").await.unwrap();
    sqlx::query("update routes set org_id = $2 where id = $1")
        .bind(route.id)
        .bind(org_b.id)
        .execute(&pool)
        .await
        .unwrap();
    let org: Uuid = sqlx::query_scalar("select org_id from routes where id = $1")
        .bind(route.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(org, org_a.id);
    // and moves with a team that changes org
    let refused = sqlx::query("update teams set org_id = $2 where id = $1")
        .bind(team_a.id)
        .bind(org_b.id)
        .execute(&pool)
        .await
        .expect_err("org-b already holds gpt-4o, so moving org-a's team must refuse");
    assert!(
        refused.to_string().contains("routes_org_model_unique"),
        "{refused}"
    );
    sqlx::query("delete from routes where model = 'gpt-4o' and project_id = $1")
        .bind(b1.id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("update teams set org_id = $2 where id = $1")
        .bind(team_a.id)
        .bind(org_b.id)
        .execute(&pool)
        .await
        .unwrap();
    let org: Uuid = sqlx::query_scalar("select org_id from routes where id = $1")
        .bind(route.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(org, org_b.id);
}

/// Providers and provider groups share one `slug/model` address per org
/// (#1857), and `0206` holds it across the two tables (#3459): a group cannot
/// take a slug a provider of its org holds, or the reverse, even when both
/// creates race, while another org may take it.
#[tokio::test]
async fn providers_and_groups_share_one_slug_per_org() {
    if !super::super::test_database::is_configured() {
        eprintln!("skipping: {} not set", super::super::test_database::URL_ENV);
        return;
    }
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let org_a = OrgRepo(&pool).create("org-a", "org-a").await.unwrap();
    let org_b = OrgRepo(&pool).create("org-b", "org-b").await.unwrap();
    let provider = |slug: &'static str| NewProvider {
        name: slug,
        slug,
        kind: "openai_compatible",
        api_base: "http://127.0.0.1:9/v1",
        api_key_env: None,
        egress_proxy: None,
        egress_proxies: &[],
        kv_events: None,
        lmcache: None,
        project_id: None,
        allow_custom_api_base: false,
    };
    let providers = ProviderRepo(&pool);
    let groups = ProviderGroupRepo(&pool);

    // a provider and a group racing for one slug in one org: one wins
    let (p, g) = tokio::join!(
        providers.create(org_a.id, provider("edge")),
        groups.create(org_a.id, "edge", "edge", "round_robin", None),
    );
    assert!(
        p.is_ok() != g.is_ok(),
        "exactly one holder: provider {p:?}, group {g:?}"
    );
    let lost = if p.is_ok() {
        g.map(|_| ())
    } else {
        p.map(|_| ())
    };
    assert!(matches!(lost, Err(Error::AlreadyExists(_))), "{lost:?}");

    // sequentially, both ways round
    providers.create(org_a.id, provider("pool")).await.unwrap();
    assert!(matches!(
        groups
            .create(org_a.id, "pool", "pool", "round_robin", None)
            .await,
        Err(Error::AlreadyExists(_))
    ));
    groups
        .create(org_a.id, "fleet", "fleet", "round_robin", None)
        .await
        .unwrap();
    assert!(matches!(
        providers.create(org_a.id, provider("fleet")).await,
        Err(Error::AlreadyExists(_))
    ));

    // another org takes every one of them, as a provider or as a group
    providers.create(org_b.id, provider("edge")).await.unwrap();
    groups
        .create(org_b.id, "pool", "pool", "round_robin", None)
        .await
        .unwrap();
    providers.create(org_b.id, provider("fleet")).await.unwrap();
}

#[tokio::test]
async fn deleting_a_team_project_or_key_drops_its_mcp_tool_group_assignments() {
    if !super::super::test_database::is_configured() {
        eprintln!("skipping: {} not set", super::super::test_database::URL_ENV);
        return;
    }
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let org = OrgRepo(&pool).create("acme", "acme").await.unwrap();
    let teams = TeamRepo(&pool);
    let projects = ProjectRepo(&pool);
    let keys = VirtualKeyRepo(&pool);
    let lone_team = teams.create(org.id, "lone").await.unwrap();
    let team = teams.create(org.id, "core").await.unwrap();
    let lone_project = projects.create(team.id, "lone").await.unwrap();
    let key_project = projects.create(team.id, "keys").await.unwrap();
    let keys_ref = &keys;
    let new_key = |hash, prefix| async move {
        keys_ref
            .create(
                key_project.id,
                NewVirtualKey {
                    key_hash: hash,
                    key_prefix: prefix,
                    name: None,
                    models: &[],
                    providers: &[],
                    cache_enabled: None,
                    created_by: None,
                    expires_at: None,
                    purpose: None,
                },
            )
            .await
            .unwrap()
    };
    let deleted_key = new_key("hash-a", "sk-a").await;
    let cascaded_key = new_key("hash-b", "sk-b").await;

    let group = McpToolGroupRepo(&pool)
        .create(NewMcpToolGroup {
            org_id: org.id,
            name: "docs",
            slug: "docs",
            description: "",
            enabled: true,
            tools: &serde_json::json!([]),
            assignments: &[
                ("team".to_string(), lone_team.id),
                ("project".to_string(), lone_project.id),
                ("virtual_key".to_string(), deleted_key.id),
                ("virtual_key".to_string(), cascaded_key.id),
            ],
        })
        .await
        .unwrap();
    let assignments = McpToolGroupAssignmentRepo(&pool);
    let version = || async {
        sqlx::query_scalar::<_, i64>("select version from config_version where id = 1")
            .fetch_one(&pool)
            .await
            .unwrap()
    };
    let subjects = || async {
        let mut ids: Vec<Uuid> = assignments
            .list_for_group(group.id)
            .await
            .unwrap()
            .into_iter()
            .map(|(_, id)| id)
            .collect();
        ids.sort();
        ids
    };
    assert_eq!(subjects().await.len(), 4);

    keys.delete(deleted_key.id).await.unwrap();
    assert!(!subjects().await.contains(&deleted_key.id));

    // deleting a team or a project bumps `config_version` through the tables
    // it cascades into whatever happens to the assignments, so the sweep is
    // measured against the same delete of a subject with none: it adds exactly
    // one bump, and a subject with no assignment adds none (a bump wakes every
    // gateway)
    let unassigned_team = teams.create(org.id, "unassigned").await.unwrap();
    let unassigned_project = projects.create(team.id, "unassigned").await.unwrap();
    let before = version().await;
    teams.delete(unassigned_team.id).await.unwrap();
    let team_baseline = version().await - before;
    let before = version().await;
    projects.delete(unassigned_project.id).await.unwrap();
    let project_baseline = version().await - before;

    let before = version().await;
    projects.delete(lone_project.id).await.unwrap();
    assert!(!subjects().await.contains(&lone_project.id));
    assert_eq!(
        version().await - before,
        project_baseline + 1,
        "the sweep reaches the gateway"
    );

    let before = version().await;
    teams.delete(lone_team.id).await.unwrap();
    assert!(!subjects().await.contains(&lone_team.id));
    assert_eq!(
        version().await - before,
        team_baseline + 1,
        "so does a team's"
    );

    // a project takes its keys with it, and their assignments follow
    projects.delete(key_project.id).await.unwrap();
    assert!(subjects().await.is_empty());
}

/// #3476: two organizations' `gpt-4o` carry their own labels. A label naming
/// an organization shadows the deployment-wide one of the same key for that
/// organization's model only, and uniqueness holds per organization with the
/// deployment-wide labels counted as one owner.
#[tokio::test]
async fn model_labels_are_keyed_by_organization_and_fall_back_to_the_deployment() {
    if !super::super::test_database::is_configured() {
        eprintln!("skipping: {} not set", super::super::test_database::URL_ENV);
        return;
    }
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let orgs = OrgRepo(&pool);
    let acme = orgs.create("acme", "acme").await.unwrap();
    let globex = orgs.create("globex", "globex").await.unwrap();
    for org in [acme.id, globex.id] {
        let team = TeamRepo(&pool).create(org, "core").await.unwrap();
        let project = ProjectRepo(&pool).create(team.id, "default").await.unwrap();
        RouteRepo(&pool)
            .create(project.id, "gpt-4o", "power_of_two")
            .await
            .unwrap();
    }

    let labels = LabelRepo(&pool);
    let label = |org_id: Option<Uuid>, key: &'static str, value: &'static str| CustomLabelInput {
        subject_type: "model",
        subject_id: "gpt-4o",
        org_id,
        key,
        value: Some(value),
    };
    // deployment-wide, then each org's own `tier`
    labels
        .create_custom(label(None, "tier", "standard"))
        .await
        .unwrap()
        .expect("the deployment's label");
    labels
        .create_custom(label(None, "family", "gpt"))
        .await
        .unwrap()
        .expect("a second deployment label");
    let acme_tier = labels
        .create_custom(label(Some(acme.id), "tier", "gold"))
        .await
        .unwrap()
        .expect("acme's own tier beside the deployment's");
    assert_eq!(acme_tier.org_id, Some(acme.id));
    labels
        .create_custom(label(Some(globex.id), "tier", "bronze"))
        .await
        .unwrap()
        .expect("globex's own tier beside acme's");

    // one key per owner: a second deployment-wide `tier` collides, null
    // counted as one value, and so does a second of acme's
    assert!(labels
        .create_custom(label(None, "tier", "again"))
        .await
        .unwrap()
        .is_none());
    assert!(labels
        .create_custom(label(Some(acme.id), "tier", "again"))
        .await
        .unwrap()
        .is_none());

    let applying = |org: Uuid| {
        let labels = &labels;
        async move {
            let mut rows: Vec<(String, String, Option<Uuid>)> = labels
                .list_models_for_org(org, LabelFilter::default())
                .await
                .unwrap()
                .into_iter()
                .map(|l| (l.key, l.value.unwrap_or_default(), l.org_id))
                .collect();
            rows.sort();
            rows
        }
    };
    // each org reads its own tier, never the other's, and the deployment's
    // family it has not overridden
    assert_eq!(
        applying(acme.id).await,
        vec![
            ("family".into(), "gpt".into(), None),
            ("tier".into(), "gold".into(), Some(acme.id)),
        ]
    );
    assert_eq!(
        applying(globex.id).await,
        vec![
            ("family".into(), "gpt".into(), None),
            ("tier".into(), "bronze".into(), Some(globex.id)),
        ]
    );
    // a filter applies to what the org reads, not to the rows it shadows
    let gold = labels
        .list_models_for_org(
            globex.id,
            LabelFilter {
                value: Some("gold"),
                ..LabelFilter::default()
            },
        )
        .await
        .unwrap();
    assert!(gold.is_empty(), "{gold:?}");

    assert!(labels.org_has_model(acme.id, "gpt-4o").await.unwrap());
    assert!(!labels.org_has_model(acme.id, "claude").await.unwrap());

    // only a model label names an organization
    let route_label = labels
        .create_custom(CustomLabelInput {
            subject_type: "route",
            subject_id: &Uuid::new_v4().to_string(),
            org_id: Some(acme.id),
            key: "tier",
            value: None,
        })
        .await;
    assert!(route_label.is_err(), "{route_label:?}");

    // deleting an org takes its labels with it and leaves the rest
    sqlx::query("delete from orgs where id = $1")
        .bind(acme.id)
        .execute(&pool)
        .await
        .unwrap();
    let left = labels.list(LabelFilter::default()).await.unwrap();
    assert_eq!(left.len(), 3, "{left:?}");
    assert!(left.iter().all(|l| l.org_id != Some(acme.id)));
}
