//! Predicted-latency scheduling (#853): rank targets by what *this* request is
//! expected to cost on each of them, rather than by what past requests averaged.
//!
//! The `fastest` strategy ranks on a per-target latency EWMA. That is the right
//! signal when every request is the same size, and the wrong one otherwise: a
//! target whose average is high because it happened to serve the long prompts
//! looks slow even when it is the emptiest box in the fleet, and a target with a
//! deep queue looks fast right up until the queue is what you join.
//!
//! So model latency instead of averaging it. Per target:
//!
//! ```text
//! latency_ms ≈ w0 + w1 · queue_depth + w2 · prompt_ktokens
//! ```
//!
//! `w0` is fixed overhead, `w1` is what one queued request ahead of you costs,
//! and `w2` is prefill cost per thousand prompt tokens. The three coefficients
//! are learned online from completed requests with [normalized least mean
//! squares][nlms] — one multiply-add per feature per sample, no matrix, no
//! allocation, no periodic refit.
//!
//! [nlms]: https://en.wikipedia.org/wiki/Least_mean_squares_filter
//!
//! ## Why this shape and not something richer
//!
//! A gateway sees queue depth and prompt size. It does not see batch
//! composition, KV-cache pressure, or where the engine is in its scheduling
//! loop, and a model with parameters it cannot observe fits noise. Three
//! features that each have a mechanical reason to matter is the honest ceiling
//! for this vantage point; the interesting error is between "the queue is deep"
//! and "the prompt is long", and a linear model separates those.
//!
//! ## Why it cannot mislead a cold route
//!
//! A target predicts nothing until it has [`MIN_SAMPLES`] completed requests,
//! and the scorer treats an unpredicted target as unknown rather than as slow.
//! A route that switches to `predicted_latency` therefore behaves exactly like
//! the least-load pipeline until the model has evidence, so the switch itself
//! moves no traffic.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::Mutex;

use crate::scorer::LatencyPredictionSource;
use crate::RouteContext;

/// Completed requests a target needs before its prediction is used. Below this
/// the coefficients are still dominated by their initial values, and a
/// confidently wrong prediction is worse than no prediction.
pub const MIN_SAMPLES: u64 = 8;

/// NLMS step size. Small enough that one outlier moves the fit a few percent,
/// large enough to track an upstream that genuinely got slower within tens of
/// requests.
const STEP: f64 = 0.15;

/// Guards the normalizer against a zero feature vector (queue 0, prompt 0),
/// where the update would otherwise divide by the bias term alone.
const EPSILON: f64 = 1e-3;

/// Upper bound on a learned coefficient, in milliseconds. Nothing observable
/// justifies "one queued request costs an hour"; a sample that implies it is a
/// stall, a clock jump, or a client that held a stream open, and letting it
/// through would poison every later prediction.
const MAX_COEFFICIENT: f64 = 600_000.0;

/// Feature vector for one request against one target.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Features {
    /// requests already in flight against the target when this one was dispatched
    pub queue_depth: u64,
    /// prompt size in tokens; `0` when the request carried none we could count
    pub prompt_tokens: usize,
}

impl Features {
    /// The design row `[1, queue, ktokens]`. Prompt size is scaled to thousands
    /// so all three features sit in the same order of magnitude — NLMS
    /// normalizes by `|x|²`, so a feature measured in raw tokens would swamp
    /// the other two and make the fit depend on prompt length alone.
    fn row(&self) -> [f64; 3] {
        [
            1.0,
            self.queue_depth as f64,
            self.prompt_tokens as f64 / 1000.0,
        ]
    }
}

/// One target's learned coefficients and the evidence behind them.
#[derive(Debug, Clone)]
struct TargetModel {
    weights: [f64; 3],
    samples: u64,
}

impl Default for TargetModel {
    fn default() -> Self {
        // all-zero weights predict 0 ms, which would look infinitely attractive
        // if it ever escaped. `MIN_SAMPLES` is what keeps it from escaping; the
        // zeros are simply the unbiased start
        Self {
            weights: [0.0; 3],
            samples: 0,
        }
    }
}

impl TargetModel {
    fn predict(&self, features: &Features) -> f64 {
        let x = features.row();
        let y: f64 = self
            .weights
            .iter()
            .zip(x.iter())
            .map(|(w, xi)| w * xi)
            .sum();
        // a linear fit can extrapolate below zero on a feature combination it
        // has not seen; negative latency is not a prediction, it is a bug
        y.max(0.0)
    }

    fn observe(&mut self, features: &Features, latency_ms: f64) {
        let x = features.row();
        let norm: f64 = EPSILON + x.iter().map(|xi| xi * xi).sum::<f64>();
        let error = latency_ms - self.predict(features);
        for (w, xi) in self.weights.iter_mut().zip(x.iter()) {
            *w += STEP * error * xi / norm;
            *w = w.clamp(-MAX_COEFFICIENT, MAX_COEFFICIENT);
        }
        self.samples = self.samples.saturating_add(1);
    }
}

