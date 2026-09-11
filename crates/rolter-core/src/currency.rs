//! Currency codes and conversion for cost accounting.
//!
//! A currency code is an **open string**, never an enum. Today that means
//! ISO-4217 (`USD`, `EUR`, `GBP`); tomorrow it may mean a crypto or custom
//! settlement unit (`BTC`, `USDC`, internal credits). Adding one must cost a
//! rate-table entry and nothing else — no code change anywhere in the stack.

use std::collections::HashMap;

use rust_decimal::Decimal;

use serde::{Deserialize, Serialize};

/// Serde for the rate table: [`Decimal`] in memory, JSON/TOML numbers on the
/// wire.
///
/// `rust_decimal` ships `serde::float` for a bare field but nothing for a map
/// value, so the two conversions live here. Keeping the wire as numbers is
/// deliberate and is explained on [`crate::ModelPriceConfig`] — a snapshot is
/// exchanged between a control plane and a gateway that may be different
/// builds, and this is not the release to change that shape in.
mod rates_as_floats {
    use std::collections::HashMap;

    use rust_decimal::prelude::{FromPrimitive, ToPrimitive};
    use rust_decimal::Decimal;
    use serde::de::Error as _;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub(super) fn serialize<S: Serializer>(
        rates: &HashMap<String, Decimal>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        rates
            .iter()
            .map(|(code, rate)| (code, rate.to_f64().unwrap_or(f64::NAN)))
            .collect::<HashMap<_, _>>()
            .serialize(serializer)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<HashMap<String, Decimal>, D::Error> {
        HashMap::<String, f64>::deserialize(deserializer)?
            .into_iter()
            .map(|(code, rate)| {
                // a rate that is not a finite number is a misconfiguration, and
                // silently dropping it would convert at the wrong number rather
                // than refuse to convert at all
                Decimal::from_f64(rate)
                    .map(|rate| (code.clone(), rate))
                    .ok_or_else(|| {
                        D::Error::custom(format!(
                            "currency rate for '{code}' is not a finite number"
                        ))
                    })
            })
            .collect()
    }
}

/// The settlement currency spend accumulates in when none is configured.
pub const DEFAULT_BASE_CURRENCY: &str = "USD";

pub(crate) fn default_base_currency() -> String {
    DEFAULT_BASE_CURRENCY.to_string()
}

/// Normalize a currency code for lookup: trimmed and upper-cased, so `usd`,
/// `USD ` and `Usd` are the same currency. Codes are compared this way
/// everywhere; the original spelling is preserved for display.
pub fn normalize_code(code: &str) -> String {
    code.trim().to_ascii_uppercase()
}

/// Convert an amount between currency codes.
///
/// Implementations are free to source rates however they like — an operator's
/// static table, a slow-cadence feed, a ledger — but they must be honest about
/// what they do not know: an unknown pair returns `None` rather than a guess,
/// so the caller can fail closed instead of silently charging the wrong number.
pub trait CurrencyConverter: Send + Sync {
    /// Amount expressed in `to`, or `None` when the pair is not convertible.
    fn convert(&self, amount: Decimal, from: &str, to: &str) -> Option<Decimal>;
}

/// Operator-configured currency settings: the base everything settles in, plus
/// a static rate table.
///
/// Static rates are the only offline-safe source, and rolter must run
/// air-gapped, so this is the default and every other source degrades to it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CurrencyConfig {
    /// currency all budgets and accumulated spend are denominated in
    #[serde(default = "default_base_currency")]
    pub base: String,
    /// how many units of `base` one unit of the keyed currency is worth
    /// (`EUR = 1.09` means one euro costs 1.09 base units when base is USD).
    /// The base itself is implicitly 1.0 and need not be listed.
    ///
    /// Decimal rather than `f64` so a conversion is exact (#967); kept as JSON
    /// numbers on the wire for the reason [`crate::ModelPriceConfig`] gives.
    #[serde(default, with = "rates_as_floats")]
    pub rates: HashMap<String, Decimal>,
}

impl Default for CurrencyConfig {
    fn default() -> Self {
        Self {
            base: default_base_currency(),
            rates: HashMap::new(),
        }
    }
}

