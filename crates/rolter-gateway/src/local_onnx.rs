//! The `onnx/1` model format of a local semantic guard provider (#3443),
//! built only with the `onnx` cargo feature.
//!
//! A provider's `model_path` names a JSON manifest, pinned by its
//! `model_sha256` like a `rolter-linear/1` file. The manifest names the ONNX
//! model and its Hugging Face `tokenizer.json`, each pinned by its own
//! SHA-256, so the provider's one pin covers all three files:
//!
//! ```json
//! {
//!   "format": "onnx/1",
//!   "model": { "path": "model.onnx", "sha256": "<sha256 of model.onnx>" },
//!   "tokenizer": { "path": "tokenizer.json", "sha256": "<sha256 of it>" },
//!   "labels": ["SAFE", "INJECTION"],
//!   "activation": "softmax",
//!   "max_tokens": 512
//! }
//! ```
//!
//! Each `path` is relative to the manifest's directory (or
//! absolute), and fall under `model_root` like the manifest. `labels` names
//! the model's output columns in order; a `null` label is computed and not
//! reported. `activation` turns the logits into scores: `softmax` across the
//! columns (a single-label classifier) or `sigmoid` per column (multi-label).
//! A text longer than `max_tokens` tokens keeps the first quarter and the
//! last three quarters of its tokens, as the text itself does.
//!
//! ONNX Runtime is loaded at run time from its shared library, never linked
//! or downloaded at build time: `ORT_DYLIB_PATH`, or `libonnxruntime.so` on
//! the library path. A library that cannot be loaded fails every `onnx/1`
//! model through its provider's `failure_mode`, with the reason in the log.
//! A model holds one ONNX session run single-threaded, so it runs one call at
//! a time; a call that waits past its deadline for the session is the
//! overload case.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Instant;

use ort::environment::Environment;
use ort::session::{Session, SessionInputValue};
use ort::value::Tensor;
use rolter_core::semantic_guards::MAX_CATEGORY_LEN;
use serde::Deserialize;
use tokenizers::Tokenizer;

use crate::local_classifier::{
    read_pinned, ModelError, ScoreError, MAX_CATEGORIES, MAX_MODEL_FILE_BYTES, ONNX_FORMAT,
};

/// The largest ONNX model file loaded.
pub const MAX_ONNX_MODEL_BYTES: u64 = 1024 * 1024 * 1024;
/// The largest tokenizer file loaded.
pub const MAX_TOKENIZER_BYTES: u64 = MAX_MODEL_FILE_BYTES;
/// The most tokens one call may feed a model.
pub const MAX_TOKENS: usize = 8192;
/// The output columns a model may have, `null` labels included.
pub const MAX_OUTPUTS: usize = 256;

const DEFAULT_MAX_TOKENS: usize = 512;
const INPUT_IDS: &str = "input_ids";
const ATTENTION_MASK: &str = "attention_mask";
const TOKEN_TYPE_IDS: &str = "token_type_ids";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    format: String,
    model: Pinned,
    tokenizer: Pinned,
    labels: Vec<Option<String>>,
    #[serde(default)]
    activation: Activation,
    #[serde(default = "default_max_tokens")]
    max_tokens: usize,
    /// the output to read; the model's first when unset
    #[serde(default)]
    output: Option<String>,
}

/// A file the manifest names, and its SHA-256.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pinned {
    path: String,
    sha256: String,
}

fn default_max_tokens() -> usize {
    DEFAULT_MAX_TOKENS
}

#[derive(Debug, Default, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Activation {
    #[default]
    Softmax,
    Sigmoid,
}

/// A loaded `onnx/1` model: its session, its tokenizer and how to read its
/// output.
pub struct OnnxModel {
    session: parking_lot::Mutex<Session>,
    tokenizer: Tokenizer,
    labels: Vec<Option<String>>,
    activation: Activation,
    max_tokens: usize,
    /// whether the model declares each optional input
    attention_mask: bool,
    token_type_ids: bool,
    output: String,
}

