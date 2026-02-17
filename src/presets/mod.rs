use serde_json::{Value, json};

pub fn provider_presets() -> Value {
    json!([
        {
            "id": "openai_official",
            "defaults": {
                "base_url": "https://api.openai.com/v1",
                "models": {
                    "gpt-4.1": { "redirect": null, "multiplier": 1.0 },
                    "gpt-4.1-mini": { "redirect": null, "multiplier": 1.0 },
                    "gpt-4.1-nano": { "redirect": null, "multiplier": 1.0 },
                    "o1": { "redirect": null, "multiplier": 1.0 },
                    "o3": { "redirect": null, "multiplier": 1.0 },
                    "o3-mini": { "redirect": null, "multiplier": 1.0 },
                    "o4-mini": { "redirect": null, "multiplier": 1.0 }
                },
                "transforms": [
                    {
                        "transform": "system_to_developer_role",
                        "enabled": true,
                        "models": ["o1*", "o3*", "o4*"],
                        "phase": "request",
                        "config": {}
                    }
                ]
            }
        },
        {
            "id": "anthropic_claude",
            "defaults": {
                "base_url": "https://api.anthropic.com/v1",
                "models": {
                    "claude-sonnet-4": { "redirect": "claude-sonnet-4-20250514", "multiplier": 1.0 },
                    "claude-3.5-haiku": { "redirect": "claude-3-5-haiku-20241022", "multiplier": 1.0 }
                },
                "transforms": [
                    {
                        "transform": "reasoning_effort_to_budget",
                        "enabled": true,
                        "models": ["claude-3.5-*"],
                        "phase": "request",
                        "config": { "low": 1024, "med": 4096, "high": 16384 }
                    }
                ]
            }
        },
        {
            "id": "deepseek",
            "defaults": {
                "base_url": "https://api.deepseek.com/v1",
                "models": {
                    "deepseek-r1": { "redirect": "deepseek-reasoner", "multiplier": 1.0 },
                    "deepseek-v3": { "redirect": "deepseek-chat", "multiplier": 1.0 }
                },
                "transforms": []
            }
        },
        {
            "id": "google_gemini",
            "defaults": {
                "base_url": "https://generativelanguage.googleapis.com/v1beta",
                "models": {
                    "gemini-2.5-pro": { "redirect": null, "multiplier": 1.0 },
                    "gemini-2.5-flash": { "redirect": null, "multiplier": 1.0 }
                },
                "transforms": []
            }
        },
        {
            "id": "xai_grok",
            "defaults": {
                "base_url": "https://api.x.ai",
                "models": {
                    "grok-4": { "redirect": null, "multiplier": 1.0 },
                    "grok-3": { "redirect": null, "multiplier": 1.0 }
                },
                "transforms": []
            }
        }
    ])
}

pub fn apikey_presets() -> Value {
    json!([
        {
            "id": "chatui_think_xml",
            "defaults": {
                "max_multiplier": null,
                "transforms": [
                    {
                        "transform": "reasoning_to_think_xml",
                        "enabled": true,
                        "models": null,
                        "phase": "response",
                        "config": { "tag": "think" }
                    }
                ]
            }
        },
        {
            "id": "raw_passthrough",
            "defaults": {
                "max_multiplier": null,
                "transforms": []
            }
        }
    ])
}

#[cfg(test)]
mod tests {
    use super::provider_presets;

    #[test]
    fn provider_presets_include_gemini_and_grok() {
        let presets = provider_presets();
        let arr = presets.as_array().expect("provider presets array");

        let gemini = arr
            .iter()
            .find(|p| p.get("id").and_then(|v| v.as_str()) == Some("google_gemini"))
            .expect("google_gemini preset");
        assert_eq!(
            gemini["defaults"]["base_url"].as_str(),
            Some("https://generativelanguage.googleapis.com/v1beta")
        );

        let grok = arr
            .iter()
            .find(|p| p.get("id").and_then(|v| v.as_str()) == Some("xai_grok"))
            .expect("xai_grok preset");
        assert_eq!(
            grok["defaults"]["base_url"].as_str(),
            Some("https://api.x.ai")
        );
    }
}
