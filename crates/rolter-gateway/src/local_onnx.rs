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
//! Longer text is scored in overlapping token windows, with the tokenizer's
//! special tokens reapplied to every window and each category's maximum retained.
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
use std::sync::{Arc, OnceLock, Weak};
use std::time::{Duration, Instant};

use ort::environment::{Environment, EnvironmentBuilder};
use ort::session::{Session, SessionInputValue};
use ort::value::Tensor;
use rolter_core::guardrail_webhook::{OnTruncated, MAX_SCORED_WINDOWS};
use rolter_core::semantic_guards::MAX_CATEGORY_LEN;
use serde::Deserialize;
use tokenizers::decoders::DecoderWrapper;
use tokenizers::models::ModelWrapper;
use tokenizers::normalizers::NormalizerWrapper;
use tokenizers::pattern::Invert;
use tokenizers::pre_tokenizers::split::SplitPattern;
use tokenizers::pre_tokenizers::PreTokenizerWrapper;
use tokenizers::processors::PostProcessorWrapper;
use tokenizers::{
    NormalizedString, Normalizer, PostProcessor, PreTokenizedString, PreTokenizer, TokenizerImpl,
    TruncationParams,
};
use tokio::sync::Semaphore;

use crate::local_classifier::{
    read_pinned, ModelError, ModelScores, RuntimeOwner, ScoreError, MAX_CATEGORIES,
    MAX_MODEL_FILE_BYTES, ONNX_FORMAT,
};

type ErrorPropagatingTokenizer = TokenizerImpl<
    ModelWrapper,
    ErrorPropagatingNormalizer,
    ErrorPropagatingPreTokenizer,
    PostProcessorWrapper,
    DecoderWrapper,
>;

/// The largest ONNX model file loaded.
pub const MAX_ONNX_MODEL_BYTES: u64 = 1024 * 1024 * 1024;
/// The largest tokenizer file loaded.
pub const MAX_TOKENIZER_BYTES: u64 = MAX_MODEL_FILE_BYTES;
/// The most tokens one inference window may feed a model.
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
    /// The output to read; the model's first when unset.
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
    // keep the environment alive until this model's session has dropped
    _environment: Arc<Environment>,
    pub(crate) admission: Arc<Semaphore>,
    tokenizer: ErrorPropagatingTokenizer,
    labels: Vec<Option<String>>,
    activation: Activation,
    max_tokens: usize,
    /// Whether the model declares each optional input.
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

/// A shared runtime while gateway readers or models own it, never a static owner.
static RUNTIME: OnceLock<parking_lot::Mutex<Weak<Environment>>> = OnceLock::new();

/// Reuse a live environment, retrying a failed load on the next call.
fn runtime(owner: Option<Weak<RuntimeOwner>>) -> Result<Arc<Environment>, ModelError> {
    let Some(owner) = owner else {
        return shared_runtime();
    };
    let owner = owner
        .upgrade()
        .ok_or_else(|| ModelError::Runtime("the gateway model loader has stopped".into()))?;
    let mut owned = owner.environment.lock();
    if let Some(environment) = &*owned {
        return Ok(environment.clone());
    }
    let environment = shared_runtime()?;
    *owned = Some(environment.clone());
    Ok(environment)
}

fn shared_runtime() -> Result<Arc<Environment>, ModelError> {
    let mut cached = RUNTIME
        .get_or_init(|| parking_lot::Mutex::new(Weak::new()))
        .lock();
    if let Some(environment) = cached.upgrade() {
        return Ok(environment);
    }
    let loaded = std::panic::catch_unwind(|| {
        library()?
            .build()
            .map_err(|error| ModelError::Runtime(error.to_string()))
    });
    let environment = Arc::new(match loaded {
        Ok(environment) => environment?,
        Err(_) => {
            return Err(ModelError::Runtime(
                "initializing the environment panicked".into(),
            ))
        }
    });
    *cached = Arc::downgrade(&environment);
    tracing::info!("onnx runtime environment created for local guardrail models");
    Ok(environment)
}

/// Check the shared library without consuming ort's one environment creation.
fn library() -> Result<EnvironmentBuilder, ModelError> {
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
    // a library that is not ONNX Runtime can make ort panic rather than return an error
    let loaded = std::panic::catch_unwind(|| {
        ort::init_from(&path)
            .map(|builder| builder.with_name("rolter"))
            .map_err(|error| error.to_string())
    });
    match loaded {
        Ok(builder) => builder,
        Err(_) => Err("loading it panicked".to_string()),
    }
    .map_err(|error| {
        ModelError::Runtime(format!(
            "cannot load the ONNX Runtime library {} ({error}); set ORT_DYLIB_PATH to \
             its libonnxruntime",
            path.display()
        ))
    })
}

fn malformed(why: impl std::fmt::Display) -> ModelError {
    ModelError::Malformed(format!("the {ONNX_FORMAT} manifest: {why}"))
}

fn is_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

fn validate_tokenizer_patterns(value: &serde_json::Value) -> Result<(), ModelError> {
    match value {
        serde_json::Value::Object(fields) => {
            // serialized Split/Replace regexes use this one-key enum representation
            if fields.len() == 1 {
                if let Some(serde_json::Value::String(pattern)) = fields.get("Regex") {
                    fancy_regex::Regex::new(pattern)
                        .map_err(|error| malformed(format_args!("the tokenizer: {error}")))?;
                }
            }
            for child in fields.values() {
                validate_tokenizer_patterns(child)?;
            }
        }
        serde_json::Value::Array(values) => {
            for child in values {
                validate_tokenizer_patterns(child)?;
            }
        }
        _ => {}
    }
    Ok(())
}

