//! Parsing of task `data` values.

/// Parses a value string as JSON; bare strings that are not valid JSON are
/// stored as-is. Rejects `null`.
pub fn parse_value(raw: &str) -> anyhow::Result<serde_json::Value> {
    let value: serde_json::Value =
        serde_json::from_str(raw).unwrap_or_else(|_| serde_json::Value::String(raw.to_owned()));
    if value.is_null() {
        anyhow::bail!("data value must not be null");
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_json_and_bare_strings() {
        assert_eq!(parse_value("42").unwrap(), serde_json::json!(42));
        assert_eq!(parse_value("true").unwrap(), serde_json::json!(true));
        assert_eq!(parse_value("[1,2]").unwrap(), serde_json::json!([1, 2]));
        assert_eq!(parse_value("hello").unwrap(), serde_json::json!("hello"));
    }

    #[test]
    fn rejects_null() {
        assert!(parse_value("null").is_err());
    }
}
