import re

with open("crates/rolter-control/tests/ux_pipeline.rs", "r") as f:
    content = f.read()

# Instead of hardcoded datetimes in ux_pipeline.rs, we can use a dynamic one to avoid MAX_CLOCK_BEHIND drift.

replace_fn = """async fn a_dashboard_batch_lands_in_clickhouse_with_its_own_screen_action_and_ts() {
    let ch_url = skip_without_stack!();
    let http = reqwest::Client::new();
    ensure_table(&http, &ch_url).await;

    let db = fresh_db().await;
    let pool = db.pool().clone();
    // the app runs the migrations, so it is built before anything is seeded
    let app = rolter_control::test_app_with_clickhouse(pool.clone(), &ch_url)
        .await
        .unwrap();
    let addr = serve(app).await;
    let (user_id, token) = seed_session(&pool, "ux-pipeline@example.com").await;

    let session = session_id();

    let now = chrono::Utc::now();
    let ts1 = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let ts2 = (now + std::time::Duration::from_millis(4500)).to_rfc3339_opts(chrono::SecondsFormat::Millis, true);

    let batch = json!({"events": [
        {
            "event_id": "ux-e2e-1",
            "ts": ts1,
            "screen": "providers",
            "action": "screen_view",
            "session_id": session,
            "from_screen": "dashboard",
            "app_version": "0.0.0-test",
        },
        {
            "event_id": "ux-e2e-2",
            "ts": ts2,
            "screen": "providers",
            "action": "form_submit",
            "target": "provider-sheet",
            "outcome": "ok",
            "duration_ms": 1234,
            "session_id": session,
            "app_version": "0.0.0-test",
        },
    ]});

    let response = http
        .post(format!("http://{addr}/api/v1/ui-events"))
        .bearer_auth(&token)"""

# Also fix the assert
#         assert_eq!(ch_rows[0].ts, "2026-09-20 12:14:57.830");
#         assert_eq!(ch_rows[1].ts, "2026-09-20 12:15:02.330");

# Let's find the exact text in the file.
import sys

match = re.search(r'async fn a_dashboard_batch_lands_in_clickhouse_with_its_own_screen_action_and_ts\(\) \{.*\.bearer_auth\(&token\)', content, re.DOTALL)
if match:
    content = content[:match.start()] + replace_fn + content[match.end():]
else:
    print("Could not find function body to replace")
    sys.exit(1)

# Now find the assertions
#     assert_eq!(ch_rows[0].ts, "2026-09-18 10:00:00.250");
#     assert_eq!(ch_rows[1].ts, "2026-09-18 10:00:04.750");

# Replace with:
#     assert_eq!(ch_rows[0].ts, ts1.replace("T", " ")[..23]);
#     assert_eq!(ch_rows[1].ts, ts2.replace("T", " ")[..23]);

match2 = re.search(r'assert_eq!\(ch_rows\[0\]\.ts, "[^"]*"\);\s*assert_eq!\(ch_rows\[1\]\.ts, "[^"]*"\);', content)
if match2:
    replace_asserts = """assert_eq!(ch_rows[0].ts, ts1.replace("T", " ")[..23]);
        assert_eq!(ch_rows[1].ts, ts2.replace("T", " ")[..23]);"""
    content = content[:match2.start()] + replace_asserts + content[match2.end():]
else:
    print("Could not find assertions to replace")
    sys.exit(1)

with open("crates/rolter-control/tests/ux_pipeline.rs", "w") as f:
    f.write(content)
print("Fixed ux_pipeline")
