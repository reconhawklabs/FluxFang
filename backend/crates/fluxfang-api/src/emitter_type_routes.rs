//! `GET /api/emitter-types/:kind`: exposes
//! `fluxfang_core::emitter_types_for_kind` as JSON, driving the frontend's
//! emitter-create "type" dropdown for a data source `kind` (e.g. `wifi`) —
//! replacing a free-text field. PROTECTED — mounted in `lib.rs::app`'s
//! protected router group, behind `require_auth`, same convention as
//! `catalog_routes`.
//!
//! An unknown `kind` returns `200 OK` with an empty array rather than
//! `404`, matching `emitter_types_for_kind`'s own "unknown kinds have no
//! types" behavior — same rationale as `GET /api/catalog/:kind`.

use axum::extract::Path;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

use fluxfang_core::emitter_types_for_kind;
use fluxfang_core::EmitterTypeInfo;

use crate::state::AppState;

pub fn protected_routes() -> Router<AppState> {
    Router::new().route("/api/emitter-types/:kind", get(list_emitter_types))
}

/// One emitter type over the wire: `key` (machine value, what a client
/// sends back as `POST /api/emitters`' `emitter_type`) plus `label`
/// (human-readable, for display in the dropdown).
#[derive(Debug, Serialize)]
struct EmitterTypeDto {
    key: &'static str,
    label: &'static str,
}

impl From<EmitterTypeInfo> for EmitterTypeDto {
    fn from(info: EmitterTypeInfo) -> Self {
        EmitterTypeDto {
            key: info.key,
            label: info.label,
        }
    }
}

async fn list_emitter_types(Path(kind): Path<String>) -> Json<Vec<EmitterTypeDto>> {
    let types = emitter_types_for_kind(&kind);
    Json(types.into_iter().map(EmitterTypeDto::from).collect())
}