/// Per-target latency models for one route, learned from completed requests.
///
/// Cheap to clone (the state is shared) so the gateway can hold one per route
/// and hand the same handle to both the balancer and the completion path.
#[derive(Debug, Default)]
pub struct LatencyPredictor {
    models: Mutex<Vec<TargetModel>>,
    /// total observations, readable without taking the lock, for telemetry and
    /// for the cheap "is this predictor cold" check
    observed: AtomicU64,
}

impl LatencyPredictor {
    /// A predictor for a route of `n` targets.
    #[must_use]
    pub fn new(n: usize) -> Self {
        Self {
            models: Mutex::new(vec![TargetModel::default(); n]),
            observed: AtomicU64::new(0),
        }
    }

    /// Total completed requests folded into the models.
    #[must_use]
    pub fn observed(&self) -> u64 {
        self.observed.load(Relaxed)
    }

    /// Fold one completed request into `target`'s model. Out-of-range targets
    /// and non-finite or negative latencies are ignored: a bad sample must not
    /// be able to teach the model anything.
    pub fn observe(&self, target: usize, features: Features, latency_ms: f64) {
        if !latency_ms.is_finite() || latency_ms < 0.0 {
            return;
        }
        let mut models = self.models.lock().unwrap_or_else(|e| e.into_inner());
        let Some(model) = models.get_mut(target) else {
            return;
        };
        model.observe(&features, latency_ms);
        drop(models);
        self.observed.fetch_add(1, Relaxed);
    }

    /// Predicted latency in milliseconds for each target `0..n`, or `None` for
    /// a target with too little evidence to be trusted.
    #[must_use]
    pub fn predict(&self, n: usize, loads: &[u64], prompt_tokens: usize) -> Vec<Option<f64>> {
        let models = self.models.lock().unwrap_or_else(|e| e.into_inner());
        (0..n)
            .map(|i| {
                let model = models.get(i)?;
                if model.samples < MIN_SAMPLES {
                    return None;
                }
                Some(model.predict(&Features {
                    // an unknown queue depth is `0`, not "skip": the bias and
                    // prompt terms still carry a usable prediction
                    queue_depth: loads.get(i).copied().unwrap_or(0),
                    prompt_tokens,
                }))
            })
            .collect()
    }

    /// The learned coefficients for `target` as `[fixed_ms, per_queued_ms,
    /// per_ktoken_ms]`, for telemetry and tests. `None` for an unknown target.
    #[must_use]
    pub fn coefficients(&self, target: usize) -> Option<[f64; 3]> {
        let models = self.models.lock().unwrap_or_else(|e| e.into_inner());
        models.get(target).map(|m| m.weights)
    }

    /// Completed requests folded into `target`'s model.
    #[must_use]
    pub fn samples(&self, target: usize) -> u64 {
        let models = self.models.lock().unwrap_or_else(|e| e.into_inner());
        models.get(target).map_or(0, |m| m.samples)
    }
}

impl LatencyPredictionSource for LatencyPredictor {
    fn predict(&self, n: usize, loads: &[u64], prompt_tokens: usize) -> Vec<Option<f64>> {
        LatencyPredictor::predict(self, n, loads, prompt_tokens)
    }
}

