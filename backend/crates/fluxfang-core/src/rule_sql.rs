//! Rule → SQL translator: turns [`Condition`]s into a parameterized SQL
//! `WHERE`-fragment over the `emission.payload` JSONB column, for use by
//! `EmissionRepo::query` (emission filtering) and the emissions API.
//!
//! This module is pure (no DB connection, no I/O) — it only builds a SQL
//! string plus an ordered list of bind values. The caller (sqlx, in the DB
//! layer) is responsible for actually binding those values and running the
//! query.
//!
//! # Bind representation and agreement with `eval`
//!
//! `payload->>'field'` always yields **TEXT** in Postgres (the `->>`
//! operator), regardless of what type the JSON value underneath actually
//! is. Every bind value is coerced to its canonical **text** form and
//! returned as `serde_json::Value::String(..)`:
//!
//! - a JSON string binds as itself (`"home"` -> `"home"`)
//! - a JSON number binds as its canonical decimal text (`6` -> `"6"`,
//!   `1.5` -> `"1.5"`) — this matches `eval`'s numeric ops, which parse
//!   both sides as `f64` before comparing.
//! - a JSON bool binds as `"true"`/`"false"`.
//!
//! This text-bind representation on its own does **not** guarantee the SQL
//! predicate agrees with [`crate::rule::eval`] for every input — `eval` is
//! typed (it compares/parses the *JSON* value), while the SQL side compares
//! *text*. The two translation functions close that gap to different
//! degrees:
//!
//! - [`Op::Gte`]/[`Op::Lte`]/[`Op::Matches`] are guarded in *both*
//!   [`conditions_to_sql`] and [`conditions_to_sql_checked`]: exactly like
//!   `eval_numeric`/`eval_matches`, a `condition.value` of the wrong JSON
//!   type (non-number for `Gte`/`Lte`, non-string for `Matches`) is never
//!   text-bound into a live comparison — it becomes a guaranteed-false
//!   `FALSE` clause instead.
//! - [`Op::Eq`]/[`Op::Neq`] have **no such guard in [`conditions_to_sql`]**:
//!   because `->>` always yields text, a text-bind of e.g. the *number* `6`
//!   (`"6"`) and the *string* `"6"` are indistinguishable to SQL, so an
//!   `Eq`/`Neq` condition can text-match a value whose JSON type wouldn't
//!   have satisfied `eval`'s typed `==`/`!=`. This divergence is closed
//!   only by [`conditions_to_sql_checked`], which validates each
//!   `condition.value`'s JSON type against the matched field's
//!   [`crate::catalog::FieldType`] (see below) and rejects a type mismatch
//!   with [`RuleSqlError::InvalidValueType`] rather than translating it.
//!
//! For [`Op::Gte`]/[`Op::Lte`] the SQL casts the *column* side to
//! `numeric` (`(payload->>'field')::numeric >= $N`); Postgres then infers
//! the parameter's type from that context and parses the bound text back
//! into a `numeric`, so `'6'` compared as numeric still behaves like the
//! number `6`. This is exactly the sketch given in the task brief:
//! `(payload->>'channel')::numeric >= $2`, no cast needed on the bind
//! itself.
//!
//! # Injection safety
//!
//! `condition.value` is **never** interpolated into the SQL string — it is
//! always returned as a bind value for the caller to parameterize.
//!
//! `condition.field` *is* interpolated (Postgres has no way to
//! parameterize a JSON path key), so it is validated before use:
//!
//! - [`conditions_to_sql`] (the brief's exact signature) validates the
//!   field name against the character allow-list `^[a-z0-9_]+$`. This is
//!   sufficient to rule out SQL injection through the field name (no quotes,
//!   operators, whitespace, or comment sequences are possible), but it
//!   can't check that the field is a *real*, catalog-known field for the
//!   emission's `kind` — the brief's signature has no catalog parameter.
//!   A condition whose field fails the allow-list becomes a literal
//!   `FALSE` (no bind consumed), so translation never errors or panics —
//!   it just makes that one condition unsatisfiable.
//! - [`conditions_to_sql_checked`] additionally takes the `kind`'s
//!   catalog (`&[FieldDef]`, from [`crate::catalog::catalog_for`]) and
//!   is the *type-safe boundary*: it returns `Err(RuleSqlError::UnknownField)`
//!   for any field not present in the catalog (also re-checking the
//!   allow-list, so a hypothetical unsafe catalog key can't reach
//!   interpolation either), and `Err(RuleSqlError::InvalidValueType)` when
//!   `condition.value` (or, for `Op::In`, any array element) doesn't match
//!   the field's declared `FieldType` (JSON number for `FieldType::Number`;
//!   JSON string for `Text`/`Mac`/`Enum`). This is the variant real callers
//!   (query building, the emissions API) should use, since it both rejects
//!   rules referencing fields the data source doesn't expose *and* closes
//!   the `Eq`/`Neq` text-vs-typed divergence described above — the
//!   unchecked [`conditions_to_sql`] alone does not.
//!
//! Operators and the `payload->>'...'`/`::numeric` SQL fragments used per
//! [`Op`] are fixed, hard-coded strings selected from the closed `Op` enum
//! — never derived from user input.

