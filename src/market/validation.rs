//! Zod-parity parsing of `manifest.json` files.
//!
//! Mirrors `manifestSchema` in FetchRemotes.ts field by field:
//!
//! | field       | schema behaviour on bad input                        |
//! |-------------|------------------------------------------------------|
//! | name        | required, trimmed, min 1 -> whole manifest rejected  |
//! | description | required, trimmed, min 1 -> whole manifest rejected  |
//! | main        | optional, trimmed, min 1 -> rejected                 |
//! | usercss     | optional, trimmed, min 1 -> rejected                 |
//! | branch      | optional, trimmed, min 1 -> rejected                 |
//! | schemes     | optional string -> rejected                          |
//! | authors     | array of {name, url?} -> `.catch([])` (falls back)   |
//! | preview     | nullish string, defaults "" -> rejected              |
//! | readme      | nullish string, defaults "" -> rejected              |
//! | tags        | string or string[] -> `.catch([])`                   |
//! | include     | string[] -> `.catch([])`                             |
//! | assets      | spice-pm exclusive: optional trimmed min-1 string    |
//! | *           | passthrough (unknown keys preserved)                 |

/// Keys consumed by the schema; everything else lands in `extra` passthrough.
const KNOWN_KEYS: [&str; 12] = [
    "name",
    "description",
    "main",
    "usercss",
    "branch",
    "schemes",
    "authors",
    "preview",
    "readme",
    "tags",
    "include",
    "assets",
];

use super::types::{Author, Manifest};
use serde_json::Value;
use std::collections::BTreeMap;

/// Parse one manifest object. Returns Err with a short reason when the
/// manifest would fail `safeParse` in the web app.
pub fn parse_manifest(value: &Value) -> Result<Manifest, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "manifest is not an object".to_owned())?;

    let name = required_trimmed(obj, "name")?;
    let description = required_trimmed(obj, "description")?;
    let main = optional_trimmed_min1(obj, "main")?;
    let usercss = optional_trimmed_min1(obj, "usercss")?;
    let branch = optional_trimmed_min1(obj, "branch")?;
    let schemes = optional_string(obj, "schemes")?;

    let preview = nullish_string(obj, "preview")?.unwrap_or_default();
    let readme = nullish_string(obj, "readme")?.unwrap_or_default();
    let authors = parse_authors_catch(obj.get("authors"));
    let tags = parse_tags_catch(obj.get("tags"));
    let include = parse_include_catch(obj.get("include"));
    let assets = optional_trimmed_min1(obj, "assets")?;

    let extra: BTreeMap<String, Value> = obj
        .iter()
        .filter(|(k, _)| !KNOWN_KEYS.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    Ok(Manifest {
        name,
        description,
        main,
        usercss,
        authors,
        preview,
        readme,
        tags,
        branch,
        schemes,
        include,
        assets,
        extra,
    })
}

/// Parse either a single manifest object or an array of them.
/// Invalid entries are skipped, mirroring `manifests.flatMap(... safeParse ...)`.
/// Returns the parsed manifests plus warnings for skipped ones.
pub fn parse_manifests(value: &Value, source_label: &str) -> (Vec<Manifest>, Vec<String>) {
    let mut out = Vec::new();
    let mut warnings = Vec::new();
    let entries: Vec<&Value> = match value {
        Value::Array(items) => items.iter().collect(),
        Value::Object(_) => vec![value],
        _ => {
            return (
                out,
                vec![format!("Invalid Marketplace manifest from {source_label}")],
            );
        }
    };
    for entry in entries {
        match parse_manifest(entry) {
            Ok(m) => out.push(m),
            Err(reason) => warnings.push(format!(
                "Invalid Marketplace manifest from {source_label}: {reason}"
            )),
        }
    }
    (out, warnings)
}

