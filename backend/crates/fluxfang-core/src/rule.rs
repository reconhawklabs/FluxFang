//! The one shared rule/condition format, reused by emitter matching (later
//! task), emission filtering, and alert content-matching. Pure data types
//! only — no evaluation logic here (see Task 3.2's `eval`).

use serde::{Deserialize, Serialize};

/// A comparison operator usable in a [`Condition`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Op {
    Eq,
    Neq,
    Matches,
    #[serde(rename = "in")]
    In,
    Gte,
    Lte,
}

/// A single field/operator/value comparison.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Condition {
    pub field: String,
    pub op: Op,
    pub value: serde_json::Value,
}

/// How a [`Rule`]'s conditions combine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MatchMode {
    All,
    Any,
}

/// A rule: a set of conditions combined by `match_mode`.
///
/// JSON shape: `{"match": "all", "conditions": [...]}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    #[serde(rename = "match")]
    pub match_mode: MatchMode,
    pub conditions: Vec<Condition>,
}

/// Evaluate a [`Rule`] against a JSON payload (a JSON object, e.g.
/// `{"bssid": "aa:bb:...", "channel": 6}`).
///
/// NOTE (not JS/Python `eval`): this `eval` performs pure, in-memory
/// structural matching of `payload` against `rule`'s conditions. It never
/// parses or executes code — `Condition::value` is only ever compared,
/// stringified, or (for `Matches`) compiled as a data-only regex pattern.
///
/// `match_mode` decides how per-condition results combine:
/// - [`MatchMode::All`]: every condition must pass (logical AND). An empty
///   `conditions` list has no condition that can fail, so this returns
///   `true` (vacuous truth).
/// - [`MatchMode::Any`]: at least one condition must pass (logical OR). An
///   empty `conditions` list has no condition that can succeed, so this
///   returns `false`.
///
/// For each condition, `payload[field]` is looked up first. If the field is
/// absent, the condition is `false` for *every* operator, including `Neq` —
/// `eval` treats "field not present" as "cannot be evaluated", not as an
/// implicit mismatch to exploit for `Neq`. This keeps the semantics simple:
/// a condition only ever passes when the field is present and the
/// comparison holds.
pub fn eval(rule: &Rule, payload: &serde_json::Value) -> bool {
    match rule.match_mode {
        MatchMode::All => rule.conditions.iter().all(|c| eval_condition(c, payload)),
        MatchMode::Any => rule.conditions.iter().any(|c| eval_condition(c, payload)),
    }
}

fn eval_condition(condition: &Condition, payload: &serde_json::Value) -> bool {
    let Some(field_value) = payload.get(&condition.field) else {
        return false;
    };

    match condition.op {
        Op::Eq => field_value == &condition.value,
        Op::Neq => field_value != &condition.value,
        Op::Matches => eval_matches(field_value, &condition.value),
        Op::In => eval_in(field_value, &condition.value),
        Op::Gte => eval_numeric(field_value, &condition.value, |a, b| a >= b),
        Op::Lte => eval_numeric(field_value, &condition.value, |a, b| a <= b),
    }
}

/// `condition.value` is a regex pattern string; `field_value` is coerced to
/// its string form (JSON string -> the string itself; number -> textual
/// form; anything else -> not matched). An invalid regex pattern is treated
/// as "does not match" rather than panicking.
fn eval_matches(field_value: &serde_json::Value, pattern: &serde_json::Value) -> bool {
    let Some(pattern) = pattern.as_str() else {
        return false;
    };
    let Some(haystack) = json_value_as_match_string(field_value) else {
        return false;
    };
    match regex::Regex::new(pattern) {
        Ok(re) => re.is_match(&haystack),
        Err(_) => false,
    }
}

