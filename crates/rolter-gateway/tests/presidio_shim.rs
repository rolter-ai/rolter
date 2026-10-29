//! The gateway's own `restore_stream` client against the reference Presidio
//! shim in `examples/presidio-restore-shim/` (#3444).
//!
//! The shim's pytest suite drives it with a hand-written client that speaks the
//! exchange as documented, and the gateway's unit tests drive `restore_stream`
//! against a stand-in written in Rust. Neither would notice the two drifting
//! apart from the documented protocol (framing, channel ids, close and reset
//! semantics, the JSON escaping of a `"format": "json"` channel). This does: it
//! runs the real shim under uvicorn and a real gateway route whose restore leg
//! points at it, then streams an answer through.
//!
//! It needs Python with the shim's requirements installed. `ROLTER_TEST_SHIM_PYTHON`
//! names that interpreter (CI points it at a virtualenv built from
//! `examples/presidio-restore-shim/requirements.txt`) and, once set, a missing
//! dependency fails the test instead of skipping it, so CI cannot pass this file
//! vacuously. Left unset, `python3` is tried and the test skips with the reason
//! when it cannot import the shim's dependencies.

use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use rolter_core::{
    BalancingStrategy, GatewayConfig, ModelRoute, ProviderConfig, ProviderKind, Target,
};
use serde_json::{json, Value};

const TOKEN: &str = "tok-shim-1";
const PLAIN: &str = "<EMAIL_ADDRESS_1>";
const QUOTED: &str = "<PERSON_2>";
const PLAIN_VALUE: &str = "ops@corp.com";
// a name carrying a double quote, which breaks the tool-call arguments below
// unless the shim JSON-escapes it for the json channel
const QUOTED_VALUE: &str = "Ada \"Ace\" Lovelace";

/// Starts the shim on a free loopback port and stops it when dropped.
struct Shim {
    child: Child,
    addr: SocketAddr,
}

impl Drop for Shim {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn shim_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/presidio-restore-shim")
}