const TOKENIZER_BACKTRACK_LIMIT: usize = 1_000_000;
const TOKENIZER_DELEGATE_SIZE_LIMIT: usize = 1_000_000;

fn compile_fancy(pattern: &str, literal: bool) -> Result<fancy_regex::Regex, ModelError> {
    let pattern = if literal {
        Cow::Owned(regex::escape(pattern))
    } else {
        Cow::Borrowed(pattern)
    };
    let mut builder = fancy_regex::RegexBuilder::new(&pattern);
    builder
        .backtrack_limit(TOKENIZER_BACKTRACK_LIMIT)
        .delegate_size_limit(TOKENIZER_DELEGATE_SIZE_LIMIT);
    builder
        .build()
        .map_err(|error| malformed(format_args!("the tokenizer: {error}")))
}

struct ErrorPropagatingPreTokenizer {
    original: PreTokenizerWrapper,
    regex: Option<fancy_regex::Regex>,
    sequence: Vec<Self>,
}

impl ErrorPropagatingPreTokenizer {
    fn new(original: &PreTokenizerWrapper) -> Result<Self, ModelError> {
        let regex = match original {
            PreTokenizerWrapper::Split(split) => Some(match &split.pattern {
                SplitPattern::String(pattern) => compile_fancy(pattern, true)?,
                SplitPattern::Regex(pattern) => compile_fancy(pattern, false)?,
            }),
            _ => None,
        };
        let sequence = match original {
            PreTokenizerWrapper::Sequence(sequence) => sequence
                .as_ref()
                .iter()
                .map(Self::new)
                .collect::<Result<Vec<_>, _>>()?,
            _ => Vec::new(),
        };
        Ok(Self {
            original: original.clone(),
            regex,
            sequence,
        })
    }
}

impl PreTokenizer for ErrorPropagatingPreTokenizer {
    fn pre_tokenize(&self, pretokenized: &mut PreTokenizedString) -> tokenizers::Result<()> {
        match &self.original {
            PreTokenizerWrapper::Sequence(_) => {
                for tokenizer in &self.sequence {
                    tokenizer.pre_tokenize(pretokenized)?;
                }
                Ok(())
            }
            PreTokenizerWrapper::Split(split) => {
                let regex = self.regex.as_ref().ok_or("a split has no compiled regex")?;
                if split.invert {
                    pretokenized
                        .split(|_, normalized| normalized.split(Invert(regex), split.behavior))
                } else {
                    pretokenized.split(|_, normalized| normalized.split(regex, split.behavior))
                }
            }
            original => original.pre_tokenize(pretokenized),
        }
    }
}

struct ErrorPropagatingNormalizer {
    original: NormalizerWrapper,
    regex: Option<fancy_regex::Regex>,
    sequence: Vec<Self>,
    error: Arc<parking_lot::Mutex<Option<String>>>,
}

impl ErrorPropagatingNormalizer {
    fn new(original: &NormalizerWrapper) -> Result<Self, ModelError> {
        Self::new_shared(original, Arc::default())
    }

    fn new_shared(
        original: &NormalizerWrapper,
        error: Arc<parking_lot::Mutex<Option<String>>>,
    ) -> Result<Self, ModelError> {
        let regex = match original {
            NormalizerWrapper::Replace(replace) => {
                let value = serde_json::to_value(replace)
                    .map_err(|error| malformed(format_args!("the tokenizer: {error}")))?;
                let fields = value["pattern"].as_object().ok_or_else(|| {
                    malformed("the tokenizer has a Replace without a valid pattern")
                })?;
                if let Some(serde_json::Value::String(pattern)) = fields.get("Regex") {
                    Some(compile_fancy(pattern, false)?)
                } else if let Some(serde_json::Value::String(pattern)) = fields.get("String") {
                    Some(compile_fancy(pattern, true)?)
                } else {
                    return Err(malformed("the tokenizer has a Replace without a pattern"));
                }
            }
            _ => None,
        };
        let sequence = match original {
            NormalizerWrapper::Sequence(sequence) => sequence
                .as_ref()
                .iter()
                .map(|child| Self::new_shared(child, Arc::clone(&error)))
                .collect::<Result<Vec<_>, _>>()?,
            _ => Vec::new(),
        };
        Ok(Self {
            original: original.clone(),
            regex,
            sequence,
            error,
        })
    }

    fn clear_error(&self) {
        *self.error.lock() = None;
    }

    fn take_error(&self) -> Option<String> {
        self.error.lock().take()
    }
}

impl Normalizer for ErrorPropagatingNormalizer {
    fn normalize(&self, normalized: &mut NormalizedString) -> tokenizers::Result<()> {
        let result = match &self.original {
            NormalizerWrapper::Sequence(_) => {
                for normalizer in &self.sequence {
                    normalizer.normalize(normalized)?;
                }
                Ok(())
            }
            NormalizerWrapper::Replace(replace) => normalized.replace(
                self.regex
                    .as_ref()
                    .ok_or("a replace has no compiled regex")?,
                &replace.content,
            ),
            original => original.normalize(normalized),
        };
        if let Err(error) = &result {
            *self.error.lock() = Some(error.to_string());
        }
        result
    }
}