use crate::catalog::FieldDef;
use crate::rule::{Condition, MatchMode, Op};
use serde_json::Value;
use std::fmt;

/// Error returned by [`conditions_to_sql_checked`] when a condition
/// references a field that isn't in the supplied catalog, or supplies a
/// `value` whose JSON type doesn't match that field's [`FieldType`].
#[derive(Debug, Clone, PartialEq)]
pub enum RuleSqlError {
    UnknownField(String),
    /// `condition.value` (or, for `Op::In`, one of its array elements)
    /// isn't the JSON type expected by the field's `FieldType` (JSON
    /// number for `FieldType::Number`, JSON string for `Text`/`Mac`/`Enum`).
    InvalidValueType { field: String, expected: &'static str },
}

impl fmt::Display for RuleSqlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RuleSqlError::UnknownField(field) => {
                write!(f, "unknown or invalid field: {field:?}")
            }
            RuleSqlError::InvalidValueType { field, expected } => {
                write!(
                    f,
                    "invalid value type for field {field:?}: expected a JSON {expected}"
                )
            }
        }
    }
}

impl std::error::Error for RuleSqlError {}

/// Translate `conds` into a parameterized SQL boolean expression over the
/// `payload` JSONB column, combined by `mode` and wrapped in parentheses.
///
/// `next_bind` is the first positional-parameter index to use (`$next_bind`,
/// `$next_bind + 1`, ...) — callers building a larger query with earlier
/// parameters already bound should pass one past their last used index.
///
/// Returns the SQL fragment and the ordered bind values (see the module docs
/// for the bind representation). Field names are validated against
/// `^[a-z0-9_]+$` (see module docs on injection safety); a condition with an
/// invalid field name translates to a literal `FALSE` and consumes no bind.
///
/// Prefer [`conditions_to_sql_checked`] when a catalog is available — it
/// additionally rejects fields unknown to the catalog instead of silently
/// making them unsatisfiable.
pub fn conditions_to_sql(
    conds: &[Condition],
    mode: MatchMode,
    next_bind: usize,
) -> (String, Vec<Value>) {
    build(conds, mode, next_bind, None)
}

