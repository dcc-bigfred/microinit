//! Label helpers for CLI selectors and formatting.

use std::collections::BTreeMap;

use crate::error::{Error, Result};

/// Parse a single `key=value` selector (exactly one `=`).
pub fn parse_selector(s: &str) -> Result<(String, String)> {
    let Some((key, value)) = s.split_once('=') else {
        return Err(Error::Config(format!(
            "invalid label selector {s:?}: want key=value"
        )));
    };
    if key.is_empty() || value.is_empty() {
        return Err(Error::Config(format!(
            "invalid label selector {s:?}: key and value must be non-empty"
        )));
    }
    if key.contains('=') {
        return Err(Error::Config(format!(
            "invalid label selector {s:?}: want key=value"
        )));
    }
    Ok((key.to_string(), value.to_string()))
}

/// True when `labels` contains every key=value in `want` (AND).
pub fn matches_selectors(labels: &BTreeMap<String, String>, want: &[(String, String)]) -> bool {
    want.iter()
        .all(|(k, v)| labels.get(k).map(|have| have == v).unwrap_or(false))
}

/// True when `labels` contains every key in `keys` (presence, any value).
/// Empty `keys` always matches.
#[must_use]
pub fn has_keys(labels: &BTreeMap<String, String>, keys: &[String]) -> bool {
    keys.iter().all(|k| labels.contains_key(k))
}

/// Format labels as `k=v,k=v` (BTreeMap order).
pub fn format_labels(labels: &BTreeMap<String, String>) -> String {
    labels
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ok() {
        assert_eq!(
            parse_selector("created-by=bigfred").unwrap(),
            ("created-by".into(), "bigfred".into())
        );
    }

    #[test]
    fn parse_rejects_bad() {
        assert!(parse_selector("noequals").is_err());
        assert!(parse_selector("=v").is_err());
        assert!(parse_selector("k=").is_err());
    }

    #[test]
    fn match_and() {
        let mut labels = BTreeMap::new();
        labels.insert("created-by".into(), "bigfred".into());
        labels.insert("env".into(), "prod".into());
        assert!(matches_selectors(
            &labels,
            &[("created-by".into(), "bigfred".into())]
        ));
        assert!(matches_selectors(
            &labels,
            &[
                ("created-by".into(), "bigfred".into()),
                ("env".into(), "prod".into())
            ]
        ));
        assert!(!matches_selectors(
            &labels,
            &[("created-by".into(), "other".into())]
        ));
        assert!(!matches_selectors(
            &labels,
            &[("missing".into(), "x".into())]
        ));
    }

    #[test]
    fn has_keys_presence() {
        let mut labels = BTreeMap::new();
        labels.insert("microdns-port".into(), "8080".into());
        assert!(has_keys(&labels, &[]));
        assert!(has_keys(&labels, &["microdns-port".into()]));
        assert!(!has_keys(&labels, &["microdns-type".into()]));
        assert!(!has_keys(
            &labels,
            &["microdns-port".into(), "microdns-type".into()]
        ));
    }
}