impl CurrencyConfig {
    /// Base currency, normalized for comparison.
    pub fn base_code(&self) -> String {
        normalize_code(&self.base)
    }

    /// Rate for `code` in units of base, or `None` when the table has no entry.
    /// The base currency is always 1.0 without needing a row.
    pub fn rate(&self, code: &str) -> Option<Decimal> {
        let code = normalize_code(code);
        if code == self.base_code() {
            return Some(Decimal::ONE);
        }
        self.rates
            .iter()
            .find(|(k, _)| normalize_code(k) == code)
            .map(|(_, rate)| *rate)
    }

    /// Every currency this deployment can price in: the base plus each code in
    /// the rate table, normalized and deduplicated, base first and the rest in
    /// alphabetical order.
    ///
    /// This is exactly the set [`Self::rate`] answers for, which is what makes
    /// it safe to offer as a chooser: the dashboard used to hardcode seven
    /// ISO-4217 codes, so a configured `RUB` was unselectable while an offered
    /// `JPY` with no rate was rejected on save (#965). Deriving both the
    /// chooser and the validator from one function means neither can drift.
    pub fn codes(&self) -> Vec<String> {
        let base = self.base_code();
        let mut rest: Vec<String> = self
            .rates
            .keys()
            .map(|code| normalize_code(code))
            .filter(|code| !code.is_empty() && *code != base)
            .collect();
        rest.sort();
        rest.dedup();
        // the base leads because it is the currency spend settles in, and an
        // operator picking "the default" should land on it without hunting
        std::iter::once(base).chain(rest).collect()
    }

    /// Problems that make this table unusable, in the shape
    /// [`crate::GatewayConfig::validate`] collects.
    pub fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if self.base.trim().is_empty() {
            problems.push("currency.base must not be empty".to_string());
        }
        for (code, rate) in &self.rates {
            if code.trim().is_empty() {
                problems.push("currency.rates has an empty currency code".to_string());
            }
            // a Decimal is finite by construction, so the non-finite half of
            // this check is now enforced at deserialization (where a NaN or an
            // infinity is refused) rather than here
            if *rate <= Decimal::ZERO {
                problems.push(format!(
                    "currency.rates['{code}'] must be a positive rate (got {rate})"
                ));
            }
        }
        problems
    }
}

/// A [`CurrencyConverter`] over an operator-supplied rate table. Works
/// offline, which is the point.
#[derive(Debug, Clone, Default)]
pub struct StaticRates {
    config: CurrencyConfig,
}

