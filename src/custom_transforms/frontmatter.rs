//! Frontmatter parser for custom JS transforms (`custom-js-transforms.spec.md` §2).
//!
//! The source must begin with one block comment whose first non-empty
//! normalized line is `@monoize-transform`, followed by `key: value` lines.

use crate::transforms::{Phase, TransformScope};
use serde::{Deserialize, Serialize};

pub const CUSTOM_TRANSFORM_ID_PREFIX: &str = "js:";
pub const CUSTOM_TRANSFORM_ID_MAX_LEN: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CustomTransformVisibility {
    Admin,
    User,
}

impl CustomTransformVisibility {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::User => "user",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "admin" => Some(Self::Admin),
            "user" => Some(Self::User),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CustomTransformMeta {
    pub id: String,
    pub name: String,
    pub description: String,
    pub author: String,
    pub phases: Vec<Phase>,
    pub scopes: Vec<TransformScope>,
    pub visibility: CustomTransformVisibility,
}

/// CJS-ID-2: `^js:[a-z0-9]+(-[a-z0-9]+)*$`, at most 64 chars including prefix.
pub fn is_valid_custom_transform_id(id: &str) -> bool {
    if id.len() > CUSTOM_TRANSFORM_ID_MAX_LEN {
        return false;
    }
    let Some(body) = id.strip_prefix(CUSTOM_TRANSFORM_ID_PREFIX) else {
        return false;
    };
    if body.is_empty() {
        return false;
    }
    body.split('-').all(|segment| {
        !segment.is_empty()
            && segment
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit())
    })
}

const FRONTMATTER_KEYS: &[&str] = &[
    "id",
    "name",
    "description",
    "author",
    "phase",
    "scopes",
    "visibility",
];

/// Parses the frontmatter block per CJS-FM-1 through CJS-FM-6.
/// Returns a human-readable rejection reason on any violation.
pub fn parse_frontmatter(source: &str) -> Result<CustomTransformMeta, String> {
    let trimmed = source.trim_start();
    let after_opener = trimmed
        .strip_prefix("/**")
        .or_else(|| trimmed.strip_prefix("/*"))
        .ok_or_else(|| {
            "source must begin with a /* ... */ frontmatter block comment".to_string()
        })?;
    let body = after_opener
        .split_once("*/")
        .map(|(body, _)| body)
        .ok_or_else(|| "frontmatter block comment is not terminated with */".to_string())?;

    let mut lines = body.lines().filter_map(normalize_frontmatter_line);
    match lines.next() {
        Some(first) if first == "@monoize-transform" => {}
        _ => {
            return Err(
                "the first frontmatter line must be exactly '@monoize-transform'".to_string(),
            );
        }
    }

    let mut id = None;
    let mut name = None;
    let mut description = None;
    let mut author = None;
    let mut phase_raw = None;
    let mut scopes_raw = None;
    let mut visibility_raw = None;

    for line in lines {
        let (key, value) = line
            .split_once(':')
            .map(|(key, value)| (key.trim(), value.trim()))
            .ok_or_else(|| format!("frontmatter line '{line}' is not in 'key: value' form"))?;
        if !FRONTMATTER_KEYS.contains(&key) {
            return Err(format!("unknown frontmatter key '{key}'"));
        }
        let slot = match key {
            "id" => &mut id,
            "name" => &mut name,
            "description" => &mut description,
            "author" => &mut author,
            "phase" => &mut phase_raw,
            "scopes" => &mut scopes_raw,
            _ => &mut visibility_raw,
        };
        if slot.is_some() {
            return Err(format!("duplicated frontmatter key '{key}'"));
        }
        *slot = Some(value.to_string());
    }

    let id = require_value("id", id)?;
    if !is_valid_custom_transform_id(&id) {
        return Err(format!(
            "id '{id}' must match ^js:[a-z0-9]+(-[a-z0-9]+)*$ and be at most \
             {CUSTOM_TRANSFORM_ID_MAX_LEN} characters"
        ));
    }
    let name = require_bounded("name", name, 100)?;
    let description = require_bounded("description", description, 500)?;
    let author = require_bounded("author", author, 100)?;

    let phases = match phase_raw.as_deref() {
        None | Some("both") => vec![Phase::Request, Phase::Response],
        Some("request") => vec![Phase::Request],
        Some("response") => vec![Phase::Response],
        Some(other) => {
            return Err(format!(
                "phase '{other}' must be one of request, response, both"
            ));
        }
    };

    let scopes = match scopes_raw.as_deref() {
        None => vec![
            TransformScope::Provider,
            TransformScope::Global,
            TransformScope::ApiKey,
        ],
        Some(raw) => parse_scopes(raw)?,
    };

    let visibility = match visibility_raw.as_deref() {
        None => CustomTransformVisibility::Admin,
        Some(raw) => CustomTransformVisibility::parse(raw)
            .ok_or_else(|| format!("visibility '{raw}' must be one of admin, user"))?,
    };

    Ok(CustomTransformMeta {
        id,
        name,
        description,
        author,
        phases,
        scopes,
        visibility,
    })
}

/// CJS-FM-2 normalization: strip leading whitespace, one optional `*`, then one
/// optional space. Empty results are dropped.
fn normalize_frontmatter_line(line: &str) -> Option<&str> {
    let line = line.trim_start();
    let line = line.strip_prefix('*').unwrap_or(line);
    let line = line.strip_prefix(' ').unwrap_or(line);
    let line = line.trim_end();
    if line.is_empty() { None } else { Some(line) }
}