impl<'de> Deserialize<'de> for ErrorPropagatingPreTokenizer {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let original = PreTokenizerWrapper::deserialize(deserializer)?;
        Self::new(&original).map_err(serde::de::Error::custom)
    }
}

impl<'de> Deserialize<'de> for ErrorPropagatingNormalizer {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let original = NormalizerWrapper::deserialize(deserializer)?;
        Self::new(&original).map_err(serde::de::Error::custom)
    }
}

impl OnnxModel {
    /// Parse the manifest at `manifest` (its pinned `bytes`), then read,
    /// check and load the model and tokenizer it names.
    #[cfg(test)]
    pub fn from_manifest(
        manifest: &Path,
        bytes: &[u8],
        root: Option<&Path>,
    ) -> Result<Self, ModelError> {
        Self::from_manifest_owned(manifest, bytes, root, None)
    }

    pub(crate) fn from_manifest_owned(
        manifest: &Path,
        bytes: &[u8],
        root: Option<&Path>,
        runtime_owner: Option<Weak<RuntimeOwner>>,
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
        // fail before hashing large parts, without consuming the one environment creation
        drop(library()?);
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
        // fancy-regex's own Pattern implementation propagates resource-limit errors
        validate_tokenizer_patterns(
            &serde_json::from_slice(&tokenizer)
                .map_err(|error| malformed(format_args!("the tokenizer: {error}")))?,
        )?;
        let mut tokenizer = ErrorPropagatingTokenizer::from_bytes(&tokenizer)
            .map_err(|error| malformed(format_args!("the tokenizer: {error}")))?;
        let added = tokenizer
            .get_post_processor()
            .map_or(0, |processor| processor.added_tokens(false));
        if file.max_tokens <= added {
            return Err(malformed("max_tokens must leave room after special tokens"));
        }
        let content_tokens = file.max_tokens - added;
        // overflow encodings preserve context overlap and reapply special tokens
        tokenizer
            .with_truncation(Some(TruncationParams {
                max_length: file.max_tokens,
                stride: (content_tokens / 4).max(1).min(content_tokens - 1),
                ..Default::default()
            }))
            .map_err(|error| malformed(format_args!("the tokenizer: {error}")))?;
        tokenizer.with_padding(None);
        let (_, model) = read_pinned(
            &directory.join(&file.model.path),
            &file.model.sha256,
            MAX_ONNX_MODEL_BYTES,
            root,
        )
        .map_err(part("the onnx model"))?;

        let environment = runtime(runtime_owner)?;
        let runtime_error = |error: ort::Error| match error.code() {
            ort::ErrorCode::InvalidArgument
            | ort::ErrorCode::InvalidProtobuf
            | ort::ErrorCode::InvalidGraph
            | ort::ErrorCode::NotImplemented
            | ort::ErrorCode::NoModel => malformed(format_args!("the onnx model: {error}")),
            _ => ModelError::Runtime(error.to_string()),
        };
        // one thread per session: the gateway's local_concurrency semaphore,
        // not ONNX Runtime's own pool, bounds the CPU local models take
        let session = Session::builder(&environment)
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
        let model = Self {
            session: parking_lot::Mutex::new(session),
            _environment: environment,
            admission: Arc::new(Semaphore::new(1)),
            tokenizer,
            labels: file.labels,
            activation: file.activation,
            max_tokens: file.max_tokens,
            attention_mask,
            token_type_ids,
            output,
        };
        // validate the actual output and empty-input path before publishing loaded
        model
            .score(
                "",
                Instant::now() + Duration::from_secs(5),
                OnTruncated::FailClosed,
            )
            .categories
            .map_err(|error| malformed(format_args!("empty-input probe failed: {error:?}")))?;
        Ok(model)
    }

    /// How many categories the model reports.
    pub fn categories(&self) -> usize {
        self.labels.iter().flatten().count()
    }