/// The interpreter to run the shim with, or the reason there is none.
fn python() -> Result<String, String> {
    let named = std::env::var("ROLTER_TEST_SHIM_PYTHON").ok();
    let python = named.clone().unwrap_or_else(|| "python3".to_string());
    let probe = Command::new(&python)
        .args(["-c", "import uvicorn, starlette, presidio_anonymizer"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output();
    let failure = match probe {
        Ok(out) if out.status.success() => return Ok(python),
        Ok(out) => String::from_utf8_lossy(&out.stderr).trim().to_string(),
        Err(error) => error.to_string(),
    };
    let reason = format!("{python} cannot import the shim's requirements: {failure}");
    if named.is_some() {
        panic!("ROLTER_TEST_SHIM_PYTHON is set but unusable: {reason}");
    }
    Err(reason)
}

/// How many times start-up is attempted on a fresh port before the test fails.
const START_ATTEMPTS: usize = 4;

/// Start the shim, retrying on a fresh port when the child exits early. The
/// port is probed by binding and dropping it, so another test in the binary
/// (or on the machine) can take it before uvicorn does and uvicorn then exits
/// with `EADDRINUSE`; that is a retry, not a 30 second wait followed by a
/// misleading panic.
fn start_shim(python: &str) -> Shim {
    let mut failures = Vec::new();
    for _ in 0..START_ATTEMPTS {
        match try_start_shim(python) {
            Ok(shim) => return shim,
            Err(failure) => failures.push(failure),
        }
    }
    panic!(
        "the shim did not start in {START_ATTEMPTS} attempts:\n{}",
        failures.join("\n---\n")
    );
}

fn try_start_shim(python: &str) -> Result<Shim, String> {
    let port = {
        let probe = TcpListener::bind("127.0.0.1:0").expect("a free port");
        probe.local_addr().expect("a bound address").port()
    };
    let mapping = json!({ PLAIN: PLAIN_VALUE, QUOTED: QUOTED_VALUE }).to_string();
    // `app_from_env` starts with an empty store, so the harness builds the app
    // itself with the mapping a sanitize service would have written
    let script = "import json, sys, uvicorn; sys.path.insert(0, sys.argv[1]); import shim; \
                  store = shim.InMemoryStore(); store.put(sys.argv[3], json.loads(sys.argv[4])); \
                  uvicorn.run(shim.create_app(store), host='127.0.0.1', port=int(sys.argv[2]), \
                  log_level='warning')";
    let child = Command::new(python)
        .args(["-c", script])
        .arg(shim_dir())
        .arg(port.to_string())
        .arg(TOKEN)
        .arg(mapping)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python starts");
    let mut shim = Shim {
        child,
        addr: SocketAddr::from(([127, 0, 0, 1], port)),
    };
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = shim.child.try_wait().expect("the child can be polled") {
            let mut stderr = String::new();
            if let Some(mut pipe) = shim.child.stderr.take() {
                let _ = std::io::Read::read_to_string(&mut pipe, &mut stderr);
            }
            return Err(format!(
                "the shim exited with {status} on port {port}: {}",
                stderr.trim()
            ));
        }
        if std::net::TcpStream::connect_timeout(&shim.addr, Duration::from_millis(200)).is_ok() {
            // a process that is not ours may be what answered: the child has
            // to still be alive a moment later for the port to be its own
            std::thread::sleep(Duration::from_millis(150));
            if shim
                .child
                .try_wait()
                .expect("the child can be polled")
                .is_none()
            {
                return Ok(shim);
            }
            continue;
        }
        if Instant::now() >= deadline {
            return Err(format!("the shim never listened on port {port} in 30s"));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

async fn serve(app: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

/// The sanitize leg: it hands back the restoration token the shim knows.
async fn sanitize(Json(body): Json<Value>) -> Json<Value> {
    Json(json!({
        "content": body.get("content").cloned().unwrap_or(Value::Null),
        "findings": [{"entity_type": "EMAIL_ADDRESS", "count": 1, "placeholders": [PLAIN]}],
        "restoration_token": TOKEN,
    }))
}

fn frame(delta: Value) -> String {
    let chunk = json!({"choices": [{"index": 0, "delta": delta}]});
    format!("data: {chunk}\n\n")
}

fn call_arguments(arguments: &str) -> String {
    frame(json!({"tool_calls": [{"index": 0, "function": {"arguments": arguments}}]}))
}

/// A provider that answers with placeholders, each split across two frames: one
/// in prose and one inside the JSON arguments of a tool call.
async fn upstream() -> impl IntoResponse {
    let mut sse = String::new();
    sse.push_str(&frame(json!({"content": "Write to <EMAIL_AD"})));
    sse.push_str(&frame(json!({"content": "DRESS_1> today, "})));
    sse.push_str(&frame(json!({"content": "then call."})));
    sse.push_str(&call_arguments("{\"to\": \"<PERSO"));
    sse.push_str(&call_arguments("N_2>\", \"cc\": \"<EMAIL_ADDRESS_1>\"}"));
    sse.push_str("data: [DONE]\n\n");
    (
        [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
        sse,
    )
}

fn config(upstream: SocketAddr, sanitizer: SocketAddr, shim: SocketAddr) -> GatewayConfig {
    let mut config = GatewayConfig::default();
    config.providers.push(ProviderConfig {
        name: "up".to_string(),
        kind: ProviderKind::OpenaiCompatible,
        api_base: format!("http://{upstream}"),
        ..Default::default()
    });
    config.routes.push(ModelRoute {
        model: "test-model".to_string(),
        strategy: BalancingStrategy::RoundRobin,
        targets: vec![Target {
            provider: "up".to_string(),
            ..Default::default()
        }],
        params: Default::default(),
        param_policy: Default::default(),
        advanced: Default::default(),
        cache: None,
        variants: Default::default(),
        tenancy: None,
    });
    config.pii_sanitizer = rolter_core::PiiSanitizerConfig {
        enabled: true,
        url: format!("http://{sanitizer}/sanitize"),
        restore_stream_url: format!("http://{shim}/restore-stream"),
        direction: rolter_core::SanitizeDirection::Request,
        restoration: rolter_core::RestorationPolicy::TrustedDownstream,
        streaming: rolter_core::StreamingResponse::Restore,
        timeout_ms: 10_000,
        ..Default::default()
    };
    config
}

/// The deltas of an SSE body, concatenated per field.
fn collected(sse: &str) -> (String, String) {
    let (mut content, mut arguments) = (String::new(), String::new());
    for data in sse.lines().filter_map(|l| l.strip_prefix("data: ")) {
        let Ok(event) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        let delta = &event["choices"][0]["delta"];
        if let Some(text) = delta["content"].as_str() {
            content.push_str(text);
        }
        if let Some(text) = delta["tool_calls"][0]["function"]["arguments"].as_str() {
            arguments.push_str(text);
        }
    }
    (content, arguments)
}

#[tokio::test]
async fn a_streamed_answer_comes_back_restored_by_the_real_shim() {
    let python = match python() {
        Ok(python) => python,
        Err(reason) => {
            eprintln!("skipped: {reason}");
            return;
        }
    };
    let shim = start_shim(&python);
    let up = serve(Router::new().route("/v1/chat/completions", post(upstream))).await;
    let sanitizer = serve(Router::new().route("/sanitize", post(sanitize))).await;
    let gw = serve(rolter_gateway::build_router_from_config(&config(
        up, sanitizer, shim.addr,
    )))
    .await;

    let resp = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({
            "model": "test-model",
            "stream": true,
            "messages": [{"role": "user", "content": format!("mail {PLAIN_VALUE} now")}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let sse = resp.text().await.unwrap();
    let (content, arguments) = collected(&sse);

    // a placeholder split across two frames is held back, then restored whole
    assert_eq!(
        content,
        format!("Write to {PLAIN_VALUE} today, then call."),
        "unrestored stream: {sse}"
    );
    // the json channel: the restored name carries a quote, so the arguments
    // only parse if the shim escaped it
    let call: Value = serde_json::from_str(&arguments)
        .unwrap_or_else(|error| panic!("tool arguments are not json ({error}): {arguments}"));
    assert_eq!(call["to"], QUOTED_VALUE);
    assert_eq!(call["cc"], PLAIN_VALUE);
    assert!(!sse.contains("EMAIL_ADDRESS_1") && !sse.contains("PERSON_2"));
}