/// Prompt size in tokens for a routing context, in the order the estimate is
/// trustworthy: exact ids when the caller tokenized, then a character-based
/// estimate, then nothing.
///
/// The `/ 4` is the usual English rule of thumb. It is wrong for code and wrong
/// for CJK, but the model learns a coefficient *over* whatever this returns, so
/// a consistent bias is absorbed into `w2` — what matters is that the estimate
/// scales with the real prompt, not that it is accurate. That is why it reads
/// [`RouteContext::prompt_len`] ahead of the prompt text: the gateway hands
/// over only the leading bytes of a long prompt, and an estimate taken from
/// those would stop growing at the cut.
#[must_use]
pub fn prompt_tokens(ctx: &RouteContext) -> usize {
    if let Some(ids) = ctx.token_ids {
        return ids.len();
    }
    ctx.prompt_len
        .or(ctx.prompt.map(str::len))
        .map_or(0, |bytes| bytes / 4)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drive a model with a known linear truth and check it converges to it.
    fn train(model: &LatencyPredictor, target: usize, truth: impl Fn(&Features) -> f64, n: usize) {
        for i in 0..n {
            let features = Features {
                queue_depth: (i % 7) as u64,
                prompt_tokens: (i % 5) * 500,
            };
            model.observe(target, features, truth(&features));
        }
    }

    #[test]
    fn a_cold_target_predicts_nothing() {
        let p = LatencyPredictor::new(2);
        assert_eq!(p.predict(2, &[0, 0], 100), vec![None, None]);
        // still cold one sample short of the threshold
        for _ in 0..MIN_SAMPLES - 1 {
            p.observe(
                0,
                Features {
                    queue_depth: 1,
                    prompt_tokens: 100,
                },
                50.0,
            );
        }
        assert_eq!(p.predict(2, &[0, 0], 100)[0], None);
    }

    #[test]
    fn it_learns_a_linear_latency_law() {
        let p = LatencyPredictor::new(1);
        // 20ms fixed + 30ms per queued request + 40ms per thousand tokens
        train(
            &p,
            0,
            |f| 20.0 + 30.0 * f.queue_depth as f64 + 40.0 * (f.prompt_tokens as f64 / 1000.0),
            400,
        );
        let [fixed, per_queued, per_ktok] = p.coefficients(0).unwrap();
        assert!(
            (fixed - 20.0).abs() < 5.0,
            "fixed overhead {fixed} should approach 20"
        );
        assert!(
            (per_queued - 30.0).abs() < 5.0,
            "queue cost {per_queued} should approach 30"
        );
        assert!(
            (per_ktok - 40.0).abs() < 5.0,
            "prefill cost {per_ktok} should approach 40"
        );
    }

    #[test]
    fn it_separates_a_deep_queue_from_a_long_prompt() {
        let p = LatencyPredictor::new(2);
        // target 0 is queue-sensitive and cheap to prefill: a busy small box
        train(&p, 0, |f| 10.0 + 100.0 * f.queue_depth as f64, 400);
        // target 1 barely notices its queue but prefills slowly: a big batched box
        train(
            &p,
            1,
            |f| 10.0 + 5.0 * f.queue_depth as f64 + 300.0 * (f.prompt_tokens as f64 / 1000.0),
            400,
        );

        // a short prompt arriving while both are busy belongs on target 1
        let busy = p.predict(2, &[4, 4], 50);
        assert!(
            busy[1].unwrap() < busy[0].unwrap(),
            "short prompt, deep queues: {busy:?}"
        );
        // a long prompt arriving while both are idle belongs on target 0
        let long = p.predict(2, &[0, 0], 4000);
        assert!(
            long[0].unwrap() < long[1].unwrap(),
            "long prompt, idle queues: {long:?}"
        );
    }

    #[test]
    fn it_tracks_an_upstream_that_got_slower() {
        let p = LatencyPredictor::new(1);
        train(&p, 0, |_| 50.0, 200);
        let before = p.predict(1, &[0], 0)[0].unwrap();
        assert!((before - 50.0).abs() < 8.0, "settled at {before}");
        train(&p, 0, |_| 500.0, 200);
        let after = p.predict(1, &[0], 0)[0].unwrap();
        assert!(after > 400.0, "should have followed the shift, got {after}");
    }

    #[test]
    fn a_prediction_is_never_negative() {
        let p = LatencyPredictor::new(1);
        // a law with a negative queue term drives a coefficient below zero
        train(
            &p,
            0,
            |f| (100.0 - 200.0 * f.queue_depth as f64).max(0.0),
            200,
        );
        for queue in 0..50u64 {
            let predicted = p.predict(1, &[queue], 0)[0].unwrap();
            assert!(predicted >= 0.0, "queue {queue} predicted {predicted}");
        }
    }

    #[test]
    fn bad_samples_teach_nothing() {
        let p = LatencyPredictor::new(1);
        p.observe(
            0,
            Features {
                queue_depth: 1,
                prompt_tokens: 10,
            },
            f64::NAN,
        );
        p.observe(
            0,
            Features {
                queue_depth: 1,
                prompt_tokens: 10,
            },
            -5.0,
        );
        // out-of-range target
        p.observe(
            9,
            Features {
                queue_depth: 1,
                prompt_tokens: 10,
            },
            5.0,
        );
        assert_eq!(p.observed(), 0);
        assert_eq!(p.samples(0), 0);
        assert!(p.coefficients(0).unwrap().iter().all(|w| *w == 0.0));
    }

    #[test]
    fn a_stalled_sample_cannot_poison_the_model() {
        let p = LatencyPredictor::new(1);
        train(&p, 0, |_| 50.0, 200);
        // a client that held a stream open for a week
        p.observe(
            0,
            Features {
                queue_depth: 1,
                prompt_tokens: 100,
            },
            7.0 * 24.0 * 3600.0 * 1000.0,
        );
        assert!(
            p.coefficients(0)
                .unwrap()
                .iter()
                .all(|w| w.abs() <= MAX_COEFFICIENT),
            "coefficients escaped their clamp: {:?}",
            p.coefficients(0)
        );
        // and the model recovers rather than staying wrecked
        train(&p, 0, |_| 50.0, 400);
        let recovered = p.predict(1, &[0], 0)[0].unwrap();
        assert!(recovered < 200.0, "still poisoned at {recovered}");
    }

    #[test]
    fn prompt_tokens_prefers_exact_ids_then_estimates() {
        let ids = [1u32, 2, 3, 4, 5];
        let ctx = RouteContext {
            token_ids: Some(&ids),
            prompt: Some("this text is ignored when ids are present"),
            ..Default::default()
        };
        assert_eq!(prompt_tokens(&ctx), 5);

        let ctx = RouteContext {
            prompt: Some("12345678"),
            ..Default::default()
        };
        assert_eq!(prompt_tokens(&ctx), 2);

        assert_eq!(prompt_tokens(&RouteContext::default()), 0);

        // a bounded prompt is sized by the whole prompt it was cut from
        let ctx = RouteContext {
            prompt: Some("12345678"),
            prompt_len: Some(200_000),
            ..Default::default()
        };
        assert_eq!(prompt_tokens(&ctx), 50_000);
    }
}
