//! Wire DTOs — types that control the exact JSON shape the API returns,
//! kept separate from `fluxfang-core`'s domain types so the wire format can
//! evolve independently of internal representations (and so a core struct's
//! default `serde` derive never leaks onto the wire by accident; see
//! [`FieldDefDto`]'s docs).

use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use fluxfang_core::catalog::{FieldDef, FieldType};
use fluxfang_core::rule::Op;

use fluxfang_db::models::{Emission, Emitter, Entity, Zone};

/// One operator as exposed over the wire: its `serde` code plus a
/// plain-English label the frontend can render directly in a dropdown.
#[derive(Debug, Clone, Serialize)]
pub struct OpDto {
    pub code: &'static str,
    pub label: &'static str,
}

/// Map a core [`Op`] to its wire `code` (matching `Op`'s own `#[serde]`
/// names) and a plain-English label for the UI.
fn op_dto(op: &Op) -> OpDto {
    match op {
        Op::Eq => OpDto {
            code: "eq",
            label: "is exactly",
        },
        Op::Neq => OpDto {
            code: "neq",
            label: "is not",
        },
        Op::Matches => OpDto {
            code: "matches",
            label: "contains / matches",
        },
        Op::In => OpDto {
            code: "in",
            label: "is any of",
        },
        Op::Gte => OpDto {
            code: "gte",
            label: "is at least",
        },
        Op::Lte => OpDto {
            code: "lte",
            label: "is at most",
        },
    }
}

/// One field in a `GET /api/catalog/:kind` response.
///
/// Deliberately hand-built rather than `#[derive(Serialize)]`-ing
/// `fluxfang_core::catalog::FieldDef` directly: that struct's field is named
/// `ty` (a reserved-word dodge for `type`) and would serialize to the wire
/// as `"ty"` under its own derive. The review of Task 3.1 flagged exactly
/// this trap, so this DTO explicitly renames it to `"type"` and additionally
/// flattens `Enum(Vec<String>)`'s payload into a sibling `"values"` field
/// (present only for enum-typed fields) rather than nesting it, since a
/// nested `{"Enum": [...]}` shape would leak the core enum's tag name onto
/// the wire too.
#[derive(Debug, Clone, Serialize)]
pub struct FieldDefDto {
    pub key: String,
    pub label: String,
    #[serde(rename = "type")]
    pub ty: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<String>>,
    pub ops: Vec<OpDto>,
}

impl From<&FieldDef> for FieldDefDto {
    fn from(field: &FieldDef) -> Self {
        let (ty, values) = match &field.ty {
            FieldType::Text => ("text", None),
            FieldType::Mac => ("mac", None),
            FieldType::Number => ("number", None),
            FieldType::Enum(values) => ("enum", Some(values.clone())),
        };
        FieldDefDto {
            key: field.key.clone(),
            label: field.label.clone(),
            ty,
            values,
            ops: field.ops.iter().map(op_dto).collect(),
        }
    }
}

/// One row in a `GET /api/emissions` response (Task 6.3). A thin, explicit
/// projection of `fluxfang_db::models::Emission` rather than a re-export:
/// `Emission` already `#[derive(Serialize)]`s with a wire-compatible shape,
/// but this DTO exists so the emissions API's response shape is controlled
/// here rather than coupled to however the DB model happens to be laid out
/// (e.g. it deliberately omits `created_at`, which isn't part of the brief's
/// response shape).
#[derive(Debug, Clone, Serialize)]
pub struct EmissionDto {
    pub id: Uuid,
    pub data_source_id: Option<Uuid>,
    pub emitter_id: Option<Uuid>,
    pub session_id: Option<Uuid>,
    pub observed_at: DateTime<Utc>,
    pub signal_strength: Option<i32>,
    /// Longitude/latitude decoded from `Emission::lon`/`lat` (themselves
    /// projected from PostGIS `location` — see `fluxfang_db::models`'
    /// module docs). `None` for both when the emission has no location.
    pub lon: Option<f64>,
    pub lat: Option<f64>,
    pub kind: String,
    pub payload: serde_json::Value,
}

impl From<&Emission> for EmissionDto {
    fn from(e: &Emission) -> Self {
        EmissionDto {
            id: e.id,
            data_source_id: e.data_source_id,
            emitter_id: e.emitter_id,
            session_id: e.session_id,
            observed_at: e.observed_at,
            signal_strength: e.signal_strength,
            lon: e.lon,
            lat: e.lat,
            kind: e.kind.clone(),
            payload: e.payload.clone(),
        }
    }
}

