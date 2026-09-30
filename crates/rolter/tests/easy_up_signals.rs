//! `rolter easy-up` must end on Ctrl-C and `SIGTERM` (#1963).
//!
//! Both planes install their own signal handler, so the process used to stay up
//! on the control plane after the gateway had drained. These tests spawn the
//! real binary and bound how long it may take to exit.

#![cfg(unix)]

use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// generous: a slow CI box still finishes a drain well inside this, while the
/// bug this guards against never exits at all
const EXIT_BOUND: Duration = Duration::from_secs(15);

fn reserve_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

async fn wait_until_serving(port: u16, child: &mut Child) {
    for _ in 0..800 {
        if let Ok(Some(status)) = child.try_wait() {
            panic!("exited before serving: {status}");
        }
        if let Ok(resp) = reqwest::get(format!("http://127.0.0.1:{port}/healthz")).await {
            if resp.status().is_success() {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("never became reachable on {port}");
}

async fn wait_for_exit(child: &mut Child, what: &str) -> std::process::ExitStatus {
    let deadline = tokio::time::Instant::now() + EXIT_BOUND;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if tokio::time::Instant::now() >= deadline {
            let _ = child.kill();
            panic!("{what} still running {EXIT_BOUND:?} after the signal");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn easy_up_exits_on(signal: libc::c_int) {
    let gw = reserve_port();
    let control = reserve_port();
    let dir = std::env::temp_dir().join(format!("rolter-easy-up-signal-{gw}-{control}"));
    std::fs::create_dir_all(dir.join("ui")).unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_rolter"))
        .arg("easy-up")
        .arg("--config")
        .arg(dir.join("rolter.toml"))
        .arg("--ui-dir")
        .arg(dir.join("ui"))
        .arg("--gateway-port")
        .arg(gw.to_string())
        .arg("--control-port")
        .arg(control.to_string())
        .env_remove("ROLTER_DATABASE_URL")
        .env_remove("ROLTER_ADMIN_TOKEN")
        .env_remove("ROLTER_REDIS_URL")
        .env_remove("CLICKHOUSE_URL")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_until_serving(gw, &mut child).await;
    wait_until_serving(control, &mut child).await;

    assert_eq!(unsafe { libc::kill(child.id() as libc::pid_t, signal) }, 0);
    let status = wait_for_exit(&mut child, "easy-up").await;
    assert!(status.success(), "easy-up exited uncleanly: {status}");
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn easy_up_exits_on_sigterm() {
    easy_up_exits_on(libc::SIGTERM).await;
}

#[tokio::test]
async fn easy_up_exits_on_sigint() {
    easy_up_exits_on(libc::SIGINT).await;
}
