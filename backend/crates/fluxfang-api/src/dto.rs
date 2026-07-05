//! Wire DTOs — types that control the exact JSON shape the API returns,
//! kept separate from `fluxfang-core`'s domain types so the wire format can
//! evolve independently of internal representations (and so a core struct's
//! default `serde` derive never leaks onto the wire by accident; see
//! [`FieldDefDto`]'s docs).

use serde::Serialize;

use fluxfang_core::catalog::{FieldDef, FieldType};
use fluxfang_core::rule::Op;

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
