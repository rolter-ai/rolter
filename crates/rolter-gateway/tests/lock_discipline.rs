//! Source-level drift guard for lock discipline on the data plane (#1049).
//!
//! A `std::sync::Mutex` poisons itself when a thread panics while holding it,
//! and every later `.lock().unwrap()` then panics too — a single transient
//! panic becomes a permanent outage that only a restart clears. The breaker,
//! cooldown, load and health registries are consulted on every upstream
//! attempt, so the blast radius is all traffic.
//!
//! The gateway therefore uses `parking_lot::Mutex`, which has no poison state
//! at all. This test keeps it that way: it is the same shape of guard as the
//! RBAC matrix drift test in `rolter-control`.

use std::path::{Path, PathBuf};

/// Every `.rs` file under the `src/` of the gateway and of `rolter-balancer`.
/// The balancer runs inside the gateway's request path (every pick of a route
/// reads it, every completed request writes it), so it is held to the same rule
/// (#3388).
fn sources() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("src/ is readable") {
            let path = entry.expect("readable dir entry").path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    walk(&manifest.join("src"), &mut out);
    walk(&manifest.join("../rolter-balancer/src"), &mut out);
    out.sort();
    out
}

#[test]
fn the_guard_reads_the_balancer_as_well() {
    let files = sources();
    for expected in [
        "rolter-balancer/src/predictor.rs",
        "rolter-gateway/src/lib.rs",
    ] {
        assert!(
            files
                .iter()
                .any(|path| path.to_string_lossy().contains(expected)),
            "{expected} is not scanned"
        );
    }
}

/// `std::sync::Mutex` is the poisonable one. It must not come back, in either
/// the imported or the fully-qualified spelling.
#[test]
fn the_gateway_never_uses_a_poisonable_mutex() {
    let mut offenders = Vec::new();
    for path in sources() {
        let text = std::fs::read_to_string(&path).expect("source file is utf-8");
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.starts_with("//") || line.starts_with("//!") {
                continue;
            }
            if line.contains("std::sync::Mutex")
                || line.contains("use std::sync::{Arc, Mutex}")
                || line.contains("std::sync::MutexGuard")
                // a grouped import, e.g. `use std::sync::{atomic::AtomicU64, Mutex}`
                || (line.starts_with("use std::sync::{") && line.contains("Mutex"))
            {
                offenders.push(format!("{}:{}: {line}", path.display(), n + 1));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "std::sync::Mutex poisons on panic and would brick the request path; \
         use parking_lot::Mutex instead:\n{}",
        offenders.join("\n")
    );
}

/// The failure mode this guards is `.lock().unwrap()` specifically: with
/// `parking_lot` there is nothing to unwrap, so its presence means a std mutex
/// crept back in under a different spelling.
#[test]
fn no_lock_unwrap_remains_on_any_gateway_path() {
    let mut offenders = Vec::new();
    for path in sources() {
        let text = std::fs::read_to_string(&path).expect("source file is utf-8");
        // normalise the multi-line `.lock()\n    .unwrap()` chain rustfmt
        // produces, so it cannot hide from a line-wise scan
        let flattened = text.replace(".lock()\n", ".lock()").replace('\t', " ");
        for (n, line) in flattened.lines().enumerate() {
            let squashed: String = line.chars().filter(|c| !c.is_whitespace()).collect();
            if squashed.contains(".lock().unwrap()") || squashed.contains(".lock().expect(") {
                offenders.push(format!("{}:{}: {}", path.display(), n + 1, line.trim()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "a poisonable lock is back on a gateway path:\n{}",
        offenders.join("\n")
    );
}