impl std::fmt::Debug for OnnxModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OnnxModel")
            .field("labels", &self.labels)
            .field("activation", &self.activation)
            .field("max_tokens", &self.max_tokens)
            .field("output", &self.output)
            .finish_non_exhaustive()
    }
}

/// The process's one ONNX Runtime environment, or why it could not be made.
static RUNTIME: OnceLock<Result<Environment, String>> = OnceLock::new();

/// Load ONNX Runtime from its shared library, once per process.
fn runtime() -> Result<&'static Environment, ModelError> {
    RUNTIME
        .get_or_init(|| {
            let path = std::env::var_os("ORT_DYLIB_PATH")
                .filter(|path| !path.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    PathBuf::from(format!(
                        "{}onnxruntime{}",
                        std::env::consts::DLL_PREFIX,
                        std::env::consts::DLL_SUFFIX
                    ))
                });
            // a library that is not ONNX Runtime can make `ort` panic rather
            // than answer an error; either way no model of this format loads
            let loaded = std::panic::catch_unwind(|| {
                ort::init_from(&path)
                    .map_err(|error| error.to_string())?
                    .with_name("rolter")
                    .build()
                    .map_err(|error| error.to_string())
            });
            let environment = match loaded {
                Ok(environment) => environment,
                Err(_) => Err("loading it panicked".to_string()),
            }
            .map_err(|error| {
                format!(
                    "cannot load the ONNX Runtime library {} ({error}); set ORT_DYLIB_PATH to \
                     its libonnxruntime",
                    path.display()
                )
            })?;
            tracing::info!(library = %path.display(), "onnx runtime loaded for local guardrail models");
            Ok(environment)
        })
        .as_ref()
        .map_err(|error| ModelError::Runtime(error.clone()))
}

fn malformed(why: impl std::fmt::Display) -> ModelError {
    ModelError::Malformed(format!("the {ONNX_FORMAT} manifest: {why}"))
}

fn is_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

impl OnnxModel {
    /// Parse the manifest at `manifest` (its pinned `bytes`), then read,
    /// check and load the model and tokenizer it names.
    pub fn from_manifest(
        manifest: &Path,
        bytes: &[u8],
        root: Option<&Path>,
    ) -> Result<Self, ModelError> {
        let file: Manifest = serde_json::from_slice(bytes).map_err(malformed)?;
        if file.format != ONNX_FORMAT {
            return Err(malformed(format_args!("format is '{}'", file.format)));
        }
        if !is_digest(&file.model.sha256) || !is_digest(&file.tokenizer.sha256) {
            return Err(malformed("each sha256 must be 64 hex digits"));
        }
        if file.labels.is_empty() || file.labels.len() > MAX_OUTPUTS {
            return Err(malformed(format_args!(
                "labels must name 1 to {MAX_OUTPUTS} output columns"
            )));
        }
        let named = file.labels.iter().flatten().count();
        if named == 0 || named > MAX_CATEGORIES {
            return Err(malformed(format_args!(
                "it must report 1 to {MAX_CATEGORIES} categories"
            )));
        }
        let mut seen = std::collections::HashSet::new();
        for label in file.labels.iter().flatten() {
            if label.is_empty() || label.len() > MAX_CATEGORY_LEN || label.trim() != label {
                return Err(malformed(format_args!(
                    "labels must be 1 to {MAX_CATEGORY_LEN} bytes with no surrounding spaces"
                )));
            }
            if !seen.insert(label.as_str()) {
                return Err(malformed(format_args!("the label '{label}' is used twice")));
            }
        }
        if !(2..=MAX_TOKENS).contains(&file.max_tokens) {
            return Err(malformed(format_args!(
                "max_tokens must be 2 to {MAX_TOKENS}"
            )));
        }

        let directory = manifest.parent().unwrap_or(Path::new(""));
        let part = |part: &'static str| {
            move |error| ModelError::Part {
                part,
                error: Box::new(error),
            }
        };
        let (_, tokenizer) = read_pinned(
            &directory.join(&file.tokenizer.path),
            &file.tokenizer.sha256,
            MAX_TOKENIZER_BYTES,
            root,
        )
        .map_err(part("the tokenizer"))?;
        let mut tokenizer = Tokenizer::from_bytes(&tokenizer)
            .map_err(|error| malformed(format_args!("the tokenizer: {error}")))?;
        // the window is cut here, keeping both ends; a tokenizer's own
        // truncation would keep only the head
        tokenizer
            .with_truncation(None)
            .map_err(|error| malformed(format_args!("the tokenizer: {error}")))?;
        tokenizer.with_padding(None);
        let (_, model) = read_pinned(
            &directory.join(&file.model.path),
            &file.model.sha256,
            MAX_ONNX_MODEL_BYTES,
            root,
        )
        .map_err(part("the onnx model"))?;