fn require_value(key: &str, value: Option<String>) -> Result<String, String> {
    match value {
        Some(value) if !value.is_empty() => Ok(value),
        _ => Err(format!("required frontmatter key '{key}' is missing or empty")),
    }
}

fn require_bounded(key: &str, value: Option<String>, max_chars: usize) -> Result<String, String> {
    let value = require_value(key, value)?;
    if value.chars().count() > max_chars {
        return Err(format!(
            "frontmatter '{key}' must be at most {max_chars} characters"
        ));
    }
    Ok(value)
}

fn parse_scopes(raw: &str) -> Result<Vec<TransformScope>, String> {
    let mut scopes = Vec::new();
    for entry in raw.split(',') {
        let entry = entry.trim();
        let scope = match entry {
            "provider" => TransformScope::Provider,
            "global" => TransformScope::Global,
            "api_key" => TransformScope::ApiKey,
            other => {
                return Err(format!(
                    "scope '{other}' must be one of provider, global, api_key"
                ));
            }
        };
        if scopes.contains(&scope) {
            return Err(format!("duplicated scope '{entry}'"));
        }
        scopes.push(scope);
    }
    if scopes.is_empty() {
        return Err("scopes must not be empty".to_string());
    }
    Ok(scopes)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_SOURCE: &str = r#"/**
 * @monoize-transform
 * id: js:example-rewrite
 * name: Example Rewrite
 * description: Rewrites request fields for demo.
 * author: alice
 * phase: request
 * scopes: provider, global
 * visibility: user
 */
function transform(ctx) {}
"#;

    #[test]
    fn parses_full_frontmatter() {
        let meta = parse_frontmatter(VALID_SOURCE).expect("parses");
        assert_eq!(meta.id, "js:example-rewrite");
        assert_eq!(meta.name, "Example Rewrite");
        assert_eq!(meta.description, "Rewrites request fields for demo.");
        assert_eq!(meta.author, "alice");
        assert_eq!(meta.phases, vec![Phase::Request]);
        assert_eq!(
            meta.scopes,
            vec![TransformScope::Provider, TransformScope::Global]
        );
        assert_eq!(meta.visibility, CustomTransformVisibility::User);
    }

    #[test]
    fn applies_defaults_for_optional_keys() {
        let source = "/* @monoize-transform\nid: js:x1\nname: N\ndescription: D\nauthor: a */";
        let meta = parse_frontmatter(source).expect("parses");
        assert_eq!(meta.phases, vec![Phase::Request, Phase::Response]);
        assert_eq!(
            meta.scopes,
            vec![
                TransformScope::Provider,
                TransformScope::Global,
                TransformScope::ApiKey
            ]
        );
        assert_eq!(meta.visibility, CustomTransformVisibility::Admin);
    }

    #[test]
    fn rejects_missing_marker() {
        let source = "/* id: js:x */";
        assert!(parse_frontmatter(source).is_err());
    }

    #[test]
    fn rejects_unknown_and_duplicate_keys() {
        let unknown =
            "/* @monoize-transform\nid: js:x\nname: N\ndescription: D\nauthor: a\nextra: v */";
        assert!(parse_frontmatter(unknown).unwrap_err().contains("unknown"));
        let duplicate =
            "/* @monoize-transform\nid: js:x\nid: js:y\nname: N\ndescription: D\nauthor: a */";
        assert!(
            parse_frontmatter(duplicate)
                .unwrap_err()
                .contains("duplicated")
        );
    }

    #[test]
    fn rejects_invalid_ids() {
        for id in [
            "custom:x",
            "js:",
            "js:UpperCase",
            "js:has_underscore",
            "js:-leading",
            "js:trailing-",
            "js:a--b",
        ] {
            assert!(!is_valid_custom_transform_id(id), "id {id} must be invalid");
        }
        assert!(is_valid_custom_transform_id("js:a-2b-c"));
        assert!(!is_valid_custom_transform_id(&format!(
            "js:{}",
            "a".repeat(CUSTOM_TRANSFORM_ID_MAX_LEN)
        )));
    }

    #[test]
    fn rejects_bad_optional_values() {
        let bad_phase =
            "/* @monoize-transform\nid: js:x\nname: N\ndescription: D\nauthor: a\nphase: pre */";
        assert!(parse_frontmatter(bad_phase).is_err());
        let bad_scope =
            "/* @monoize-transform\nid: js:x\nname: N\ndescription: D\nauthor: a\nscopes: user */";
        assert!(parse_frontmatter(bad_scope).is_err());
        let duplicate_scope = "/* @monoize-transform\nid: js:x\nname: N\ndescription: D\nauthor: a\nscopes: global, global */";
        assert!(parse_frontmatter(duplicate_scope).is_err());
        let bad_visibility = "/* @monoize-transform\nid: js:x\nname: N\ndescription: D\nauthor: a\nvisibility: everyone */";
        assert!(parse_frontmatter(bad_visibility).is_err());
    }

    #[test]
    fn rejects_missing_required_keys() {
        let source = "/* @monoize-transform\nid: js:x\nname: N\nauthor: a */";
        assert!(parse_frontmatter(source).unwrap_err().contains("description"));
    }
}