    /// Hold the native inference resource for a deterministic overload test.
    #[cfg(test)]
    pub(crate) fn occupy_session(&self) -> parking_lot::MutexGuard<'_, Session> {
        self.session.lock()
    }

    /// Score `text`, waiting for the session until `deadline` at most.
    pub fn score(&self, text: &str, deadline: Instant, on_truncated: OnTruncated) -> ModelScores {
        let failed = |error: &dyn std::fmt::Display| ScoreError::Failed(error.to_string());
        if let Some(normalizer) = self.tokenizer.get_normalizer() {
            normalizer.clear_error();
        }
        // a tokenizer panic is a provider failure, never a benign classification
        let encoded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.tokenizer.encode(text, true)
        }));
        let encoding = match encoded {
            Ok(Ok(encoding)) => encoding,
            Ok(Err(error)) => {
                return ModelScores {
                    categories: Err(failed(&error)),
                    truncated: false,
                };
            }
            Err(_) => {
                return ModelScores {
                    categories: Err(ScoreError::Failed("the tokenizer panicked".into())),
                    truncated: false,
                };
            }
        };
        // added_vocabulary drops the normalizer's Result, so the wrapper retains it
        if let Some(error) = self
            .tokenizer
            .get_normalizer()
            .and_then(ErrorPropagatingNormalizer::take_error)
        {
            return ModelScores {
                categories: Err(ScoreError::Failed(error)),
                truncated: false,
            };
        }
        let truncated = encoding.get_overflowing().len() >= MAX_SCORED_WINDOWS;
        let categories = if truncated && on_truncated != OnTruncated::Score {
            Err(ScoreError::Truncated)
        } else {
            self.score_encodings(&encoding, deadline)
        };
        ModelScores {
            categories,
            truncated,
        }
    }

    fn score_encodings(
        &self,
        encoding: &tokenizers::Encoding,
        deadline: Instant,
    ) -> Result<BTreeMap<String, f64>, ScoreError> {
        let failed = |error: &dyn std::fmt::Display| ScoreError::Failed(error.to_string());
        let Some(mut session) = self.session.try_lock_until(deadline) else {
            return Err(ScoreError::Busy);
        };
        let to_i64 =
            |values: &[u32]| -> Vec<i64> { values.iter().copied().map(i64::from).collect() };
        let mut best = BTreeMap::new();
        let count = encoding.get_overflowing().len() + 1;
        let head = MAX_SCORED_WINDOWS / 4;
        let tail = MAX_SCORED_WINDOWS - head;
        for (index, window) in std::iter::once(encoding)
            .chain(encoding.get_overflowing())
            .enumerate()
        {
            // a cut retains both ends, matching the byte budget's selection
            if count > MAX_SCORED_WINDOWS && index >= head && index < count - tail {
                continue;
            }
            if Instant::now() >= deadline {
                return Err(ScoreError::Timeout);
            }
            let ids = to_i64(window.get_ids());
            let shape = [1usize, ids.len()];
            let mut inputs: Vec<(Cow<'_, str>, SessionInputValue<'_>)> = vec![(
                INPUT_IDS.into(),
                Tensor::from_array((shape, ids))
                    .map_err(|error| failed(&error))?
                    .into(),
            )];
            if self.attention_mask {
                inputs.push((
                    ATTENTION_MASK.into(),
                    Tensor::from_array((shape, to_i64(window.get_attention_mask())))
                        .map_err(|error| failed(&error))?
                        .into(),
                ));
            }
            if self.token_type_ids {
                inputs.push((
                    TOKEN_TYPE_IDS.into(),
                    Tensor::from_array((shape, to_i64(window.get_type_ids())))
                        .map_err(|error| failed(&error))?
                        .into(),
                ));
            }
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
            for (label, score) in self.labels.iter().zip(activate(logits, self.activation)) {
                if let Some(label) = label {
                    let entry = best.entry(label.clone()).or_insert(score);
                    *entry = entry.max(score);
                }
            }
        }
        Ok(best)
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

    fn provider_for_manifest(
        folder: &Path,
        manifest: &serde_json::Value,
    ) -> rolter_core::GuardrailWebhookConfig {
        use sha2::{Digest, Sha256};
        let bytes = manifest.to_string();
        std::fs::write(folder.join("manifest.json"), &bytes).unwrap();
        let digest: String = Sha256::digest(bytes.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let mut provider = crate::local_classifier::tests::local_provider(&digest, 5_000);
        provider.model_path = folder.join("manifest.json").to_string_lossy().into_owned();
        provider
    }

    fn configured(
        provider: &rolter_core::GuardrailWebhookConfig,
    ) -> crate::local_classifier::LocalClassifiers {
        let classifiers = crate::local_classifier::LocalClassifiers::default();
        classifiers.configure(
            &rolter_core::SemanticGuardsConfig {
                providers: vec![provider.clone()],
                ..Default::default()
            },
            &rolter_core::GuardrailWebhookConfig::default(),
        );
        classifiers
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
        let Some(_model) = fixture() else { return };
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
        let attack = model
            .score("please jailbreak", later(), OnTruncated::Score)
            .categories
            .unwrap();
        assert!(attack["prompt_injection"] > 0.95, "{attack:?}");
        assert_eq!(attack.len(), 1, "a null label is not reported");
        let benign = model
            .score(
                "What is the capital of France?",
                later(),
                OnTruncated::Score,
            )
            .categories
            .unwrap();
        assert!(benign["prompt_injection"] < 0.05, "{benign:?}");
    }

    #[test]
    fn a_long_text_is_scored_on_its_middle_and_last_tokens() {
        let Some(model) = fixture() else { return };
        // 16 tokens per window: later tokens still reach inference
        let late = format!("{} jailbreak", "hello ".repeat(40));
        assert!(
            model
                .score(&late, later(), OnTruncated::Score)
                .categories
                .unwrap()["prompt_injection"]
                > 0.95
        );
        // middle tokens are covered rather than silently discarded
        let middle = format!("{} jailbreak {}", "hello ".repeat(20), "hello ".repeat(20));
        assert!(
            model
                .score(&middle, later(), OnTruncated::Score)
                .categories
                .unwrap()["prompt_injection"]
                > 0.95
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_late_runtime_mount_recovers_without_restart_or_reading_parts_first() {
        const CHILD: &str = "ROLTER_TEST_ORT_MOUNT_SOURCE";
        if let Some(source) = std::env::var_os(CHILD) {
            let mut manifest: serde_json::Value =
                serde_json::from_slice(&std::fs::read(MANIFEST).unwrap()).unwrap();
            manifest["tokenizer"]["path"] = serde_json::json!("missing-tokenizer.json");
            let bytes = serde_json::to_vec(&manifest).unwrap();
            let before = OnnxModel::from_manifest(Path::new(MANIFEST), &bytes, None).unwrap_err();
            assert!(matches!(&before, ModelError::Runtime(_)), "{before}");
            assert!(before.is_transient());
            let destination = std::env::var_os("ORT_DYLIB_PATH").unwrap();
            std::os::unix::fs::symlink(source, destination).unwrap();
            let missing = OnnxModel::from_manifest(Path::new(MANIFEST), &bytes, None).unwrap_err();
            assert!(matches!(&missing, ModelError::Part { .. }), "{missing}");
            assert!(
                missing.is_transient(),
                "a missing tokenizer can arrive later"
            );
            let model = fixture().expect("the library mounted at the same path must recover");
            let scores = model
                .score("jailbreak", later(), OnTruncated::Score)
                .categories
                .unwrap();
            assert!(scores["prompt_injection"] > 0.95);
            return;
        }
        let Some(source) = std::env::var_os("ORT_DYLIB_PATH") else {
            assert!(
                std::env::var_os("ROLTER_TEST_REQUIRE_ORT").is_none(),
                "the required-runtime mount test needs an explicit ORT_DYLIB_PATH"
            );
            eprintln!("skipped: the late-mount test needs ORT_DYLIB_PATH");
            return;
        };
        let Some(_model) = fixture() else { return };
        let source = std::fs::canonicalize(source).unwrap();
        let folder = crate::local_classifier::tests::TempDir::new("runtime-mount");
        let destination = folder.0.join("late-runtime-library");
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "local_onnx::tests::a_late_runtime_mount_recovers_without_restart_or_reading_parts_first",
                "--nocapture",
            ])
            .env(CHILD, source)
            .env("ORT_DYLIB_PATH", destination)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "mount recovery subprocess failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[tokio::test]
    async fn a_native_model_can_be_removed_and_readded_after_empty_reloads() {
        if let Err(error) = library() {
            assert!(
                std::env::var_os("ROLTER_TEST_REQUIRE_ORT").is_none(),
                "{error}"
            );
            eprintln!("skipped: {error}");
            return;
        }
        let mut provider =
            crate::local_classifier::tests::local_provider(&manifest_sha256(), 2_000);
        provider.model_path = MANIFEST.to_string();
        let configured = rolter_core::SemanticGuardsConfig {
            providers: vec![provider.clone()],
            ..Default::default()
        };
        let classifiers = crate::local_classifier::LocalClassifiers::default();
        classifiers.configure(&configured, &Default::default());
        let content = serde_json::json!("jailbreak");
        assert!(
            classifiers
                .classify(&provider, &content)
                .await
                .unwrap()
                .categories["prompt_injection"]
                > 0.95
        );
        // the previous snapshot deliberately retains a dropped model for one apply
        classifiers.configure(&Default::default(), &Default::default());
        classifiers.configure(&Default::default(), &Default::default());
        classifiers.configure(&Default::default(), &Default::default());
        classifiers.configure(&configured, &Default::default());
        assert!(
            classifiers
                .classify(&provider, &content)
                .await
                .unwrap()
                .categories["prompt_injection"]
                > 0.95
        );
    }

    #[test]
    fn invalid_onnx_bytes_do_not_retry_as_if_the_library_were_missing() {
        use sha2::{Digest, Sha256};
        let Some(_model) = fixture() else { return };
        let folder = crate::local_classifier::tests::TempDir::new("invalid-onnx");
        let path = folder.0.join("invalid.onnx");
        let bytes = b"not an ONNX protobuf";
        std::fs::write(&path, bytes).unwrap();
        let digest: String = Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(MANIFEST).unwrap()).unwrap();
        manifest["model"]["path"] = serde_json::json!(path);
        manifest["model"]["sha256"] = serde_json::json!(digest);
        let error =
            OnnxModel::from_manifest(Path::new(MANIFEST), manifest.to_string().as_bytes(), None)
                .unwrap_err();
        assert!(
            !error.is_transient(),
            "a pinned invalid protobuf cannot recover: {error}"
        );
        assert!(matches!(error, ModelError::Malformed(_)));
    }

    #[test]
    fn an_output_width_mismatch_is_refused_at_load_not_on_every_request() {
        let Some(_model) = fixture() else { return };
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(MANIFEST).unwrap()).unwrap();
        manifest["labels"] = serde_json::json!(["prompt_injection"]);
        let error =
            OnnxModel::from_manifest(Path::new(MANIFEST), manifest.to_string().as_bytes(), None)
                .unwrap_err();
        assert!(
            error.to_string().contains("2 values for 1 labels"),
            "{error}"
        );
        assert!(matches!(error, ModelError::Malformed(_)));
    }

    #[test]
    fn an_output_dtype_mismatch_is_refused_at_load_not_on_every_request() {
        use sha2::{Digest, Sha256};
        let Some(_model) = fixture() else { return };
        let path = Path::new(MANIFEST).with_file_name("model-f64.onnx");
        let digest: String = Sha256::digest(std::fs::read(&path).unwrap())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(MANIFEST).unwrap()).unwrap();
        manifest["model"]["path"] = serde_json::json!(path);
        manifest["model"]["sha256"] = serde_json::json!(digest);
        let error =
            OnnxModel::from_manifest(Path::new(MANIFEST), manifest.to_string().as_bytes(), None)
                .unwrap_err();
        assert!(
            error.to_string().contains("empty-input probe failed"),
            "{error}"
        );
        assert!(error.to_string().contains("Tensor<f64>"), "{error}");
        assert!(matches!(error, ModelError::Malformed(_)));
    }

    #[tokio::test]
    async fn a_zero_id_empty_sequence_is_probed_and_contentless_calls_succeed_repeatedly() {
        use sha2::{Digest, Sha256};
        let Some(_model) = fixture() else { return };
        let folder = crate::local_classifier::tests::TempDir::new("zero-token-onnx");
        let path = Path::new(MANIFEST).with_file_name("model-empty.onnx");
        let fixtures = Path::new(MANIFEST).parent().unwrap();
        let mut tokenizer: serde_json::Value =
            serde_json::from_slice(&std::fs::read(fixtures.join("tokenizer.json")).unwrap())
                .unwrap();
        tokenizer["post_processor"] = serde_json::Value::Null;
        let tokenizer_bytes = serde_json::to_vec(&tokenizer).unwrap();
        std::fs::write(folder.0.join("tokenizer.json"), &tokenizer_bytes).unwrap();
        let hash = |bytes: &[u8]| -> String {
            Sha256::digest(bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect()
        };
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(MANIFEST).unwrap()).unwrap();
        manifest["model"]["path"] = serde_json::json!(path);
        manifest["model"]["sha256"] = serde_json::json!(hash(&std::fs::read(path).unwrap()));
        manifest["tokenizer"]["sha256"] = serde_json::json!(hash(&tokenizer_bytes));
        let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
        let manifest_path = folder.0.join("manifest.json");
        std::fs::write(&manifest_path, &manifest_bytes).unwrap();
        let model = OnnxModel::from_manifest(&manifest_path, &manifest_bytes, None).unwrap();
        assert!(model.tokenizer.encode("", true).unwrap().is_empty());
        let mut provider =
            crate::local_classifier::tests::local_provider(&hash(&manifest_bytes), 2_000);
        provider.model_path = manifest_path.to_str().unwrap().to_owned();
        let classifiers = crate::local_classifier::LocalClassifiers::default();
        classifiers.configure(
            &rolter_core::SemanticGuardsConfig {
                providers: vec![provider.clone()],
                ..Default::default()
            },
            &Default::default(),
        );
        for content in [
            serde_json::Value::Null,
            serde_json::json!(""),
            serde_json::Value::Null,
        ] {
            let scores = classifiers.classify(&provider, &content).await.unwrap();
            assert!(scores.categories["prompt_injection"] < 0.05, "{scores:?}");
        }
    }

    #[test]
    fn the_real_roberta_tokenizer_matches_onig_vectors_on_adversarial_inputs() {
        use sha2::{Digest, Sha256};
        use std::io::Read as _;
        let fixtures = Path::new(MANIFEST).parent().unwrap();
        let mut source = Vec::new();
        flate2::read::GzDecoder::new(
            std::fs::File::open(fixtures.join("roberta-tokenizer.json.gz")).unwrap(),
        )
        .read_to_end(&mut source)
        .unwrap();
        let digest: String = Sha256::digest(&source)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert_eq!(
            digest,
            "847bbeab6174d66a88898f729d52fa8d355fafe1bea101cf960dd404581df70e"
        );
        validate_tokenizer_patterns(&serde_json::from_slice(&source).unwrap()).unwrap();
        let tokenizer = ErrorPropagatingTokenizer::from_bytes(&source).unwrap();
        let vectors: serde_json::Value = serde_json::from_slice(
            &std::fs::read(fixtures.join("tokenizer-vectors.json")).unwrap(),
        )
        .unwrap();
        for case in vectors["cases"].as_array().unwrap() {
            let text = format!(
                "{}{}{}",
                case["prefix"].as_str().unwrap(),
                case["unit"]
                    .as_str()
                    .unwrap()
                    .repeat(case["repeat"].as_u64().unwrap() as usize),
                case["suffix"].as_str().unwrap(),
            );
            let encoded = tokenizer.encode(text, true).unwrap();
            assert_eq!(
                encoded.len(),
                case["tokens"].as_u64().unwrap() as usize,
                "{}",
                case["name"]
            );
            let bytes: Vec<u8> = encoded
                .get_ids()
                .iter()
                .flat_map(|id| id.to_le_bytes())
                .collect();
            let actual: String = Sha256::digest(bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            assert_eq!(
                actual,
                case["ids_sha256_le_u32"].as_str().unwrap(),
                "{}",
                case["name"]
            );
        }
    }

    #[tokio::test]
    async fn a_split_regex_error_does_not_hide_a_late_injection() {
        use sha2::{Digest, Sha256};
        let Some(_model) = fixture() else { return };
        let folder = crate::local_classifier::tests::TempDir::new("regex-error-onnx");
        let fixtures = Path::new(MANIFEST).parent().unwrap();
        let mut tokenizer: serde_json::Value =
            serde_json::from_slice(&std::fs::read(fixtures.join("tokenizer.json")).unwrap())
                .unwrap();
        tokenizer["pre_tokenizer"] = serde_json::json!({
            "type": "Sequence",
            "pretokenizers": [{
                "type": "Split",
                "pattern": { "Regex": r"(?:(a+)+(?=z))|\s+" },
                "behavior": "Removed",
                "invert": true,
            }],
        });
        let tokenizer_bytes = serde_json::to_vec(&tokenizer).unwrap();
        std::fs::write(folder.0.join("tokenizer.json"), &tokenizer_bytes).unwrap();
        let digest: String = Sha256::digest(&tokenizer_bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(MANIFEST).unwrap()).unwrap();
        manifest["model"]["path"] = serde_json::json!(fixtures.join("model.onnx"));
        manifest["tokenizer"]["sha256"] = serde_json::json!(digest);
        let provider = provider_for_manifest(&folder.0, &manifest);
        let classifiers = configured(&provider);
        let adversarial = format!("hello {}! jailbreak", "a".repeat(64));
        for _ in 0..2 {
            let error = classifiers
                .classify(
                    &provider,
                    &serde_json::json!({"messages": [{"role": "user", "content": adversarial}]}),
                )
                .await
                .unwrap_err();
            assert!(matches!(
                error,
                crate::guardrail_webhook::CallFailure::Model
            ));
        }
        // a supported linear Split still preserves the late needle on repeated calls
        tokenizer["pre_tokenizer"]["pretokenizers"][0]["pattern"] =
            serde_json::json!({"Regex": r"\s+"});
        tokenizer["pre_tokenizer"]["pretokenizers"][0]["invert"] = serde_json::json!(false);
        let bytes = serde_json::to_vec(&tokenizer).unwrap();
        std::fs::write(folder.0.join("tokenizer.json"), &bytes).unwrap();
        let digest: String = Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        manifest["tokenizer"]["sha256"] = serde_json::json!(digest);
        let model = OnnxModel::from_manifest(
            &folder.0.join("manifest.json"),
            manifest.to_string().as_bytes(),
            None,
        )
        .unwrap();
        for _ in 0..2 {
            assert!(
                model
                    .score(&adversarial, later(), OnTruncated::Score)
                    .categories
                    .unwrap()["prompt_injection"]
                    > 0.95
            );
        }
    }

    #[test]
    fn tokenizer_regex_validation_covers_nested_normalizers_without_confusing_vocabulary() {
        for _ in 0..2 {
            assert!(validate_tokenizer_patterns(&serde_json::Value::Null).is_ok());
            assert!(validate_tokenizer_patterns(&serde_json::json!({})).is_ok());
        }
        let normalizer = serde_json::json!({
            "normalizer": {"type": "Sequence", "normalizers": [
                {"type": "Replace", "pattern": {"Regex": r"(a+)\1"}, "content": ""}
            ]}
        });
        assert!(validate_tokenizer_patterns(&normalizer).is_ok());
        assert!(validate_tokenizer_patterns(&serde_json::json!({
            "model": {"vocab": {"Regex": 10, "hello": 13}},
            "pre_tokenizer": {"pattern": {"Regex": r"\s+"}},
        }))
        .is_ok());
    }

    #[test]
    fn a_replace_regex_error_is_returned_on_every_call() {
        let original: NormalizerWrapper = serde_json::from_value(serde_json::json!({
            "type": "Replace",
            "pattern": {"Regex": r"(?:(a+)+(?=z))|\s+"},
            "content": "",
        }))
        .unwrap();
        let normalizer = ErrorPropagatingNormalizer::new(&original).unwrap();
        for _ in 0..2 {
            let mut normalized = NormalizedString::from("a".repeat(64));
            let error = normalizer.normalize(&mut normalized).unwrap_err();
            assert!(error.to_string().contains("backtrack"), "{error}");
        }
        let backreference: NormalizerWrapper = serde_json::from_value(serde_json::json!({
            "type": "Replace",
            "pattern": {"Regex": r"(a+)\1"},
            "content": "",
        }))
        .unwrap();
        let normalizer = ErrorPropagatingNormalizer::new(&backreference).unwrap();
        let mut normalized = NormalizedString::from("aa");
        normalizer.normalize(&mut normalized).unwrap();
        assert_eq!(normalized.get(), "");
    }

    #[tokio::test]
    async fn a_nested_replace_regex_error_reaches_the_classifier() {
        use sha2::{Digest, Sha256};
        let Some(_model) = fixture() else { return };
        let folder = crate::local_classifier::tests::TempDir::new("nested-regex-error-onnx");
        let fixtures = Path::new(MANIFEST).parent().unwrap();
        let mut tokenizer: serde_json::Value =
            serde_json::from_slice(&std::fs::read(fixtures.join("tokenizer.json")).unwrap())
                .unwrap();
        tokenizer["normalizer"] = serde_json::json!({
            "type": "Sequence",
            "normalizers": [
                {"type": "Replace", "pattern": {"Regex": r"(o)\1"}, "content": ""},
                {"type": "Replace", "pattern": {"Regex": r"(?:(a+)+(?=z))"}, "content": ""},
            ],
        });
        let tokenizer_bytes = serde_json::to_vec(&tokenizer).unwrap();
        std::fs::write(folder.0.join("tokenizer.json"), &tokenizer_bytes).unwrap();
        let digest: String = Sha256::digest(&tokenizer_bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(MANIFEST).unwrap()).unwrap();
        manifest["model"]["path"] = serde_json::json!(fixtures.join("model.onnx"));
        manifest["tokenizer"]["sha256"] = serde_json::json!(digest);
        let adversarial = format!("hello {}! jailbreak", "a".repeat(64));
        let original: NormalizerWrapper =
            serde_json::from_value(tokenizer["normalizer"].clone()).unwrap();
        let normalizer = ErrorPropagatingNormalizer::new(&original).unwrap();
        let mut normalized = NormalizedString::from(adversarial.clone());
        let error = normalizer.normalize(&mut normalized).unwrap_err();
        assert!(error.to_string().contains("backtrack"), "{error}");
        let tokenizer = ErrorPropagatingTokenizer::from_bytes(&tokenizer_bytes).unwrap();
        tokenizer.encode(adversarial.clone(), true).unwrap();
        let error = tokenizer
            .get_normalizer()
            .and_then(ErrorPropagatingNormalizer::take_error)
            .expect("the normalizer error is retained after added-vocabulary extraction");
        assert!(error.contains("backtrack"), "{error}");
        let provider = provider_for_manifest(&folder.0, &manifest);
        let classifiers = configured(&provider);
        for _ in 0..2 {
            let error = classifiers
                .classify(
                    &provider,
                    &serde_json::json!({"messages": [{"role": "user", "content": adversarial}]}),
                )
                .await
                .unwrap_err();
            assert!(matches!(
                error,
                crate::guardrail_webhook::CallFailure::Model
            ));
        }
    }

    #[test]
    fn a_busy_session_past_the_deadline_is_the_overload_case() {
        let Some(model) = fixture() else { return };
        let _held = model.session.lock();
        assert!(matches!(
            model
                .score("jailbreak", Instant::now(), OnTruncated::Score)
                .categories,
            Err(ScoreError::Busy)
        ));
    }

    /// A byte window that fits the provider budget must not lose middle tokens.
    #[tokio::test]
    async fn an_onnx_provider_reads_the_middle_of_a_byte_window() {
        let Some(_model) = fixture() else { return };
        let mut provider =
            crate::local_classifier::tests::local_provider(&manifest_sha256(), 2_000);
        provider.model_path = MANIFEST.to_string();
        provider.max_body_bytes = 1_024;
        provider.max_scored_bytes = Some(1_024);
        provider.on_truncated = rolter_core::guardrail_webhook::OnTruncated::FailClosed;
        let classifiers = crate::local_classifier::LocalClassifiers::default();
        classifiers.configure(
            &rolter_core::SemanticGuardsConfig {
                providers: vec![provider.clone()],
                ..Default::default()
            },
            &Default::default(),
        );
        let text = format!("{} jailbreak {}", "hello ".repeat(20), "hello ".repeat(20));
        let classified = classifiers
            .classify_flagged(&provider, &serde_json::json!(text))
            .await;
        assert!(!classified.truncated);
        let answer = classified
            .answer
            .expect("the whole text fits the byte budget");
        assert!(
            answer.categories["prompt_injection"] > 0.95,
            "the native token window silently discarded the middle: {:?}",
            answer.categories
        );
    }

    #[test]
    fn model_root_confines_the_files_a_manifest_names() {
        let Some(_model) = fixture() else { return };
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

    #[tokio::test]
    async fn native_token_windows_obey_on_truncated_fail_closed() {
        let Some(_model) = fixture() else { return };
        let mut provider =
            crate::local_classifier::tests::local_provider(&manifest_sha256(), 2_000);
        provider.model_path = MANIFEST.to_string();
        provider.max_body_bytes = 1_024;
        provider.max_scored_bytes = Some(8_192);
        provider.on_truncated = rolter_core::guardrail_webhook::OnTruncated::FailClosed;
        let classifiers = crate::local_classifier::LocalClassifiers::default();
        classifiers.configure(
            &rolter_core::SemanticGuardsConfig {
                providers: vec![provider.clone()],
                ..Default::default()
            },
            &Default::default(),
        );
        let text = "hello ".repeat(1_000);
        assert!(text.len() < provider.scored_bytes());
        let classified = classifiers
            .classify_flagged(&provider, &serde_json::json!(text))
            .await;
        assert!(
            classified.truncated,
            "the token window cap must be observable"
        );
        assert_eq!(
            classified.answer,
            Err(crate::guardrail_webhook::CallFailure::Truncated)
        );
    }

    #[tokio::test]
    async fn scoring_a_native_token_cut_flags_it_and_keeps_the_last_window_on_repeated_calls() {
        let Some(_model) = fixture() else { return };
        let mut provider =
            crate::local_classifier::tests::local_provider(&manifest_sha256(), 2_000);
        provider.model_path = MANIFEST.to_string();
        provider.max_body_bytes = 1_024;
        provider.max_scored_bytes = Some(8_192);
        provider.on_truncated = OnTruncated::Score;
        let classifiers = crate::local_classifier::LocalClassifiers::default();
        classifiers.configure(
            &rolter_core::SemanticGuardsConfig {
                providers: vec![provider.clone()],
                ..Default::default()
            },
            &Default::default(),
        );
        let text = format!("{} jailbreak", "hello ".repeat(1_000));
        assert!(text.len() < provider.scored_bytes());
        for _ in 0..2 {
            let classified = classifiers
                .classify_flagged(&provider, &serde_json::json!(text))
                .await;
            assert!(classified.truncated);
            let answer = classified.answer.expect("score mode admits a flagged cut");
            assert!(answer.categories["prompt_injection"] > 0.95);
        }
        let empty = classifiers
            .classify_flagged(&provider, &serde_json::json!(""))
            .await;
        assert!(!empty.truncated);
        assert!(empty.answer.unwrap().categories["prompt_injection"] < 0.05);
    }
}
