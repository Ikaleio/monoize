//! Claude model capabilities that decide generated request controls (RC4, RC4d.1, RC4g).
//! Every function accepts the upstream model identifier in any case.

fn denotes_claude_model(model: &str) -> bool {
    model.contains("claude")
        || ["opus-", "sonnet-", "haiku-"]
            .iter()
            .any(|prefix| model.starts_with(prefix))
}

fn claude_family_version(model: &str, family: &str) -> Option<(u32, u32)> {
    let family_pos = model.find(family)?;
    if let Some(version) = parse_model_version_prefix(&model[family_pos + family.len()..]) {
        return Some(version);
    }

    let claude_pos = model.find("claude")?;
    if claude_pos < family_pos {
        return parse_model_version_prefix(&model[claude_pos + "claude".len()..]);
    }
    None
}

fn parse_model_version_prefix(value: &str) -> Option<(u32, u32)> {
    let value = value.trim_start_matches(['-', '.']);
    let major_len = value.chars().take_while(|c| c.is_ascii_digit()).count();
    if major_len == 0 || major_len > 2 {
        return None;
    }
    let major = value[..major_len].parse::<u32>().ok()?;
    let rest = value[major_len..]
        .strip_prefix('-')
        .or_else(|| value[major_len..].strip_prefix('.'));
    let minor = rest
        .map(|rest| {
            rest.chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
        })
        .filter(|minor| !minor.is_empty() && minor.len() <= 2)
        .and_then(|minor| minor.parse::<u32>().ok())
        .unwrap_or(0);
    Some((major, minor))
}

fn family_at_least(model: &str, family: &str, minimum: (u32, u32)) -> bool {
    claude_family_version(model, family).is_some_and(|version| version >= minimum)
}

/// Claude identifiers default to adaptive thinking. The bounded legacy exceptions are
/// Opus <= 4.5, Sonnet <= 4.5, and Haiku <= 4.5.
pub(super) fn model_supports_adaptive(model: &str) -> bool {
    let model = model.to_lowercase();
    if !denotes_claude_model(&model) {
        return false;
    }
    for family in ["opus", "sonnet", "haiku"] {
        if let Some(version) = claude_family_version(&model, family) {
            return version > (4, 5);
        }
    }
    true
}

/// RC4d.1: models that answer `thinking.type = "disabled"` with HTTP 400.
fn model_rejects_disabled_thinking(model: &str) -> bool {
    denotes_claude_model(model)
        && (model.contains("fable")
            || model.contains("mythos")
            || family_at_least(model, "opus", (5, 5))
            || family_at_least(model, "sonnet", (5, 5)))
}

/// RC4: the thinking type generated for normalized effort `none`.
pub(super) fn generated_disabled_thinking_type(model: &str) -> Option<&'static str> {
    let model = model.to_lowercase();
    if !model_rejects_disabled_thinking(&model) {
        return Some("disabled");
    }
    // Sonnet 5.5 turns up-front thinking off with `between_tools`; other always-on
    // models think without configuration, so the field is omitted.
    (claude_family_version(&model, "sonnet") == Some((5, 5))).then_some("between_tools")
}

/// RC4d.1: models that accept `output_config.effort = "xhigh"`.
pub(super) fn model_supports_xhigh_effort(model: &str) -> bool {
    let model = model.to_lowercase();
    denotes_claude_model(&model)
        && (model.contains("fable")
            || (model.contains("mythos") && !model.contains("preview"))
            || family_at_least(&model, "opus", (4, 7))
            || family_at_least(&model, "sonnet", (5, 0))
            || family_at_least(&model, "haiku", (5, 5)))
}

/// RC4d.1: models that answer non-default sampling values with HTTP 400.
pub(super) fn model_rejects_nondefault_sampling(model: &str) -> bool {
    let model = model.to_lowercase();
    denotes_claude_model(&model)
        && (model.contains("fable")
            || model.contains("mythos")
            || family_at_least(&model, "opus", (4, 7))
            || family_at_least(&model, "sonnet", (5, 0))
            || family_at_least(&model, "haiku", (5, 5)))
}

/// RC4d.1: adaptive-thinking models that reject forced tool use.
pub(super) fn model_rejects_forced_tool_choice_with_adaptive(model: &str) -> bool {
    let model = model.to_lowercase();
    denotes_claude_model(&model)
        && (family_at_least(&model, "fable", (5, 1))
            || family_at_least(&model, "mythos", (5, 1))
            || family_at_least(&model, "opus", (5, 5))
            || family_at_least(&model, "sonnet", (5, 5)))
}

/// RC4g: default `max_tokens`, bounded by the model's documented synchronous output limit.
pub(super) fn messages_default_max_tokens(model: &str) -> u64 {
    let model = model.to_lowercase();
    if !denotes_claude_model(&model) || model.contains("fable") || model.contains("mythos") {
        return 128_000;
    }
    if let Some(version) = claude_family_version(&model, "opus") {
        return match version {
            v if v >= (4, 6) => 128_000,
            v if v >= (4, 5) => 64_000,
            v if v >= (4, 0) => 32_000,
            _ => 4_096,
        };
    }
    if let Some(version) = claude_family_version(&model, "sonnet") {
        return match version {
            v if v >= (4, 6) => 128_000,
            v if v >= (3, 7) => 64_000,
            v if v >= (3, 5) => 8_192,
            _ => 4_096,
        };
    }
    if let Some(version) = claude_family_version(&model, "haiku") {
        return match version {
            v if v >= (5, 5) => 128_000,
            v if v >= (4, 0) => 64_000,
            v if v >= (3, 5) => 8_192,
            _ => 4_096,
        };
    }
    128_000
}
