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
    /// 1-based source line of the `id` key (CJS-VAL-7).
    pub id_line: usize,
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
/// Returns a human-readable rejection reason on any violation, prefixed with
/// `line <N>: ` when the violation belongs to one source line (CJS-VAL-7).
pub fn parse_frontmatter(source: &str) -> Result<CustomTransformMeta, String> {
    let trimmed = source.trim_start();
    let start_line = source[..source.len() - trimmed.len()].matches('\n').count() + 1;
    let after_opener = trimmed
        .strip_prefix("/**")
        .or_else(|| trimmed.strip_prefix("/*"))
        .ok_or_else(|| {
            at_line(
                start_line,
                "source must begin with a /* ... */ frontmatter block comment",
            )
        })?;
    let body = after_opener
        .split_once("*/")
        .map(|(body, _)| body)
        .ok_or_else(|| "frontmatter block comment is not terminated with */".to_string())?;

    // The comment body starts on the opener line, so body line `i` is source
    // line `start_line + i`.
    let mut lines = body.lines().enumerate().filter_map(|(index, line)| {
        normalize_frontmatter_line(line).map(|line| (start_line + index, line))
    });
    match lines.next() {
        Some((_, "@monoize-transform")) => {}
        other => {
            return Err(at_line(
                other.map_or(start_line, |(line_no, _)| line_no),
                "the first frontmatter line must be exactly '@monoize-transform'",
            ));
        }
    }

    let mut id = None;
    let mut name = None;
    let mut description = None;
    let mut author = None;
    let mut phase_raw = None;
    let mut scopes_raw = None;
    let mut visibility_raw = None;

    for (line_no, line) in lines {
        let (key, value) = line
            .split_once(':')
            .map(|(key, value)| (key.trim(), value.trim()))
            .ok_or_else(|| {
                at_line(
                    line_no,
                    format!("frontmatter line '{line}' is not in 'key: value' form"),
                )
            })?;
        if !FRONTMATTER_KEYS.contains(&key) {
            return Err(at_line(line_no, format!("unknown frontmatter key '{key}'")));
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
            return Err(at_line(
                line_no,
                format!("duplicated frontmatter key '{key}'"),
            ));
        }
        *slot = Some((value.to_string(), line_no));
    }

    let (id, id_line) = require_value("id", id)?;
    if !is_valid_custom_transform_id(&id) {
        return Err(at_line(
            id_line,
            format!(
                "id '{id}' must match ^js:[a-z0-9]+(-[a-z0-9]+)*$ and be at most \
                 {CUSTOM_TRANSFORM_ID_MAX_LEN} characters"
            ),
        ));
    }
    let name = require_bounded("name", name, 100)?;
    let description = require_bounded("description", description, 500)?;
    let author = require_bounded("author", author, 100)?;

    let phases = match phase_raw {
        None => vec![Phase::Request, Phase::Response],
        Some((raw, line_no)) => match raw.as_str() {
            "both" => vec![Phase::Request, Phase::Response],
            "request" => vec![Phase::Request],
            "response" => vec![Phase::Response],
            other => {
                return Err(at_line(
                    line_no,
                    format!("phase '{other}' must be one of request, response, both"),
                ));
            }
        },
    };

    let scopes = match scopes_raw {
        None => vec![
            TransformScope::Provider,
            TransformScope::Global,
            TransformScope::ApiKey,
        ],
        Some((raw, line_no)) => parse_scopes(&raw).map_err(|error| at_line(line_no, error))?,
    };

    let visibility = match visibility_raw {
        None => CustomTransformVisibility::Admin,
        Some((raw, line_no)) => CustomTransformVisibility::parse(&raw).ok_or_else(|| {
            at_line(
                line_no,
                format!("visibility '{raw}' must be one of admin, user"),
            )
        })?,
    };

    Ok(CustomTransformMeta {
        id,
        name,
        description,
        author,
        phases,
        scopes,
        visibility,
        id_line,
    })
}

/// CJS-VAL-7 line attribution prefix.
pub fn at_line(line: usize, message: impl std::fmt::Display) -> String {
    format!("line {line}: {message}")
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

fn require_value(key: &str, value: Option<(String, usize)>) -> Result<(String, usize), String> {
    match value {
        Some((value, line_no)) if !value.is_empty() => Ok((value, line_no)),
        Some((_, line_no)) => Err(at_line(
            line_no,
            format!("required frontmatter key '{key}' is missing or empty"),
        )),
        None => Err(format!(
            "required frontmatter key '{key}' is missing or empty"
        )),
    }
}

fn require_bounded(
    key: &str,
    value: Option<(String, usize)>,
    max_chars: usize,
) -> Result<String, String> {
    let (value, line_no) = require_value(key, value)?;
    if value.chars().count() > max_chars {
        return Err(at_line(
            line_no,
            format!("frontmatter '{key}' must be at most {max_chars} characters"),
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
