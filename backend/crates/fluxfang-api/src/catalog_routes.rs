//! `GET /api/catalog/:kind` (Task 6.1): exposes `fluxfang_core::catalog_for`
//! as JSON, driving the frontend's field/operator dropdowns for a data
//! source `kind` (e.g. `wifi`). PROTECTED — mounted in `lib.rs::app`'s
//! protected router group, behind `require_auth`, since it's not part of
//! the fixed public surface (`{/api/health, /api/setup/status, /api/setup,
//! /api/login}`).
//!
//! An unknown `kind` returns `200 OK` with an empty array rather than `404`,
//! matching `catalog_for`'s own "unknown kinds return an empty catalog"
//! behavior — there's no separate notion of "this kind doesn't exist" to
//! signal beyond "it has no fields".

use axum::extract::Path;
use axum::routing::get;
use axum::{Json, Router};

use fluxfang_core::catalog::catalog_for;

use crate::dto::FieldDefDto;
use crate::state::AppState;

pub fn protected_routes() -> Router<AppState> {
    Router::new().route("/api/catalog/:kind", get(catalog))
}

async fn catalog(Path(kind): Path<String>) -> Json<Vec<FieldDefDto>> {
    let fields = catalog_for(&kind);
    Json(fields.iter().map(FieldDefDto::from).collect())
}
