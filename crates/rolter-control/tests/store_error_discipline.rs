//! Every control-plane store error keeps its kind (#3148).
//!
//! `rolter_store::postgres::store_err` turns a lock or statement timeout into
//! `Error::StoreTimeout`, which the API answers with the coded `store_timeout`
//! 504, and a unique violation into `Error::AlreadyExists`. Flattening either
//! into a string — `Error::Store(e.to_string())`, or an auth path's
//! `AuthError::Internal(err.to_string())` — throws the kind away, and the
//! caller gets an opaque 500 that it then retries until the request bound
//! answers 504 anyway. This scans the crate's sources so a new call site cannot
//! bring the pattern back.

use std::fs;
use std::path::Path;

/// spellings that stringify an error into a store or auth error, where the
/// error should go through `store_err` or `AuthError::from` instead
const FORBIDDEN: &[&str] = &[
    "Error::Store(e.to_string())",
    "Error::Store(err.to_string())",
    "Error::Store(error.to_string())",
    "AuthError::Internal(e.to_string())",
    "AuthError::Internal(err.to_string())",
    "AuthError::Internal(error.to_string())",
];

fn rust_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in fs::read_dir(dir).expect("read the source directory") {
        let path = entry.expect("read a directory entry").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn no_control_plane_call_site_stringifies_a_store_error() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    assert!(
        !files.is_empty(),
        "found no sources under {}",
        src.display()
    );

    let mut offences = Vec::new();
    for file in files {
        let text = fs::read_to_string(&file).expect("read a source file");
        // spacing differs once rustfmt splits a call over lines, so compare
        // with the whitespace taken out
        for (number, line) in text.lines().enumerate() {
            let squeezed: String = line.chars().filter(|c| !c.is_whitespace()).collect();
            if FORBIDDEN.iter().any(|pattern| squeezed.contains(pattern)) {
                offences.push(format!(
                    "{}:{}: {}",
                    file.display(),
                    number + 1,
                    line.trim()
                ));
            }
        }
    }
    assert!(
        offences.is_empty(),
        "map a store error with rolter_store::postgres::store_err (or AuthError::from on an \
         auth path) so a timeout stays a coded 504:\n{}",
        offences.join("\n")
    );
}