/// Like [`conditions_to_sql`], but additionally validates each
/// `condition.field` against `catalog` (e.g. `catalog_for("wifi")`).
///
/// Returns `Err(RuleSqlError::UnknownField)` on the first condition whose
/// field is not present in `catalog` (which also enforces the
/// `^[a-z0-9_]+$` allow-list, since catalog keys are all drawn from that
/// alphabet).
pub fn conditions_to_sql_checked(
    conds: &[Condition],
    mode: MatchMode,
    next_bind: usize,
    catalog: &[FieldDef],
) -> Result<(String, Vec<Value>), RuleSqlError> {
    for c in conds {
        // Belt-and-suspenders (Fix 4): require the character allow-list
        // *and* catalog membership, so a hypothetical future catalog
        // containing an unsafe key still can't reach string interpolation.
        if !is_safe_field_name(&c.field) {
            return Err(RuleSqlError::UnknownField(c.field.clone()));
        }
        let Some(field_def) = catalog.iter().find(|f| f.key == c.field) else {
            return Err(RuleSqlError::UnknownField(c.field.clone()));
        };

        // Fix 3: validate condition.value's JSON type against the field's
        // FieldType, closing the Eq/Neq (and Gte/Lte/In) gap where SQL
        // would otherwise text-bind a value `eval` would reject outright
        // for having the wrong JSON type.
        let type_ok = match c.op {
            Op::In => match c.value.as_array() {
                Some(items) => items.iter().all(|item| value_matches_type(item, &field_def.ty)),
                None => false,
            },
            _ => value_matches_type(&c.value, &field_def.ty),
        };
        if !type_ok {
            return Err(RuleSqlError::InvalidValueType {
                field: c.field.clone(),
                expected: expected_type_name(&field_def.ty),
            });
        }
    }
    Ok(build(conds, mode, next_bind, Some(catalog)))
}

/// Does `value`'s JSON type match what `ty` expects a `condition.value` (or
/// `Op::In` array element) to be?
fn value_matches_type(value: &Value, ty: &crate::catalog::FieldType) -> bool {
    use crate::catalog::FieldType;
    match ty {
        FieldType::Number => value.is_number(),
        FieldType::Text | FieldType::Mac | FieldType::Enum(_) => value.is_string(),
    }
}

fn expected_type_name(ty: &crate::catalog::FieldType) -> &'static str {
    use crate::catalog::FieldType;
    match ty {
        FieldType::Number => "number",
        FieldType::Text | FieldType::Mac | FieldType::Enum(_) => "string",
    }
}

fn build(
    conds: &[Condition],
    mode: MatchMode,
    next_bind: usize,
    catalog: Option<&[FieldDef]>,
) -> (String, Vec<Value>) {
    if conds.is_empty() {
        // Mirrors `eval`'s vacuous-truth semantics: an empty `All` has
        // nothing that can fail, an empty `Any` has nothing that can pass.
        let literal = match mode {
            MatchMode::All => "TRUE",
            MatchMode::Any => "FALSE",
        };
        return (literal.to_string(), Vec::new());
    }

    let mut binds: Vec<Value> = Vec::new();
    let mut bind_idx = next_bind;
    let mut clauses: Vec<String> = Vec::new();

    for c in conds {
        // If validated against a catalog already (conditions_to_sql_checked),
        // the field is known-safe; otherwise fall back to the character
        // allow-list. Either way, unsafe/unknown fields never reach string
        // interpolation below.
        let field_is_safe = match catalog {
            Some(cat) => cat.iter().any(|f| f.key == c.field),
            None => is_safe_field_name(&c.field),
        };

        if !field_is_safe {
            clauses.push("FALSE".to_string());
            continue;
        }

        let (clause, mut condition_binds) = condition_clause(c, &mut bind_idx);
        clauses.push(clause);
        binds.append(&mut condition_binds);
    }

    let joiner = match mode {
        MatchMode::All => " AND ",
        MatchMode::Any => " OR ",
    };
    let sql = format!("({})", clauses.join(joiner));
    (sql, binds)
}