impl StaticRates {
    pub fn new(config: CurrencyConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &CurrencyConfig {
        &self.config
    }
}

impl CurrencyConverter for StaticRates {
    fn convert(&self, amount: Decimal, from: &str, to: &str) -> Option<Decimal> {
        let (from, to) = (normalize_code(from), normalize_code(to));
        if from == to {
            return Some(amount);
        }
        // both legs go through the base, so a table of N codes converts any of
        // the N*N pairs without listing them
        let from_rate = self.config.rate(&from)?;
        let to_rate = self.config.rate(&to)?;
        if to_rate.is_zero() {
            return None;
        }
        Some(amount * from_rate / to_rate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A decimal literal for tests. `rust_decimal`'s `dec!` macro would read
    /// slightly better, but its `macros` feature pulls `rust_decimal_macros`,
    /// `proc-macro-crate`, `toml_edit` and `borsh` into the dependency graph in
    /// production position, which is a poor trade for test ergonomics (#967).
    fn d(literal: &str) -> rust_decimal::Decimal {
        literal.parse().expect("a valid decimal literal")
    }

    fn rates() -> StaticRates {
        StaticRates::new(CurrencyConfig {
            base: "USD".to_string(),
            rates: HashMap::from([
                ("EUR".to_string(), d("1.10")),
                ("GBP".to_string(), d("1.25")),
                // an open code set: no enum lists this, only the table does
                ("BTC".to_string(), d("60000.0")),
            ]),
        })
    }

    #[test]
    fn normalizes_currency_codes() {
        assert_eq!(normalize_code("usd"), "USD");
        assert_eq!(normalize_code("USD"), "USD");
        assert_eq!(normalize_code("Usd"), "USD");
        assert_eq!(normalize_code(" usd "), "USD");
        assert_eq!(normalize_code(""), "");
        assert_eq!(normalize_code("   "), "");
    }

    #[test]
    fn converts_into_and_out_of_the_base() {
        let fx = rates();
        assert_eq!(fx.convert(d("10"), "EUR", "USD"), Some(d("11.00")));
        assert_eq!(fx.convert(d("11"), "USD", "EUR"), Some(d("10")));
        assert_eq!(fx.convert(d("1"), "USD", "USD"), Some(d("1")));
    }

    #[test]
    fn converts_between_two_non_base_currencies() {
        // 125/1.1 does not terminate, so this is the one place the result is
        // rounded rather than exact. Decimal rounds at 28 significant digits
        // instead of 15-16, and — unlike `f64` — it rounds the *decimal*
        // expansion, so the value reads as the number a person would write
        let converted = rates().convert(d("100"), "GBP", "EUR").unwrap();
        assert_eq!(converted, d("113.63636363636363636363636364"));
    }

    #[test]
    fn a_new_code_needs_only_a_table_entry() {
        // the acceptance criterion from #650: no code change, no enum edit
        let fx = rates();
        assert_eq!(fx.convert(d("2"), "BTC", "USD"), Some(d("120000.0")));
    }

    #[test]
    fn codes_are_case_and_whitespace_insensitive() {
        assert_eq!(rates().convert(d("10"), " eur ", "usd"), Some(d("11.00")));
    }

    #[test]
    fn an_unknown_pair_is_none_rather_than_a_guess() {
        // the whole point: the caller must be able to fail closed
        assert_eq!(rates().convert(d("10"), "XYZ", "USD"), None);
        assert_eq!(rates().convert(d("10"), "USD", "XYZ"), None);
    }

    #[test]
    fn codes_lead_with_the_base_then_sort() {
        assert_eq!(rates().config.codes(), ["USD", "BTC", "EUR", "GBP"]);
    }

    #[test]
    fn codes_are_exactly_the_set_that_has_a_rate() {
        // the invariant the dashboard chooser rests on: everything offered is
        // priceable, and everything priceable is offered
        let fx = rates();
        for code in fx.config.codes() {
            assert!(
                fx.config.rate(&code).is_some(),
                "offered but unpriceable: {code}"
            );
        }
        assert!(fx.config.rate("XYZ").is_none());
        assert!(!fx.config.codes().contains(&"XYZ".to_string()));
    }

    #[test]
    fn codes_are_not_a_fixed_set() {
        // #965: adding a currency must cost a rate-table entry and nothing else
        let mut config = CurrencyConfig::default();
        assert_eq!(config.codes(), ["USD"]);
        config.rates.insert("RUB".to_string(), d("0.011"));
        assert_eq!(config.codes(), ["USD", "RUB"]);
    }

    #[test]
    fn codes_normalize_and_never_repeat_the_base() {
        let config = CurrencyConfig {
            base: " usd ".to_string(),
            rates: HashMap::from([
                // the base needs no row, but listing it must not double it up
                ("usd".to_string(), d("1.0")),
                (" rub ".to_string(), d("0.011")),
                // an empty code is a config defect, not a currency to offer
                ("".to_string(), d("2.0")),
            ]),
        };
        assert_eq!(config.codes(), ["USD", "RUB"]);
    }

    #[test]
    fn a_non_base_currency_is_offered_only_once_it_has_a_rate() {
        let mut config = CurrencyConfig::default();
        assert!(!config.codes().contains(&"EUR".to_string()));
        config.rates.insert("EUR".to_string(), d("1.09"));
        assert!(config.codes().contains(&"EUR".to_string()));
    }

    #[test]
    fn problems_reject_an_unusable_table() {
        let bad = CurrencyConfig {
            base: "USD".to_string(),
            rates: HashMap::from([("EUR".to_string(), d("0.0"))]),
        };
        assert_eq!(bad.problems().len(), 1, "{:?}", bad.problems());

        let negative = CurrencyConfig {
            base: "USD".to_string(),
            rates: HashMap::from([("EUR".to_string(), d("-1.0"))]),
        };
        assert_eq!(negative.problems().len(), 1);

        assert!(CurrencyConfig::default().problems().is_empty());
    }
}
