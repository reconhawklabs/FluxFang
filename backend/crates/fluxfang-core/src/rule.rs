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
}
