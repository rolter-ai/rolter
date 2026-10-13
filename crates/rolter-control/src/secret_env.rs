//! Which environment variables an operator may name as an outbound credential.
//!
//! A provider's `api_key_env` and a guardrail webhook's `auth_env` are read
//! from the control plane's own environment when a test runs, and the url they
//! are sent to is the operator's to choose. Without a refusal, naming
//! `ROLTER_KEK` and pointing the url at a host of one's own sends the control
//! plane's secret out. The predicate itself lives in `rolter-core`, since the
//! gateway applies it to the same names. Every path that reads a caller-named variable asks
//! [`refuse`] first, so the list lives in one place.

use crate::crud::{invalid_field, ApiError};

pub(crate) use rolter_core::is_control_plane_secret;

/// A 400 `invalid_field` on `field` when `name` is a control-plane secret.
/// `what` completes "… cannot be used to test …".
pub(crate) fn refuse(field: &str, name: &str, what: &str) -> Result<(), ApiError> {
    if is_control_plane_secret(name) {
        return Err(invalid_field(
            field,
            format!("{name} holds a control-plane secret and cannot be used {what}"),
        ));
    }
    Ok(())
}