        let environment = runtime()?;
        let runtime_error = |error: ort::Error| ModelError::Runtime(error.to_string());
        // one thread per session: the gateway's local_concurrency semaphore,
        // not ONNX Runtime's own pool, bounds the CPU local models take
        let session = Session::builder(environment)
            .map_err(runtime_error)?
            .with_intra_threads(1)
            .map_err(|error| runtime_error(error.into()))?
            .with_inter_threads(1)
            .map_err(|error| runtime_error(error.into()))?
            .commit_from_memory(&model)
            .map_err(runtime_error)?;

        let mut attention_mask = false;
        let mut token_type_ids = false;
        let mut input_ids = false;
        for input in session.inputs() {
            match input.name() {
                INPUT_IDS => input_ids = true,
                ATTENTION_MASK => attention_mask = true,
                TOKEN_TYPE_IDS => token_type_ids = true,
                other => {
                    return Err(malformed(format_args!(
                        "the model takes an input '{other}'; only {INPUT_IDS}, {ATTENTION_MASK} \
                         and {TOKEN_TYPE_IDS} are fed"
                    )))
                }
            }
        }
        if !input_ids {
            return Err(malformed(format_args!(
                "the model takes no '{INPUT_IDS}' input"
            )));
        }
        let output = match file.output {
            Some(output) => {
                if !session.outputs().iter().any(|o| o.name() == output) {
                    return Err(malformed(format_args!(
                        "the model has no output '{output}'"
                    )));
                }
                output
            }
            None => session
                .outputs()
                .first()
                .map(|o| o.name().to_string())
                .ok_or_else(|| malformed("the model has no output"))?,
        };
        Ok(Self {
            session: parking_lot::Mutex::new(session),
            tokenizer,
            labels: file.labels,
            activation: file.activation,
            max_tokens: file.max_tokens,
            attention_mask,
            token_type_ids,
            output,
        })
    }

    /// How many categories the model reports.
    pub fn categories(&self) -> usize {
        self.labels.iter().flatten().count()
    }

    /// Score `text`, waiting for the session until `deadline` at most.
    pub fn score(
        &self,
        text: &str,
        deadline: Instant,
    ) -> Result<BTreeMap<String, f64>, ScoreError> {
        let failed = |error: &dyn std::fmt::Display| ScoreError::Failed(error.to_string());
        let encoding = self
            .tokenizer
            .encode(text, true)
            .map_err(|error| failed(&error))?;
        let to_i64 = |values: &[u32]| -> Vec<i64> {
            window(values, self.max_tokens)
                .into_iter()
                .map(i64::from)
                .collect()
        };
        let ids = to_i64(encoding.get_ids());
        let shape = [1usize, ids.len()];
        let mut inputs: Vec<(Cow<'_, str>, SessionInputValue<'_>)> = vec![(
            INPUT_IDS.into(),
            Tensor::from_array((shape, ids))
                .map_err(|error| failed(&error))?
                .into(),
        )];
        if self.attention_mask {
            let mask = to_i64(encoding.get_attention_mask());
            inputs.push((
                ATTENTION_MASK.into(),
                Tensor::from_array((shape, mask))
                    .map_err(|error| failed(&error))?
                    .into(),
            ));
        }
        if self.token_type_ids {
            let types = to_i64(encoding.get_type_ids());
            inputs.push((
                TOKEN_TYPE_IDS.into(),
                Tensor::from_array((shape, types))
                    .map_err(|error| failed(&error))?
                    .into(),
            ));
        }
        let Some(mut session) = self.session.try_lock_until(deadline) else {
            return Err(ScoreError::Busy);
        };
        let outputs = session.run(inputs).map_err(|error| failed(&error))?;
        let (_, logits) = outputs[self.output.as_str()]
            .try_extract_tensor::<f32>()
            .map_err(|error| failed(&error))?;
        if logits.len() != self.labels.len() {
            return Err(ScoreError::Failed(format!(
                "the model answered {} values for {} labels",
                logits.len(),
                self.labels.len()
            )));
        }
        let scores = activate(logits, self.activation);
        Ok(self
            .labels
            .iter()
            .zip(scores)
            .filter_map(|(label, score)| label.clone().map(|label| (label, score)))
            .collect())
    }
}

