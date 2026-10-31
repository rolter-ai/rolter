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

/// The full path of every name a `use` tree imports, so `use std::{sync::{Arc,
/// Mutex}}` is read as `std::sync::Arc` and `std::sync::Mutex` however rustfmt
/// wrapped it.
fn use_paths(tree: &str, prefix: &str, out: &mut Vec<String>) {
    let mut items = Vec::new();
    let (mut depth, mut start) = (0usize, 0usize);
    for (i, c) in tree.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                items.push(&tree[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    items.push(&tree[start..]);
    for item in items.into_iter().map(str::trim).filter(|i| !i.is_empty()) {
        match item.find('{') {
            Some(open) => {
                let head = item[..open].trim().trim_end_matches("::");
                let inner = item[open + 1..].trim_end().trim_end_matches('}');
                use_paths(inner, &format!("{prefix}{head}::"), out);
            }
            None => {
                let name = item.split(" as ").next().unwrap_or(item).trim();
                out.push(format!("{prefix}{name}"));
            }
        }
    }
}

/// The lines of `text` that import or spell out a `std::sync` mutex, each as
/// `(line number, the offending statement)`. A `use` statement is joined across
/// the lines it spans before it is read.
fn std_mutex_uses(text: &str) -> Vec<(usize, String)> {
    let mut found = Vec::new();
    let mut pending: Option<(usize, String)> = None;
    for (n, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.starts_with("//") {
            continue;
        }
        if pending.is_none() {
            let starts_use = ["use ", "pub use ", "pub(crate) use ", "pub(super) use "]
                .iter()
                .any(|prefix| line.starts_with(prefix));
            if starts_use {
                pending = Some((n + 1, String::new()));
            }
        }
        match pending.as_mut() {
            Some((_, joined)) => {
                joined.push_str(line);
                joined.push(' ');
                if !line.contains(';') {
                    continue;
                }
                let (first, joined) = pending.take().expect("a statement in progress");
                let body = joined
                    .split_once("use ")
                    .map_or(joined.as_str(), |(_, rest)| rest)
                    .trim()
                    .trim_end_matches(';')
                    .trim();
                let mut paths = Vec::new();
                use_paths(body, "", &mut paths);
                if paths.iter().any(|path| {
                    path == "std::sync::Mutex"
                        || path == "std::sync::MutexGuard"
                        || path.starts_with("std::sync::Mutex::")
                }) {
                    found.push((first, joined.trim().to_string()));
                }
            }
            None => {
                if line.contains("std::sync::Mutex") {
                    found.push((n + 1, line.to_string()));
                }
            }
        }
    }
    found
}

/// The clause above has to see the spellings that slip past a line-wise scan.
#[test]
fn the_mutex_scan_reads_wrapped_and_nested_imports() {
    for bad in [
        "use std::sync::Mutex;",
        "use std::sync::{Arc, Mutex};",
        "use std::sync::{\n    atomic::AtomicU64,\n    Mutex,\n};",
        "use std::{collections::HashMap, sync::Mutex};",
        "use std::{\n    collections::HashMap,\n    sync::{Arc, Mutex},\n};",
        "use std::sync::Mutex as Lock;",
        "pub(crate) use std::sync::MutexGuard;",
        "let m = std::sync::Mutex::new(1);",
    ] {
        assert!(!std_mutex_uses(bad).is_empty(), "missed: {bad}");
    }
    for good in [
        "use parking_lot::Mutex;",
        "use std::sync::{atomic::AtomicU64, Arc, RwLock};",
        "use std::{collections::HashMap, sync::Arc};",
        "use parking_lot::{Mutex, RwLock};\nuse std::sync::Arc;",
        "// use std::sync::Mutex;",
    ] {
        assert!(std_mutex_uses(good).is_empty(), "false positive: {good}");
    }
}

/// `std::sync::Mutex` is the poisonable one. It must not come back, in either
/// the imported or the fully-qualified spelling.
#[test]
fn the_gateway_never_uses_a_poisonable_mutex() {
    let mut offenders = Vec::new();
    for path in sources() {
        let text = std::fs::read_to_string(&path).expect("source file is utf-8");
        for (n, statement) in std_mutex_uses(&text) {
            offenders.push(format!("{}:{n}: {statement}", path.display()));
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