fn json_value_as_match_string(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// `condition.value` must be a JSON array; true if `field_value` equals any
/// element of it (a non-array `condition.value` is always `false`).
fn eval_in(field_value: &serde_json::Value, haystack: &serde_json::Value) -> bool {
    match haystack.as_array() {
        Some(items) => items.iter().any(|item| item == field_value),
        None => false,
    }
}

/// Coerce both sides to `f64` and compare with `cmp`; non-numeric JSON
/// values on either side make the condition `false`.
fn eval_numeric(
    field_value: &serde_json::Value,
    other: &serde_json::Value,
    cmp: impl Fn(f64, f64) -> bool,
) -> bool {
    match (field_value.as_f64(), other.as_f64()) {
        (Some(a), Some(b)) => cmp(a, b),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_round_trips_through_json_with_match_shape() {
        let rule = Rule {
            match_mode: MatchMode::All,
            conditions: vec![Condition {
                field: "ssid".to_string(),
                op: Op::Eq,
                value: serde_json::json!("home-network"),
            }],
        };

        let json = serde_json::to_value(&rule).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "match": "all",
                "conditions": [
                    {"field": "ssid", "op": "eq", "value": "home-network"}
                ]
            })
        );

        let round_tripped: Rule = serde_json::from_value(json).unwrap();
        assert_eq!(round_tripped, rule);
    }

    #[test]
    fn op_in_serializes_as_bare_in() {
        let json = serde_json::to_value(Op::In).unwrap();
        assert_eq!(json, serde_json::json!("in"));

        let op: Op = serde_json::from_value(serde_json::json!("in")).unwrap();
        assert_eq!(op, Op::In);
    }

    #[test]
    fn eval_all_matches_bssid_eq() {
        let rule: Rule = serde_json::from_str(
            r#"{"match":"all","conditions":[{"field":"bssid","op":"eq","value":"aa:bb:cc:dd:ee:ff"}]}"#).unwrap();
        let p = serde_json::json!({"bssid":"aa:bb:cc:dd:ee:ff","channel":6});
        assert!(eval(&rule, &p));
        let p2 = serde_json::json!({"bssid":"00:00:00:00:00:00"});
        assert!(!eval(&rule, &p2));
    }

    #[test]
    fn eval_any_with_matches_regex_and_gte() {
        let rule: Rule = serde_json::from_str(
            r#"{"match":"any","conditions":[
                {"field":"ssid","op":"matches","value":"^Free"},
                {"field":"channel","op":"gte","value":11}]}"#,
        )
        .unwrap();
        assert!(eval(
            &rule,
            &serde_json::json!({"ssid":"FreeWifi","channel":1})
        ));
        assert!(eval(
            &rule,
            &serde_json::json!({"ssid":"Home","channel":11})
        ));
        assert!(!eval(
            &rule,
            &serde_json::json!({"ssid":"Home","channel":1})
        ));
    }

    #[test]
    fn eval_neq_true_when_different_false_when_same() {
        let rule: Rule = serde_json::from_str(
            r#"{"match":"all","conditions":[{"field":"ssid","op":"neq","value":"Home"}]}"#,
        )
        .unwrap();
        assert!(eval(&rule, &serde_json::json!({"ssid": "Office"})));
        assert!(!eval(&rule, &serde_json::json!({"ssid": "Home"})));
    }

    #[test]
    fn eval_in_checks_membership_in_json_array() {
        let rule: Rule = serde_json::from_str(
            r#"{"match":"all","conditions":[{"field":"channel","op":"in","value":[1,6,11]}]}"#,
        )
        .unwrap();
        assert!(eval(&rule, &serde_json::json!({"channel": 6})));
        assert!(!eval(&rule, &serde_json::json!({"channel": 7})));
    }

    #[test]
    fn eval_in_condition_value_not_array_is_false() {
        let rule: Rule = serde_json::from_str(
            r#"{"match":"all","conditions":[{"field":"channel","op":"in","value":6}]}"#,
        )
        .unwrap();
        assert!(!eval(&rule, &serde_json::json!({"channel": 6})));
    }

    #[test]
    fn eval_lte_numeric_comparison() {
        let rule: Rule = serde_json::from_str(
            r#"{"match":"all","conditions":[{"field":"channel","op":"lte","value":6}]}"#,
        )
        .unwrap();
        assert!(eval(&rule, &serde_json::json!({"channel": 6})));
        assert!(eval(&rule, &serde_json::json!({"channel": 1})));
        assert!(!eval(&rule, &serde_json::json!({"channel": 7})));
    }

    #[test]
    fn eval_gte_lte_non_numeric_is_false() {
        let gte_rule: Rule = serde_json::from_str(
            r#"{"match":"all","conditions":[{"field":"channel","op":"gte","value":1}]}"#,
        )
        .unwrap();
        assert!(!eval(&gte_rule, &serde_json::json!({"channel": "six"})));

        let lte_rule: Rule = serde_json::from_str(
            r#"{"match":"all","conditions":[{"field":"channel","op":"lte","value":"nope"}]}"#,
        )
        .unwrap();
        assert!(!eval(&lte_rule, &serde_json::json!({"channel": 1})));
    }

    #[test]
    fn eval_missing_field_is_false_for_every_op_including_neq() {
        let ops = ["eq", "neq", "matches", "in", "gte", "lte"];
        for op in ops {
            let rule: Rule = serde_json::from_str(&format!(
                r#"{{"match":"all","conditions":[{{"field":"missing","op":"{op}","value":"x"}}]}}"#
            ))
            .unwrap();
            assert!(
                !eval(&rule, &serde_json::json!({"other": "y"})),
                "op {op} should be false when field is missing"
            );
        }
    }

    #[test]
    fn eval_matches_invalid_regex_is_false_not_panic() {
        let rule: Rule = serde_json::from_str(
            r#"{"match":"all","conditions":[{"field":"ssid","op":"matches","value":"("}]}"#,
        )
        .unwrap();
        assert!(!eval(&rule, &serde_json::json!({"ssid": "anything"})));
    }

    #[test]
    fn eval_empty_conditions_all_true_any_false() {
        let all_rule = Rule {
            match_mode: MatchMode::All,
            conditions: vec![],
        };
        let any_rule = Rule {
            match_mode: MatchMode::Any,
            conditions: vec![],
        };
        let p = serde_json::json!({});
        assert!(eval(&all_rule, &p));
        assert!(!eval(&any_rule, &p));
    }

    #[test]
    fn eval_matches_coerces_numeric_payload_to_string() {
        let rule: Rule = serde_json::from_str(
            r#"{"match":"all","conditions":[{"field":"channel","op":"matches","value":"^6$"}]}"#,
        )
        .unwrap();
        assert!(eval(&rule, &serde_json::json!({"channel": 6})));
        assert!(!eval(&rule, &serde_json::json!({"channel": 16})));
    }
}
