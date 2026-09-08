//! Which subsystems are experimental, for the dashboard's nav markers (#1385).
//!
//! The list itself lives in `rolter_core::stability` as a compile-time table,
//! because how finished a subsystem is a fact about the build rather than about
//! a deployment. This module only serves it, so the dashboard never keeps a
//! second copy of the list — a duplicated list in `ui/` that drifts from the
//! backend's is the failure mode the marker exists to avoid.
//!
//! Only non-stable entries cross the wire. Absence means stable, so an empty
//! array is the healthy answer for a release with nothing experimental left in
//! it, and the dashboard never has to strip a badge it should not have drawn.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};

use rolter_core::stability::{SubsystemStability, SUBSYSTEMS};

use crate::crud::ApiResult;
use crate::rbac::{authorize, Principal, ScopeChain};
use crate::rbac_matrix::cap;
use crate::ControlState;

pub(crate) fn router() -> Router<ControlState> {
    Router::new().route("/api/v1/stability", get(get_stability))
}

/// The subsystems this build marks as experimental.
///
/// Readable by every authenticated caller, including a viewer: the nav rail is
/// rendered for everyone who can sign in, and a marker the least-privileged
/// user cannot fetch is a marker that is missing exactly where the reassurance
/// is worth most. It names no tenant's data — only what this build ships.
async fn get_stability(
    principal: Principal,
    State(state): State<ControlState>,
) -> ApiResult<Json<&'static [SubsystemStability]>> {
    authorize(
        &state,
        &principal,
        ScopeChain::default(),
        cap!("stability", Read),
    )
    .await?;
    Ok(Json(SUBSYSTEMS))
}

#[cfg(test)]
mod tests {
    use rolter_core::stability::{Stability, SUBSYSTEMS};

    #[test]
    fn the_served_payload_is_the_core_table() {
        let json = serde_json::to_value(SUBSYSTEMS).expect("serializes");
        let rows = json.as_array().expect("an array");
        assert_eq!(rows.len(), SUBSYSTEMS.len());
        for row in rows {
            assert_eq!(
                row["stability"], "experimental",
                "only exceptions are served; stable is the absent default"
            );
            assert!(row["id"].is_string(), "{row}");
            assert!(row["note"].is_string(), "{row}");
            assert!(row["nav_keys"].is_array(), "{row}");
        }
        assert_eq!(Stability::default(), Stability::Stable);
    }
}