/// One row in a `GET /api/emitters`/`GET /api/emitters/:id` response (Task
/// 6.4, extended Phase A5). Same "explicit DTO, not a re-export" rationale
/// as [`EmissionDto`]: `fluxfang_db::models::Emitter` already
/// `#[derive(Serialize)]`s with a wire-compatible shape (its `#[sqlx(rename
/// = "type")]` dance only affects the SQL column mapping, not `serde`), but
/// this DTO keeps the emitters API's response shape independently
/// controlled here rather than coupled to the DB row's exact field set.
///
/// ## Phase A5 additions: `emitter_type`/`attributes`/`match_enabled` +
/// derived `type_label`/`category`
///
/// `emitter_type`, `attributes`, and `match_enabled` are plain passthroughs
/// of the DB row's Phase A1 classification columns. `type_label` and
/// `category` are *derived on every read* rather than stored: this
/// resolves the Phase A4 "`type_` snapshot" concern (an auto-created
/// emitter's free-text `type_` field was a point-in-time label render that
/// could go stale if the classification registry's labels ever changed) by
/// always recomputing them from the current `emitter_type` via
/// `fluxfang_core::{emitter_type_label, emitter_category}`:
/// - `type_label`: `emitter_type_label(&t)` when `emitter_type` is `Some`
///   (e.g. `"wifi_access_point"` -> `"WiFi Access Point"`); otherwise falls
///   back to the stored free-text `type_` field (or `"Emitter"` if that's
///   also absent, since `type_label` itself is not optional on the wire).
/// - `category`: `emitter_category(&t)` when `emitter_type` is `Some`;
///   `None` for a plain user-made emitter (no classification to group by).
#[derive(Debug, Clone, Serialize)]
pub struct EmitterDto {
    pub id: Uuid,
    pub name: String,
    #[serde(rename = "type")]
    pub type_: Option<String>,
    pub entity_id: Option<Uuid>,
    pub match_criteria: serde_json::Value,
    pub first_seen_at: Option<DateTime<Utc>>,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub emitter_type: Option<String>,
    pub attributes: serde_json::Value,
    pub match_enabled: bool,
    /// Human-readable label, derived on read (see struct docs) — never
    /// stored, so it can never go stale relative to the classification
    /// registry.
    pub type_label: String,
    /// Grouping key for map/UI category layers (e.g. `"wifi"`), derived on
    /// read from `emitter_type`. `None` for a plain, unclassified emitter.
    pub category: Option<String>,
}

impl From<&Emitter> for EmitterDto {
    fn from(e: &Emitter) -> Self {
        let (type_label, category) = match &e.emitter_type {
            Some(t) => (
                fluxfang_core::emitter_type_label(t).to_string(),
                Some(fluxfang_core::emitter_category(t).to_string()),
            ),
            None => (
                e.type_.clone().unwrap_or_else(|| "Emitter".to_string()),
                None,
            ),
        };

        EmitterDto {
            id: e.id,
            name: e.name.clone(),
            type_: e.type_.clone(),
            entity_id: e.entity_id,
            match_criteria: e.match_criteria.clone(),
            first_seen_at: e.first_seen_at,
            last_seen_at: e.last_seen_at,
            created_at: e.created_at,
            emitter_type: e.emitter_type.clone(),
            attributes: e.attributes.clone(),
            match_enabled: e.match_enabled,
            type_label,
            category,
        }
    }
}

/// One row in `GET /api/entities`/`POST /api/entities`/`PATCH
/// /api/entities/:id`, and the base fields `GET /api/entities/:id`'s detail
/// response (see `entities::EntityDetailDto`) builds on top of (Task 6.5).
/// Same "explicit DTO, not a re-export" rationale as [`EmissionDto`]/
/// [`EmitterDto`].
///
/// Deliberately excludes `last_seen`: for a *list* of entities, computing it
/// per row would mean either N extra `EntityRepo::last_seen` calls (one per
/// entity) or a broader join/aggregate query this task doesn't otherwise
/// need. The task brief explicitly allows omitting it from the list in that
/// case, so it's provided only on the single-entity detail endpoint, where
/// it costs exactly one extra query regardless of how many emitters the
/// entity has.
#[derive(Debug, Clone, Serialize)]
pub struct EntityDto {
    pub id: Uuid,
    pub name: String,
    pub notes: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl From<&Entity> for EntityDto {
    fn from(e: &Entity) -> Self {
        EntityDto {
            id: e.id,
            name: e.name.clone(),
            notes: e.notes.clone(),
            created_at: e.created_at,
        }
    }
}

/// One row in `GET /api/zones`/`POST /api/zones`/`PATCH /api/zones/:id`, and
/// the base fields `GET /api/zones/:id`'s detail response (see
/// `zones::ZoneDetailDto`) builds on top of (Task 6.7). Same "explicit DTO,
/// not a re-export" rationale as [`EmissionDto`]/[`EmitterDto`]/
/// [`EntityDto`]: `fluxfang_db::models::Zone` already `#[derive(Serialize)]`s
/// with a wire-compatible shape, but this DTO keeps the zones API's response
/// shape independently controlled here. `lon`/`lat` are flattened onto the
/// top level (mirroring `Zone`'s own shape) rather than nested under a
/// `center` object, matching every other geo-bearing DTO in this module
/// (`EmissionDto`).
#[derive(Debug, Clone, Serialize)]
pub struct ZoneDto {
    pub id: Uuid,
    pub name: String,
    pub lon: f64,
    pub lat: f64,
    pub radius_m: f64,
    pub notes: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// `GET /api/system/capture-devices` response: the enumerated hardware the
/// WebUI offers in its data-source setup dropdowns instead of making the
/// operator type an interface/device name from memory. Built directly from
/// `fluxfang_capture::enumerate::{list_wifi_interfaces, list_serial_devices}`
/// — see that module's docs for detection strategy and the "never panics,
/// empty on a hardware-less host" guarantee this DTO relies on.
#[derive(Debug, Clone, Serialize)]
pub struct CaptureDevicesDto {
    pub wifi_interfaces: Vec<String>,
    pub serial_devices: Vec<String>,
}

impl From<&Zone> for ZoneDto {
    fn from(z: &Zone) -> Self {
        ZoneDto {
            id: z.id,
            name: z.name.clone(),
            lon: z.lon,
            lat: z.lat,
            radius_m: z.radius_m,
            notes: z.notes.clone(),
            created_at: z.created_at,
        }
    }
}