fn required_trimmed(obj: &serde_json::Map<String, Value>, key: &str) -> Result<String, String> {
    match obj.get(key) {
        Some(Value::String(s)) => {
            let t = s.trim();
            if t.is_empty() {
                Err(format!("`{key}` must not be empty"))
            } else {
                Ok(t.to_owned())
            }
        }
        _ => Err(format!("missing required string `{key}`")),
    }
}

fn optional_trimmed_min1(
    obj: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<String>, String> {
    match obj.get(key) {
        None => Ok(None),
        Some(Value::String(s)) => {
            let t = s.trim();
            if t.is_empty() {
                Err(format!("`{key}` must not be empty when present"))
            } else {
                Ok(Some(t.to_owned()))
            }
        }
        // `.optional()` only accepts undefined, not null
        Some(_) => Err(format!("`{key}` must be a string when present")),
    }
}

fn optional_string(
    obj: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<String>, String> {
    match obj.get(key) {
        None => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(format!("`{key}` must be a string when present")),
    }
}

fn nullish_string(
    obj: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<String>, String> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(format!("`{key}` must be a string or null")),
    }
}

/// Authors have `.catch([])`: any violation yields an empty list.
fn parse_authors_catch(value: Option<&Value>) -> Vec<Author> {
    let Some(Value::Array(items)) = value else {
        return Vec::new();
    };
    let mut authors = Vec::with_capacity(items.len());
    for item in items {
        let Some(obj) = item.as_object() else {
            return Vec::new();
        };
        let Some(Value::String(name)) = obj.get("name") else {
            return Vec::new();
        };
        let name = name.trim();
        if name.is_empty() {
            return Vec::new();
        }
        let url = match obj.get("url") {
            // zod's z.url() = successful WHATWG parse (any scheme);
            // dangerous schemes are neutralized later by sanitize_url
            None | Some(Value::Null) => format!("https://github.com/{name}"),
            Some(Value::String(u)) => match url::Url::parse(u) {
                Ok(_) => u.clone(),
                Err(_) => return Vec::new(),
            },
            Some(_) => return Vec::new(),
        };
        authors.push(Author {
            name: name.to_owned(),
            url,
        });
    }
    authors
}

fn parse_tags_catch(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| v.as_str().map(str::to_owned))
            .collect::<Option<Vec<_>>>()
            .unwrap_or_default(),
        Some(Value::String(s)) => vec![s.clone()],
        None | Some(_) => Vec::new(),
    }
}

