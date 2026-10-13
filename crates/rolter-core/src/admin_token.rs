//! The shortest `ROLTER_ADMIN_TOKEN` rolter accepts as the credential that
//! closes the management API (#1964).
//!
//! Whoever presents the token is superadmin, so a placeholder (`...`, `x`,
//! `changeme`) is as good as no token while looking like one. The control
//! plane, `rolter easy-up` and `rolter check` all read the same rule from here,
//! the way [`MIN_ADMIN_TOKEN_LEN`] sits next to what `openssl rand -hex 16`
//! prints.

/// Fewest characters, after trimming, an admin token may have.
pub const MIN_ADMIN_TOKEN_LEN: usize = 32;

/// The token is set but too short to close the management API.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "ROLTER_ADMIN_TOKEN is {len} characters; use at least {MIN_ADMIN_TOKEN_LEN}. It is the \
     credential that makes a caller superadmin, so a short or guessable value does not close \
     the management API. Generate one with `openssl rand -hex 32`"
)]
pub struct AdminTokenTooShort {
    /// length of the trimmed token
    pub len: usize,
}

/// Check a raw `ROLTER_ADMIN_TOKEN` value. A blank value is no token at all
/// (every caller already treats it as unset) and passes here; whether a token
/// is required is a separate decision.
pub fn check(raw: &str) -> Result<(), AdminTokenTooShort> {
    let len = raw.trim().chars().count();
    if len == 0 || len >= MIN_ADMIN_TOKEN_LEN {
        Ok(())
    } else {
        Err(AdminTokenTooShort { len })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_token_is_refused_with_the_generation_hint() {
        for short in ["...", "x", "changeme", &"a".repeat(MIN_ADMIN_TOKEN_LEN - 1)] {
            let err = check(short).expect_err(short);
            assert_eq!(err.len, short.len());
            assert!(err.to_string().contains("openssl rand -hex 32"), "{err}");
        }
    }

    #[test]
    fn a_blank_token_is_unset_not_short() {
        assert!(check("").is_ok());
        assert!(check("   \n").is_ok());
    }

    #[test]
    fn a_generated_token_passes() {
        assert!(check(&"f".repeat(MIN_ADMIN_TOKEN_LEN)).is_ok());
        assert!(check(&"0123456789abcdef".repeat(4)).is_ok());
    }

    #[test]
    fn padding_does_not_count_toward_the_length() {
        let padded = format!("  {}  ", "a".repeat(MIN_ADMIN_TOKEN_LEN - 1));
        assert!(check(&padded).is_err());
    }
}
