//! URL-safe provider slug helpers.
//!
//! A provider slug is a stable, URL-safe identity used for
//! `provider-slug/model` addressing (ADR: provider/model addressing). Unlike a
//! display name it is immutable by default, so an address never breaks when a
//! provider is renamed. The canonical charset is `^[a-z0-9][a-z0-9-]{0,62}$`:
//! lowercase alphanumerics and hyphens, first character alphanumeric, 1..=63
//! characters total.

/// maximum slug length in characters
pub const SLUG_MAX_LEN: usize = 63;

/// Whether `s` is a valid provider slug: `^[a-z0-9][a-z0-9-]{0,62}$`.
pub fn is_valid_slug(s: &str) -> bool {
    let len = s.chars().count();
    if !(1..=SLUG_MAX_LEN).contains(&len) {
        return false;
    }
    let mut chars = s.chars();
    let first = chars.next().unwrap();
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return false;
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The provider or group slug a `slug/model` address names, when `model` has
/// that shape: a valid slug before the first `/` and something after it.
///
/// This is the split the gateway applies once no route by the full name
/// answers, so a route whose name has this shape sits on that address.
pub fn address_slug(model: &str) -> Option<&str> {
    let (slug, upstream) = model.split_once('/')?;
    (!upstream.is_empty() && is_valid_slug(slug)).then_some(slug)
}

/// Derive a candidate slug from a display `name`: lowercase, non-alphanumerics
/// collapse to single hyphens, leading/trailing hyphens trimmed, truncated to
/// [`SLUG_MAX_LEN`]. The result satisfies [`is_valid_slug`] as long as `name`
/// contains at least one ascii alphanumeric; otherwise it returns an empty
/// string and the caller must supply an explicit slug.
pub fn slugify(name: &str) -> String {
    let mut out = String::with_capacity(name.len().min(SLUG_MAX_LEN));
    let mut last_was_dash = true;

    for c in name.chars() {
        for lower_c in c.to_lowercase() {
            if lower_c.is_ascii_alphanumeric() {
                if out.len() >= SLUG_MAX_LEN {
                    return out;
                }
                out.push(lower_c);
                last_was_dash = false;
            } else if !last_was_dash {
                if out.len() >= SLUG_MAX_LEN {
                    return out;
                }
                out.push('-');
                last_was_dash = true;
            }
        }
    }

    if out.ends_with('-') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_canonical_slugs() {
        assert!(is_valid_slug("openai"));
        assert!(is_valid_slug("vllm-msk"));
        assert!(is_valid_slug("a"));
        assert!(is_valid_slug("0"));
        assert!(is_valid_slug(&"a".repeat(SLUG_MAX_LEN)));
    }

    #[test]
    fn rejects_invalid_slugs() {
        assert!(!is_valid_slug(""));
        assert!(!is_valid_slug("-lead"));
        assert!(!is_valid_slug("Upper"));
        assert!(!is_valid_slug("has space"));
        assert!(!is_valid_slug("under_score"));
        assert!(!is_valid_slug("slash/model"));
        assert!(!is_valid_slug(&"a".repeat(SLUG_MAX_LEN + 1)));
    }

    #[test]
    fn slugify_normalizes() {
        assert_eq!(slugify("OpenAI"), "openai");
        assert_eq!(slugify("vLLM MSK!"), "vllm-msk");
        assert_eq!(slugify("  multi  space "), "multi-space");
        assert_eq!(slugify("trailing-"), "trailing");
        assert_eq!(slugify("非ascii"), "ascii");
        assert_eq!(slugify("Kelvin"), "kelvin");
        assert_eq!(slugify("İProvider"), "i-provider");
    }

    #[test]
    fn slugify_output_is_valid_or_empty() {
        for name in ["OpenAI", "vLLM MSK", "a", "123"] {
            assert!(is_valid_slug(&slugify(name)));
        }
        assert_eq!(slugify("非"), "");
    }

    #[test]
    fn slugify_truncates_to_max_len() {
        let long = "x".repeat(100);
        assert_eq!(slugify(&long).chars().count(), SLUG_MAX_LEN);
    }

    #[test]
    fn address_slug_is_the_segment_the_gateway_pins() {
        assert_eq!(address_slug("edge/gpt-4o"), Some("edge"));
        assert_eq!(address_slug("edge/org/model:tag"), Some("edge"));
        // a slug is lower-case, so an hf-style org segment is never one
        assert_eq!(address_slug("Qwen/Qwen2.5-7B"), None);
        assert_eq!(address_slug("edge/"), None);
        assert_eq!(address_slug("gpt-4o"), None);
    }
}