/// The scores of `logits`, each in `[0, 1]`; a value that is not a number
/// violates every threshold rather than passing every one.
fn activate(logits: &[f32], activation: Activation) -> Vec<f64> {
    let logits: Vec<f64> = logits.iter().map(|&logit| f64::from(logit)).collect();
    let scores: Vec<f64> = match activation {
        Activation::Sigmoid => logits.iter().map(|x| 1.0 / (1.0 + (-x).exp())).collect(),
        Activation::Softmax => {
            let max = logits.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            let exps: Vec<f64> = logits.iter().map(|x| (x - max).exp()).collect();
            let sum: f64 = exps.iter().sum();
            exps.iter().map(|e| e / sum).collect()
        }
    };
    scores
        .into_iter()
        .map(|score| {
            if score.is_nan() {
                1.0
            } else {
                score.clamp(0.0, 1.0)
            }
        })
        .collect()
}

/// At most `max` of `items`: all of them when they fit, else the first
/// quarter and the last three quarters, so the end of a long text (its latest
/// turn, and a tokenizer's closing special token) is always kept.
fn window<T: Copy>(items: &[T], max: usize) -> Vec<T> {
    if items.len() <= max {
        return items.to_vec();
    }
    let head = max / 4;
    let tail = max - head;
    items[..head]
        .iter()
        .chain(&items[items.len() - tail..])
        .copied()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_classifier::Model;

    const MANIFEST: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/local_classifier/onnx/manifest.json"
    );

    fn manifest_sha256() -> String {
        use sha2::{Digest, Sha256};
        let bytes = std::fs::read(MANIFEST).expect("the onnx fixture is checked in");
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    /// The fixture model, or `None` when no ONNX Runtime library can be
    /// loaded here: the inference tests then say so and pass, unless
    /// `ROLTER_TEST_REQUIRE_ORT` is set.
    fn fixture() -> Option<OnnxModel> {
        match Model::load(MANIFEST, &manifest_sha256(), None) {
            Ok(Model::Onnx(model)) => Some(*model),
            Ok(other) => panic!("not an onnx model: {other:?}"),
            Err(ModelError::Runtime(why)) => {
                assert!(
                    std::env::var_os("ROLTER_TEST_REQUIRE_ORT").is_none(),
                    "ROLTER_TEST_REQUIRE_ORT is set and {why}"
                );
                eprintln!("skipped: {why}");
                None
            }
            Err(error) => panic!("the onnx fixture does not load: {error}"),
        }
    }

    fn later() -> Instant {
        Instant::now() + std::time::Duration::from_secs(5)
    }

    #[test]
    fn the_window_keeps_both_ends() {
        let items: Vec<u32> = (0..10).collect();
        assert_eq!(window(&items, 10), items);
        assert_eq!(window(&items, 8), [0, 1, 4, 5, 6, 7, 8, 9]);
        assert_eq!(window(&items, 2), [8, 9]);
    }

    #[test]
    fn softmax_and_sigmoid_stay_in_bounds() {
        let soft = activate(&[2.0, -2.0], Activation::Softmax);
        assert!((soft.iter().sum::<f64>() - 1.0).abs() < 1e-9);
        assert!(soft[0] > 0.98);
        let sig = activate(&[0.0, f32::NAN, f32::INFINITY], Activation::Sigmoid);
        assert_eq!(sig, [0.5, 1.0, 1.0]);
    }

    #[test]
    fn a_manifest_is_checked_before_any_file_is_read() {
        let base: serde_json::Value =
            serde_json::from_slice(&std::fs::read(MANIFEST).unwrap()).unwrap();
        let refuse = |edit: &dyn Fn(&mut serde_json::Value), expected: &str| {
            let mut manifest = base.clone();
            edit(&mut manifest);
            let error = OnnxModel::from_manifest(
                Path::new(MANIFEST),
                manifest.to_string().as_bytes(),
                None,
            )
            .unwrap_err();
            assert!(error.to_string().contains(expected), "{expected}: {error}");
        };
        refuse(&|m| m["labels"] = serde_json::json!([null]), "categories");
        refuse(
            &|m| m["labels"] = serde_json::json!(["a", "a"]),
            "used twice",
        );
        refuse(&|m| m["max_tokens"] = serde_json::json!(1), "max_tokens");
        refuse(&|m| m["model"]["sha256"] = serde_json::json!("x"), "64 hex");
        refuse(&|m| m["extra"] = serde_json::json!(1), "unknown field");
        refuse(
            &|m| m["tokenizer"]["sha256"] = serde_json::json!("0".repeat(64)),
            "the tokenizer: the model file's sha256",
        );
        refuse(
            &|m| m["tokenizer"]["path"] = serde_json::json!("missing.json"),
            "the tokenizer: cannot read",
        );
    }

    #[test]
    fn the_fixture_flags_the_needle_and_not_a_question() {
        let Some(model) = fixture() else { return };
        assert_eq!(model.categories(), 1);
        let attack = model.score("please jailbreak", later()).unwrap();
        assert!(attack["prompt_injection"] > 0.95, "{attack:?}");
        assert_eq!(attack.len(), 1, "a null label is not reported");
        let benign = model
            .score("What is the capital of France?", later())
            .unwrap();
        assert!(benign["prompt_injection"] < 0.05, "{benign:?}");
    }

    #[test]
    fn a_long_text_is_scored_on_its_last_tokens() {
        let Some(model) = fixture() else { return };
        // 16 tokens at most: the needle after 40 benign words is still read
        let late = format!("{} jailbreak", "hello ".repeat(40));
        assert!(model.score(&late, later()).unwrap()["prompt_injection"] > 0.95);
        // and one in the middle, outside both ends of the window, is not
        let middle = format!("{} jailbreak {}", "hello ".repeat(20), "hello ".repeat(20));
        assert!(model.score(&middle, later()).unwrap()["prompt_injection"] < 0.05);
    }

    #[test]
    fn a_busy_session_past_the_deadline_is_the_overload_case() {
        let Some(model) = fixture() else { return };
        let _held = model.session.lock();
        assert!(matches!(
            model.score("jailbreak", Instant::now()),
            Err(ScoreError::Busy)
        ));
    }

    #[test]
    fn model_root_confines_the_files_a_manifest_names() {
        // the manifest is under the root, and names a model outside it
        let dir = crate::local_classifier::tests::TempDir::new("onnx-root");
        let root = dir.0.join("models");
        std::fs::create_dir_all(&root).unwrap();
        let fixtures = Path::new(MANIFEST).parent().unwrap();
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(MANIFEST).unwrap()).unwrap();
        manifest["model"]["path"] = serde_json::json!(fixtures.join("model.onnx"));
        manifest["tokenizer"]["path"] = serde_json::json!(fixtures.join("tokenizer.json"));
        let error = OnnxModel::from_manifest(
            &root.join("manifest.json"),
            manifest.to_string().as_bytes(),
            Some(&root),
        )
        .unwrap_err();
        assert!(error.to_string().contains("outside model_root"), "{error}");
    }
}
