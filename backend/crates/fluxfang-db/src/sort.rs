//! Safe, allow-listed `ORDER BY` resolution shared by the list repos. A
//! caller-supplied `sort`/`dir` is validated against a fixed allow-list of
//! `(public key -> SQL ordering expression)` pairs; the raw `sort` value is
//! never interpolated into SQL. Unknown `sort` falls back to `default_key`;
//! `dir` other than asc/desc falls back to `default_dir`. Every column orders
//! `NULLS LAST` and a stable `id`-direction tiebreaker is appended so paging
//! is deterministic when the primary key ties.

/// Resolve `sort`/`dir` into an `ORDER BY` body (without the `ORDER BY`
/// keyword). `allow` maps each accepted public sort key to its trusted SQL
/// ordering expression. `default_dir` must be `"ASC"` or `"DESC"`.
pub fn resolve_order_by(
    sort: Option<&str>,
    dir: Option<&str>,
    allow: &[(&str, &str)],
    default_key: &str,
    default_dir: &str,
) -> String {
    let lookup = |key: &str| allow.iter().find(|(k, _)| *k == key).map(|(_, e)| *e);
    let expr = sort
        .and_then(lookup)
        .or_else(|| lookup(default_key))
        .expect("default_key must be present in allow");
    let dir_sql = match dir.map(str::to_ascii_lowercase).as_deref() {
        Some("asc") => "ASC",
        Some("desc") => "DESC",
        _ if default_dir.eq_ignore_ascii_case("asc") => "ASC",
        _ => "DESC",
    };
    format!("{expr} {dir_sql} NULLS LAST, id {dir_sql}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALLOW: &[(&str, &str)] = &[
        ("name", "name"),
        ("last_seen", "last_seen_at"),
        ("emissions", "emission_count"),
    ];

    #[test]
    fn known_key_and_dir() {
        assert_eq!(
            resolve_order_by(Some("name"), Some("asc"), ALLOW, "last_seen", "DESC"),
            "name ASC NULLS LAST, id ASC"
        );
        assert_eq!(
            resolve_order_by(Some("emissions"), Some("desc"), ALLOW, "last_seen", "DESC"),
            "emission_count DESC NULLS LAST, id DESC"
        );
    }

    #[test]
    fn unknown_sort_falls_back_to_default_key() {
        assert_eq!(
            resolve_order_by(Some("bogus"), None, ALLOW, "last_seen", "DESC"),
            "last_seen_at DESC NULLS LAST, id DESC"
        );
        assert_eq!(
            resolve_order_by(None, None, ALLOW, "last_seen", "DESC"),
            "last_seen_at DESC NULLS LAST, id DESC"
        );
    }

    #[test]
    fn garbage_dir_falls_back_to_default_dir() {
        assert_eq!(
            resolve_order_by(Some("name"), Some("sideways"), ALLOW, "last_seen", "ASC"),
            "name ASC NULLS LAST, id ASC"
        );
    }

    #[test]
    fn injection_attempt_never_reaches_output() {
        let out = resolve_order_by(
            Some("name; DROP TABLE emitter"),
            Some("desc"),
            ALLOW,
            "last_seen",
            "DESC",
        );
        // Unknown key -> default expression; the raw value never appears.
        assert_eq!(out, "last_seen_at DESC NULLS LAST, id DESC");
        assert!(!out.contains("DROP"));
    }
}