fn parse_include_catch(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| v.as_str().map(str::to_owned))
            .collect::<Option<Vec<_>>>()
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn valid_extension_manifest() {
        let m = parse_manifest(&json!({
            "name": "My Ext",
            "description": "Does things",
            "main": "myext.js",
            "authors": [{ "name": "alice" }, { "name": "bob", "url": "https://bob.dev" }],
            "tags": ["tools", "fun"],
            "unknown-key": 42
        }))
        .unwrap();
        assert_eq!(m.name, "My Ext");
        assert_eq!(m.authors.len(), 2);
        assert_eq!(m.authors[0].url, "https://github.com/alice");
        assert_eq!(m.authors[1].url, "https://bob.dev");
        assert_eq!(m.tags.len(), 2);
        assert_eq!(m.extra.get("unknown-key"), Some(&json!(42)));
        assert!(m.usercss.is_none());
    }

    #[test]
    fn missing_name_rejects() {
        assert!(parse_manifest(&json!({ "description": "x" })).is_err());
        assert!(parse_manifest(&json!({ "name": "", "description": "x" })).is_err());
        assert!(parse_manifest(&json!({ "name": "  ", "description": "x" })).is_err());
    }

    #[test]
    fn missing_description_rejects() {
        assert!(parse_manifest(&json!({ "name": "x" })).is_err());
    }

    #[test]
    fn empty_main_rejects_but_missing_ok() {
        assert!(parse_manifest(&json!({ "name": "n", "description": "d", "main": " " })).is_err());
        assert!(
            parse_manifest(&json!({ "name": "n", "description": "d" }))
                .unwrap()
                .main
                .is_none()
        );
        // null is not accepted for `.optional()` fields
        assert!(parse_manifest(&json!({ "name": "n", "description": "d", "main": null })).is_err());
    }

    #[test]
    fn authors_bad_shape_falls_back_to_empty() {
        let m = parse_manifest(&json!({
            "name": "n",
            "description": "d",
            "authors": [{ "nope": true }]
        }))
        .unwrap();
        assert!(m.authors.is_empty());

        let m = parse_manifest(&json!({
            "name": "n",
            "description": "d",
            "authors": [{ "name": "ok" }, { "name": "" }]
        }))
        .unwrap();
        assert!(m.authors.is_empty(), "one bad author poisons the list");

        // unparseable url also poisons
        let m = parse_manifest(&json!({
            "name": "n",
            "description": "d",
            "authors": [{ "name": "a", "url": "not a url" }]
        }))
        .unwrap();
        assert!(m.authors.is_empty());

        // but a valid javascript: url passes zod and is sanitized downstream
        let m = parse_manifest(&json!({
            "name": "n",
            "description": "d",
            "authors": [{ "name": "a", "url": "javascript:alert(1)" }]
        }))
        .unwrap();
        assert_eq!(m.authors[0].url, "javascript:alert(1)");
    }

    #[test]
    fn tags_accept_single_string_or_array_and_catch() {
        let m =
            parse_manifest(&json!({ "name": "n", "description": "d", "tags": "solo" })).unwrap();
        assert_eq!(m.tags, ["solo"]);
        let m = parse_manifest(&json!({
            "name": "n", "description": "d",
            "tags": ["a", 3]
        }))
        .unwrap();
        assert!(m.tags.is_empty());
    }

    #[test]
    fn preview_readme_nullish_defaults_empty() {
        let m =
            parse_manifest(&json!({ "name": "n", "description": "d", "preview": null })).unwrap();
        assert_eq!(m.preview, "");
        let m = parse_manifest(&json!({ "name": "n", "description": "d", "preview": 5 }));
        assert!(m.is_err());
    }

    #[test]
    fn assets_is_spice_pm_exclusive_optional_string() {
        // accepted and typed, no longer passthrough
        let m = parse_manifest(&json!({
            "name": "n", "description": "d", "usercss": "a.css",
            "assets": "assets"
        }))
        .unwrap();
        assert_eq!(m.assets.as_deref(), Some("assets"));
        assert!(!m.extra.contains_key("assets"), "must be a known key now");

        let m = parse_manifest(&json!({
            "name": "n", "description": "d",
            "assets": "https://github.com/u/r/tree/main/assets"
        }))
        .unwrap();
        assert_eq!(
            m.assets.as_deref(),
            Some("https://github.com/u/r/tree/main/assets")
        );

        // absent -> None
        let m = parse_manifest(&json!({ "name": "n", "description": "d" })).unwrap();
        assert!(m.assets.is_none());

        // wrong type / empty -> manifest rejected (same strictness as main)
        assert!(parse_manifest(&json!({ "name": "n", "description": "d", "assets": 5 })).is_err());
        assert!(
            parse_manifest(&json!({ "name": "n", "description": "d", "assets": null })).is_err()
        );
        assert!(
            parse_manifest(&json!({ "name": "n", "description": "d", "assets": " " })).is_err()
        );
    }

    #[test]
    fn array_of_manifests_skips_invalid() {
        let (parsed, warnings) = parse_manifests(
            &json!([
                { "name": "good", "description": "d", "main": "a.js" },
                { "description": "missing name" }
            ]),
            "u/r",
        );
        assert_eq!(parsed.len(), 1);
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn non_object_source_yields_warning_only() {
        let (parsed, warnings) = parse_manifests(&json!(null), "u/r");
        assert!(parsed.is_empty());
        assert_eq!(warnings.len(), 1);
    }
}