/// Field keys must be a plain lowercase-alnum-and-underscore token: this is
/// the only thing ever interpolated (never quoted/escaped) into
/// `payload->>'<field>'`, so it must never be able to contain a quote,
/// whitespace, or SQL syntax.
fn is_safe_field_name(field: &str) -> bool {
    !field.is_empty()
        && field
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// Build the SQL clause + binds for a single condition. `bind_idx` is the
/// next positional-parameter index to hand out; it is advanced by however
/// many params this condition consumes.
fn condition_clause(condition: &Condition, bind_idx: &mut usize) -> (String, Vec<Value>) {
    let path = format!("payload->>'{}'", condition.field);

    match condition.op {
        Op::Eq => {
            let bind = next_param(bind_idx);
            (
                format!("{path} = {bind}"),
                vec![text_bind(&condition.value)],
            )
        }
        Op::Neq => {
            let bind = next_param(bind_idx);
            (
                format!("{path} <> {bind}"),
                vec![text_bind(&condition.value)],
            )
        }
        Op::Matches => {
            // Mirrors `eval_matches`: a non-string `condition.value` (the
            // regex pattern) is never a match, for any field value.
            if !condition.value.is_string() {
                return ("FALSE".to_string(), Vec::new());
            }
            let bind = next_param(bind_idx);
            (
                format!("{path} ~ {bind}"),
                vec![text_bind(&condition.value)],
            )
        }
        Op::Gte => {
            // Mirrors `eval_numeric`: a non-number `condition.value` can
            // never satisfy a numeric comparison, regardless of what
            // Postgres's `::numeric` cast would otherwise accept as text.
            if !condition.value.is_number() {
                return ("FALSE".to_string(), Vec::new());
            }
            let bind = next_param(bind_idx);
            (
                format!("({path})::numeric >= {bind}"),
                vec![text_bind(&condition.value)],
            )
        }
        Op::Lte => {
            if !condition.value.is_number() {
                return ("FALSE".to_string(), Vec::new());
            }
            let bind = next_param(bind_idx);
            (
                format!("({path})::numeric <= {bind}"),
                vec![text_bind(&condition.value)],
            )
        }
        Op::In => match condition.value.as_array() {
            Some(items) if !items.is_empty() => {
                let mut binds = Vec::with_capacity(items.len());
                let mut params = Vec::with_capacity(items.len());
                for item in items {
                    params.push(next_param(bind_idx));
                    binds.push(text_bind(item));
                }
                (format!("{path} IN ({})", params.join(", ")), binds)
            }
            // Non-array (or empty-array) `value` can never match anything,
            // mirroring `eval_in`'s "not an array -> false" rule (and an
            // empty array is vacuously "member of nothing").
            _ => ("FALSE".to_string(), Vec::new()),
        },
    }
}

fn next_param(bind_idx: &mut usize) -> String {
    let p = format!("${bind_idx}");
    *bind_idx += 1;
    p
}

/// Coerce a JSON value to the text form `payload->>'field'` (also text)
/// should be compared against, matching `eval`'s typed comparisons:
/// - string -> itself
/// - number -> canonical decimal text (`serde_json::Number`'s `Display`)
/// - bool -> `"true"`/`"false"`
/// - null / arrays / objects -> empty string (never meaningfully equal to
///   any real `payload->>'field'` text, so this behaves like a no-match,
///   same spirit as `eval`'s "not a comparable scalar" cases)
fn text_bind(value: &Value) -> Value {
    let text = match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null | Value::Array(_) | Value::Object(_) => String::new(),
    };
    Value::String(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::catalog_for;
    use serde_json::json;

    #[test]
    fn translates_eq_and_numeric_gte() {
        let conds = vec![
            Condition {
                field: "bssid".into(),
                op: Op::Eq,
                value: json!("aa:bb:cc:dd:ee:ff"),
            },
            Condition {
                field: "channel".into(),
                op: Op::Gte,
                value: json!(6),
            },
        ];
        let (sql, binds) = conditions_to_sql(&conds, MatchMode::All, 1);
        assert_eq!(
            sql,
            "(payload->>'bssid' = $1 AND (payload->>'channel')::numeric >= $2)"
        );
        assert_eq!(binds.len(), 2);
        assert_eq!(binds[0], json!("aa:bb:cc:dd:ee:ff"));
        assert_eq!(binds[1], json!("6"));
    }

    #[test]
    fn translates_matches_as_posix_regex_operator() {
        let conds = vec![Condition {
            field: "ssid".into(),
            op: Op::Matches,
            value: json!("^Free"),
        }];
        let (sql, binds) = conditions_to_sql(&conds, MatchMode::All, 1);
        assert_eq!(sql, "(payload->>'ssid' ~ $1)");
        assert_eq!(binds, vec![json!("^Free")]);
    }

    #[test]
    fn translates_in_by_expanding_to_in_list_over_multiple_binds() {
        let conds = vec![Condition {
            field: "channel".into(),
            op: Op::In,
            value: json!([1, 6, 11]),
        }];
        let (sql, binds) = conditions_to_sql(&conds, MatchMode::All, 1);
        assert_eq!(sql, "(payload->>'channel' IN ($1, $2, $3))");
        assert_eq!(binds, vec![json!("1"), json!("6"), json!("11")]);
    }

    #[test]
    fn in_with_non_array_value_is_guaranteed_false_and_binds_nothing() {
        let conds = vec![Condition {
            field: "channel".into(),
            op: Op::In,
            value: json!(6),
        }];
        let (sql, binds) = conditions_to_sql(&conds, MatchMode::All, 1);
        assert_eq!(sql, "(FALSE)");
        assert!(binds.is_empty());
    }

    #[test]
    fn translates_neq() {
        let conds = vec![Condition {
            field: "ssid".into(),
            op: Op::Neq,
            value: json!("Home"),
        }];
        let (sql, binds) = conditions_to_sql(&conds, MatchMode::All, 1);
        assert_eq!(sql, "(payload->>'ssid' <> $1)");
        assert_eq!(binds, vec![json!("Home")]);
    }

    #[test]
    fn any_mode_joins_with_or() {
        let conds = vec![
            Condition {
                field: "ssid".into(),
                op: Op::Matches,
                value: json!("^Free"),
            },
            Condition {
                field: "channel".into(),
                op: Op::Gte,
                value: json!(11),
            },
        ];
        let (sql, _binds) = conditions_to_sql(&conds, MatchMode::Any, 1);
        assert_eq!(
            sql,
            "(payload->>'ssid' ~ $1 OR (payload->>'channel')::numeric >= $2)"
        );
    }

    #[test]
    fn next_bind_offset_continues_numbering_from_given_start() {
        let conds = vec![
            Condition {
                field: "bssid".into(),
                op: Op::Eq,
                value: json!("aa:bb:cc:dd:ee:ff"),
            },
            Condition {
                field: "channel".into(),
                op: Op::Lte,
                value: json!(11),
            },
        ];
        // Simulates a caller (e.g. EmissionRepo::query) that already bound
        // 4 earlier params (kind, since, etc.) before appending rule conds.
        let (sql, binds) = conditions_to_sql(&conds, MatchMode::All, 5);
        assert_eq!(
            sql,
            "(payload->>'bssid' = $5 AND (payload->>'channel')::numeric <= $6)"
        );
        assert_eq!(binds.len(), 2);
    }

    #[test]
    fn unknown_field_is_rejected_by_the_allow_list_as_a_false_clause() {
        // Not alphanumeric/underscore: would otherwise break out of the
        // quoted JSON path (e.g. via `'; DROP TABLE emission; --`).
        let conds = vec![Condition {
            field: "bssid' OR '1'='1".into(),
            op: Op::Eq,
            value: json!("x"),
        }];
        let (sql, binds) = conditions_to_sql(&conds, MatchMode::All, 1);
        assert_eq!(sql, "(FALSE)");
        assert!(binds.is_empty());
    }

    #[test]
    fn unknown_field_is_rejected_with_an_error_when_catalog_is_checked() {
        let conds = vec![Condition {
            field: "not_a_real_field".into(),
            op: Op::Eq,
            value: json!("x"),
        }];
        let err =
            conditions_to_sql_checked(&conds, MatchMode::All, 1, &catalog_for("wifi")).unwrap_err();
        assert_eq!(err, RuleSqlError::UnknownField("not_a_real_field".into()));
    }

    #[test]
    fn checked_variant_accepts_known_catalog_fields() {
        let conds = vec![Condition {
            field: "channel".into(),
            op: Op::Gte,
            value: json!(6),
        }];
        let (sql, binds) =
            conditions_to_sql_checked(&conds, MatchMode::All, 1, &catalog_for("wifi")).unwrap();
        assert_eq!(sql, "((payload->>'channel')::numeric >= $1)");
        assert_eq!(binds, vec![json!("6")]);
    }

    // --- Fix 1: Gte/Lte numeric guard ---
    //
    // These reproduce a real bug found in review: `eval_numeric` (rule.rs)
    // returns false whenever `condition.value` isn't a JSON number, but the
    // pre-fix SQL `Gte`/`Lte` arms text-bound *any* value and relied on
    // Postgres's `::numeric` cast, so e.g. `channel gte "10"` (a JSON
    // *string* "10") produced a live, passing SQL comparison while `eval`
    // said false. Confirmed RED against the pre-fix code (see the task
    // report for the exact failing output) before the guard was added.

    #[test]
    fn gte_with_non_numeric_json_value_is_false_clause_and_binds_nothing() {
        let conds = vec![Condition {
            field: "channel".into(),
            op: Op::Gte,
            value: json!("10"),
        }];
        let (sql, binds) = conditions_to_sql(&conds, MatchMode::All, 1);
        assert_eq!(sql, "(FALSE)");
        assert!(binds.is_empty());
    }

    #[test]
    fn lte_with_non_numeric_json_value_is_false_clause_and_binds_nothing() {
        let conds = vec![Condition {
            field: "channel".into(),
            op: Op::Lte,
            value: json!("10"),
        }];
        let (sql, binds) = conditions_to_sql(&conds, MatchMode::All, 1);
        assert_eq!(sql, "(FALSE)");
        assert!(binds.is_empty());
    }

    // --- Fix 2: Matches string guard ---

    #[test]
    fn matches_with_non_string_json_value_is_false_clause_and_binds_nothing() {
        // `eval_matches` returns false when `condition.value` isn't a JSON
        // string (it's the regex pattern). The SQL `Matches` arm must agree
        // rather than text-binding e.g. the number `6` as the pattern `"6"`.
        let conds = vec![Condition {
            field: "ssid".into(),
            op: Op::Matches,
            value: json!(6),
        }];
        let (sql, binds) = conditions_to_sql(&conds, MatchMode::All, 1);
        assert_eq!(sql, "(FALSE)");
        assert!(binds.is_empty());
    }

    // --- Fix 3: conditions_to_sql_checked is the type-safe boundary ---
    //
    // `channel` is FieldType::Number and `ssid` is FieldType::Text in the
    // wifi catalog (catalog.rs). Confirmed RED for the Number-field/
    // string-value case against the pre-fix code (see the task report).

    #[test]
    fn checked_rejects_number_field_given_string_value() {
        let conds = vec![Condition {
            field: "channel".into(),
            op: Op::Eq,
            value: json!("6"),
        }];
        let err =
            conditions_to_sql_checked(&conds, MatchMode::All, 1, &catalog_for("wifi")).unwrap_err();
        assert_eq!(
            err,
            RuleSqlError::InvalidValueType {
                field: "channel".into(),
                expected: "number",
            }
        );
    }

    #[test]
    fn checked_rejects_text_field_given_number_value() {
        let conds = vec![Condition {
            field: "ssid".into(),
            op: Op::Eq,
            value: json!(6),
        }];
        let err =
            conditions_to_sql_checked(&conds, MatchMode::All, 1, &catalog_for("wifi")).unwrap_err();
        assert_eq!(
            err,
            RuleSqlError::InvalidValueType {
                field: "ssid".into(),
                expected: "string",
            }
        );
    }

    #[test]
    fn checked_rejects_in_with_a_mistyped_element() {
        let conds = vec![Condition {
            field: "channel".into(),
            op: Op::In,
            value: json!([1, "6", 11]),
        }];
        let err =
            conditions_to_sql_checked(&conds, MatchMode::All, 1, &catalog_for("wifi")).unwrap_err();
        assert_eq!(
            err,
            RuleSqlError::InvalidValueType {
                field: "channel".into(),
                expected: "number",
            }
        );
    }

    #[test]
    fn checked_accepts_well_typed_values_including_in() {
        let conds = vec![
            Condition {
                field: "channel".into(),
                op: Op::In,
                value: json!([1, 6, 11]),
            },
            Condition {
                field: "ssid".into(),
                op: Op::Eq,
                value: json!("home-network"),
            },
        ];
        let (sql, binds) =
            conditions_to_sql_checked(&conds, MatchMode::All, 1, &catalog_for("wifi")).unwrap();
        assert_eq!(
            sql,
            "(payload->>'channel' IN ($1, $2, $3) AND payload->>'ssid' = $4)"
        );
        assert_eq!(binds.len(), 4);
    }

    // --- Fix 5: empty-conditions literal (documents current behavior) ---

    #[test]
    fn empty_conditions_all_is_true_any_is_false() {
        assert_eq!(
            conditions_to_sql(&[], MatchMode::All, 1),
            ("TRUE".to_string(), Vec::new())
        );
        assert_eq!(
            conditions_to_sql(&[], MatchMode::Any, 1),
            ("FALSE".to_string(), Vec::new())
        );
    }

    /// Documents that the generated SQL predicate agrees "in spirit" with
    /// `eval()` for a representative rule + payload: `eval`'s `Gte` parses
    /// both sides as numbers and compares numerically; the SQL predicate
    /// casts the JSONB-text column to `numeric` and compares against a
    /// bind that carries the same textual number, so Postgres parses it
    /// back into the same numeric value before comparing. Both paths
    /// reject the comparison if the stored value isn't actually numeric
    /// (`eval` via `as_f64` returning `None`; SQL via the `::numeric` cast
    /// raising/failing on non-numeric text) rather than silently coercing
    /// garbage.
    #[test]
    fn sql_predicate_matches_eval_semantics_for_a_representative_rule() {
        use crate::rule::{eval, Rule};

        let rule = Rule {
            match_mode: MatchMode::All,
            conditions: vec![
                Condition {
                    field: "bssid".into(),
                    op: Op::Eq,
                    value: json!("aa:bb:cc:dd:ee:ff"),
                },
                Condition {
                    field: "channel".into(),
                    op: Op::Gte,
                    value: json!(6),
                },
            ],
        };
        let matching_payload = json!({"bssid": "aa:bb:cc:dd:ee:ff", "channel": 11});
        let non_matching_payload = json!({"bssid": "aa:bb:cc:dd:ee:ff", "channel": 1});
        assert!(eval(&rule, &matching_payload));
        assert!(!eval(&rule, &non_matching_payload));

        let (sql, binds) = conditions_to_sql(&rule.conditions, rule.match_mode, 1);
        // Eq binds the field's text form directly (same bytes `eval` would
        // compare via JSON string equality: "aa:bb:cc:dd:ee:ff").
        assert_eq!(binds[0], json!("aa:bb:cc:dd:ee:ff"));
        // Gte casts the column to numeric and binds the number's canonical
        // text ("6"), so `11 >= 6` and `1 >= 6` resolve the same way
        // numerically in SQL as `eval`'s `f64` comparison does.
        assert_eq!(binds[1], json!("6"));
        assert!(sql.contains("(payload->>'channel')::numeric >= $2"));
    }
}
