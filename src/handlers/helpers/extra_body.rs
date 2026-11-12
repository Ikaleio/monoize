use super::*;

/// Default upstream extra_body field whitelists per provider type.
///
/// Fields that the URP request decoder already extracts into typed struct
/// fields (model, stream, temperature, etc.) are NOT in extra_body at all;
/// these lists cover only the keys that remain in `UrpRequest.extra_body`
/// and are safe to forward to the given upstream API.
const EXTRA_WHITELIST_CHAT_COMPLETION: &[&str] = &[
    "audio",
    "frequency_penalty",
    "function_call",
    "functions",
    "logit_bias",
    "logprobs",
    "top_logprobs",
    "max_completion_tokens",
    "max_tokens",
    "metadata",
    "moderation",
    "n",
    "presence_penalty",
    "prompt_cache_options",
    "safety_identifier",
    "seed",
    "service_tier",
    "stop",
    "stream_options",
    "store",
    "web_search_options",
    "parallel_tool_calls",
    "debug",
    "image_config",
    "modalities",
    "cache_control",
    "top_k",
    "top_a",
    "min_p",
    "repetition_penalty",
    "prediction",
    "prompt_cache_key",
    "prompt_cache_retention",
    "route",
    "structured_outputs",
    "verbosity",
    // OpenRouter / third-party extension fields
    "provider",
    "plugins",
    "session_id",
    "stop_server_tools_when",
    "trace",
    "thinking",
    "include_reasoning",
    "user_id",
    // Vercel AI Gateway; the Chat encoder emits it only for the vercel dialect (PC0b).
    "providerOptions",
];

const EXTRA_WHITELIST_RESPONSES: &[&str] = &[
    "access_programs",
    "background",
    "context_management",
    "conversation",
    "include",
    "instructions",
    "metadata",
    "max_tool_calls",
    "moderation",
    "parallel_tool_calls",
    "previous_response_id",
    "prompt",
    "prompt_cache_key",
    "prompt_cache_options",
    "prompt_cache_retention",
    "safety_identifier",
    "service_tier",
    "store",
    "stream_options",
    "text",
    "top_logprobs",
    "truncation",
];

const EXTRA_WHITELIST_ANTHROPIC: &[&str] = &[
    "cache_control",
    "container",
    "context_management",
    "diagnostics",
    "inference_geo",
    "max_tokens",
    "mcp_servers",
    "metadata",
    "output_config",
    "service_tier",
    "speed",
    "stop_sequences",
    "top_k",
];

const EXTRA_WHITELIST_GEMINI: &[&str] = &[
    "generationConfig",
    "safetySettings",
    "cachedContent",
    "labels",
    "continuationToken",
    "serviceTier",
    // Mapped to `serviceTier` by the Gemini encoder.
    "service_tier",
    // Mapped to `labels.safety_identifier` by the Gemini encoder.
    "safety_identifier",
];

const EXTRA_WHITELIST_OPENAI_IMAGE: &[&str] = &[
    "complexity",
    "frame_count",
    "frame_duration_ms",
    "svg",
    "safety_identifier",
    "size",
    "quality",
    "style",
    "response_format",
    "n",
    "background",
    "output_format",
    "output_compression",
    "moderation",
    "user",
    "partial_images",
    "input_fidelity",
];

const EXTRA_WHITELIST_OPENROUTER_IMAGE: &[&str] = &[
    "n",
    "resolution",
    "aspect_ratio",
    "size",
    "quality",
    "output_format",
    "background",
    "output_compression",
    "seed",
    "user",
    "provider",
];

fn default_extra_whitelist(provider_type: ProviderType) -> &'static [&'static str] {
    match provider_type {
        ProviderType::ChatCompletion => EXTRA_WHITELIST_CHAT_COMPLETION,
        ProviderType::Responses => EXTRA_WHITELIST_RESPONSES,
        ProviderType::Messages => EXTRA_WHITELIST_ANTHROPIC,
        ProviderType::Gemini => EXTRA_WHITELIST_GEMINI,
        ProviderType::OpenaiImage => EXTRA_WHITELIST_OPENAI_IMAGE,
        ProviderType::OpenrouterImage => EXTRA_WHITELIST_OPENROUTER_IMAGE,
        ProviderType::Group | ProviderType::SystemOne | ProviderType::OpenaiDecisions => &[],
        // Replicate model input schemas are model-specific; whitelist is
        // handled inside the encoder by routing fields into `input`.
        ProviderType::Replicate => &["*"],
    }
}

/// Messages body members that another protocol family may carry under the same name with an
/// incompatible shape (Responses `context_management`), so they pass the default Messages
/// whitelist only for a Messages downstream (XF6g).
const MESSAGES_SAME_FAMILY_EXTRA_FIELDS: &[&str] =
    &["context_management", "diagnostics", "mcp_servers", "speed"];

/// Filter `req.extra_body` to only contain fields allowed by the upstream
/// provider type's whitelist, optionally extended by a provider-level override.
///
/// If `provider_override` contains `"*"`, all fields pass through unfiltered.
pub(in crate::handlers) fn filter_extra_body_for_provider(
    req: &mut urp::UrpRequest,
    provider_type: ProviderType,
    downstream: DownstreamProtocol,
    provider_override: &Option<Vec<String>>,
) {
    if let Some(overrides) = provider_override
        && overrides.iter().any(|s| s == "*")
    {
        return;
    }

    let defaults = default_extra_whitelist(provider_type);
    if defaults.contains(&"*") {
        return;
    }

    let override_set: HashSet<&str> = provider_override
        .as_ref()
        .map(|v| v.iter().map(|s| s.as_str()).collect())
        .unwrap_or_default();
    let same_family = downstream.is_same_family(provider_type);

    req.extra_body.retain(|k, _| {
        urp::is_internal_key(k)
            || override_set.contains(k.as_str())
            || (defaults.contains(&k.as_str())
                && (same_family
                    || provider_type != ProviderType::Messages
                    || !MESSAGES_SAME_FAMILY_EXTRA_FIELDS.contains(&k.as_str())))
    });
}
