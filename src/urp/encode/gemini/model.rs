//! Model-family facts the encoder derives from the upstream model id (PG4b, PG4b1, PG4f, PG4i,
//! PG4j).

use crate::urp::ReasoningConfig;
use serde_json::{Map, Value, json};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ThinkingLevel {
    Minimal,
    Low,
    Medium,
    High,
}

impl ThinkingLevel {
    fn wire(self) -> &'static str {
        match self {
            ThinkingLevel::Minimal => "MINIMAL",
            ThinkingLevel::Low => "LOW",
            ThinkingLevel::Medium => "MEDIUM",
            ThinkingLevel::High => "HIGH",
        }
    }
}

const ALL_LEVELS: &[ThinkingLevel] = &[
    ThinkingLevel::Minimal,
    ThinkingLevel::Low,
    ThinkingLevel::Medium,
    ThinkingLevel::High,
];
const LEVELS_WITHOUT_MINIMAL: &[ThinkingLevel] = &[
    ThinkingLevel::Low,
    ThinkingLevel::Medium,
    ThinkingLevel::High,
];
const MINIMAL_OR_HIGH: &[ThinkingLevel] = &[ThinkingLevel::Minimal, ThinkingLevel::High];
const MINIMAL_MEDIUM_HIGH: &[ThinkingLevel] = &[
    ThinkingLevel::Minimal,
    ThinkingLevel::Medium,
    ThinkingLevel::High,
];

enum ThinkingTarget {
    Lowest,
    Level(ThinkingLevel),
    Highest,
}

enum ThinkingProfile {
    /// The model has no thinking controls; typed reasoning is not encoded.
    Unsupported,
    /// Gemini 2.5-style integer budget.
    Budget {
        min: u64,
        max: u64,
        can_disable: bool,
    },
    /// Gemini 3+/4 thinkingLevel; the slice lists supported levels in ascending order.
    Levels(&'static [ThinkingLevel]),
}

pub(super) struct GeminiModel {
    thinking: ThinkingProfile,
    /// Gemini 3+ validates current-turn function-call signatures.
    pub(super) requires_call_signatures: bool,
    /// Per-Part `mediaResolution` exists for Gemini 3+.
    pub(super) media_resolution: bool,
    pub(super) image_output: bool,
    /// Documented `imageConfig.imageSize` tiers in ascending order; empty means 1K only.
    image_sizes: &'static [&'static str],
    /// The model documents the 1:4, 4:1, 1:8 and 8:1 aspect ratios.
    extended_ratios: bool,
}

impl GeminiModel {
    pub(super) fn parse(model: &str) -> Self {
        let name = model
            .trim()
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let version = name.strip_prefix("gemini-").and_then(|rest| {
            let mut numbers = rest.split('-').next()?.split('.');
            let major = numbers.next()?.parse::<u32>().ok()?;
            let minor = match numbers.next() {
                Some(minor) => minor.parse::<u32>().ok()?,
                None => 0,
            };
            Some((major, minor))
        });
        let image_output = name.contains("image") || name.contains("nano-banana");
        let thinking = if name.contains("robotics-er-1") {
            ThinkingProfile::Budget {
                min: 0,
                max: 24576,
                can_disable: true,
            }
        } else if name.starts_with("gemma") || name.starts_with("learnlm") {
            ThinkingProfile::Unsupported
        } else if name.contains("nano-banana") {
            ThinkingProfile::Levels(MINIMAL_MEDIUM_HIGH)
        } else {
            match version {
                Some(version) if version < (2, 5) => ThinkingProfile::Unsupported,
                // Gemini 2.5 image models document thinking as not supported.
                Some((2, _)) if image_output => ThinkingProfile::Unsupported,
                Some((2, _)) if name.contains("pro") => ThinkingProfile::Budget {
                    min: 128,
                    max: 32768,
                    can_disable: false,
                },
                Some((2, _)) if name.contains("flash-lite") => ThinkingProfile::Budget {
                    min: 512,
                    max: 24576,
                    can_disable: true,
                },
                Some((2, _)) => ThinkingProfile::Budget {
                    min: 0,
                    max: 24576,
                    can_disable: true,
                },
                Some((3, _))
                    if name.contains("flash-lite-image") || name.contains("flash-image") =>
                {
                    ThinkingProfile::Levels(MINIMAL_OR_HIGH)
                }
                Some((3, minor)) if name.contains("pro") || minor == 8 => {
                    ThinkingProfile::Levels(LEVELS_WITHOUT_MINIMAL)
                }
                Some((3, _)) => ThinkingProfile::Levels(ALL_LEVELS),
                _ if name.contains("robotics-er-2") => ThinkingProfile::Levels(ALL_LEVELS),
                // Gemini 4, `-latest` aliases and unknown ids: the level set every current
                // generation accepts.
                _ => ThinkingProfile::Levels(LEVELS_WITHOUT_MINIMAL),
            }
        };
        let current = matches!(thinking, ThinkingProfile::Levels(_));
        // Image generation guide: 2.5 Flash Image and 3.1 Flash Lite Image output 1K only;
        // 3.1 Flash Image adds 512; Nano Banana 2.1 and 3 Pro Image offer 1K/2K/4K.
        let flash_image = name.contains("flash-image") && !name.contains("flash-lite-image");
        let image_sizes: &'static [&'static str] = if !image_output
            || name.contains("flash-lite-image")
            || matches!(version, Some((2, _)))
        {
            &[]
        } else if flash_image {
            &["512", "1K", "2K", "4K"]
        } else {
            &["1K", "2K", "4K"]
        };
        Self {
            thinking,
            requires_call_signatures: current,
            media_resolution: current,
            image_output,
            image_sizes,
            extended_ratios: (flash_image && !matches!(version, Some((2, _))))
                || name.contains("nano-banana"),
        }
    }

    /// The typed `thinkingConfig` members for this model family.
    pub(super) fn thinking_config(&self, reasoning: &ReasoningConfig) -> Map<String, Value> {
        let mut config = Map::new();
        let dynamic = matches!(reasoning.mode.as_deref(), Some("adaptive" | "enabled"));
        match self.thinking {
            ThinkingProfile::Unsupported => return config,
            ThinkingProfile::Budget {
                min,
                max,
                can_disable,
            } => {
                let off = if can_disable { 0 } else { min };
                let clamp = |budget: u64| budget.clamp(min.max(1), max);
                let budget = if reasoning.disabled() {
                    Some(off as i64)
                } else if let Some(budget) = reasoning.budget_tokens {
                    Some(if budget == 0 { off } else { clamp(budget) } as i64)
                } else if let Some(effort) = reasoning.effort.as_deref() {
                    effort_budget(effort).map(|budget| clamp(budget) as i64)
                } else if dynamic {
                    Some(-1)
                } else {
                    None
                };
                if let Some(budget) = budget {
                    config.insert("thinkingBudget".into(), json!(budget));
                }
            }
            ThinkingProfile::Levels(levels) => {
                let target = if reasoning.disabled() {
                    Some(ThinkingTarget::Lowest)
                } else if let Some(budget) = reasoning.budget_tokens {
                    Some(match budget {
                        0 => ThinkingTarget::Lowest,
                        1..=1024 => ThinkingTarget::Level(ThinkingLevel::Low),
                        1025..=8192 => ThinkingTarget::Level(ThinkingLevel::Medium),
                        _ => ThinkingTarget::Level(ThinkingLevel::High),
                    })
                } else {
                    match reasoning.effort.as_deref() {
                        Some("minimal") => Some(ThinkingTarget::Level(ThinkingLevel::Minimal)),
                        Some("low") => Some(ThinkingTarget::Level(ThinkingLevel::Low)),
                        Some("medium") => Some(ThinkingTarget::Level(ThinkingLevel::Medium)),
                        Some("high") => Some(ThinkingTarget::Level(ThinkingLevel::High)),
                        Some("xhigh" | "max") => Some(ThinkingTarget::Highest),
                        // Adaptive or unknown effort keeps the model's default dynamic level.
                        _ => None,
                    }
                };
                if let Some(target) = target {
                    config.insert(
                        "thinkingLevel".into(),
                        json!(select_level(levels, target).wire()),
                    );
                }
            }
        }
        let include_thoughts = if let Some(summary) = &reasoning.summary {
            Some(summary != "none")
        } else if reasoning.disabled() {
            None
        } else if dynamic {
            Some(reasoning.display.as_deref() != Some("omitted"))
        } else if reasoning.effort.is_some() || reasoning.budget_tokens.is_some() {
            Some(true)
        } else {
            None
        };
        if let Some(include) = include_thoughts {
            config.insert("includeThoughts".into(), json!(include));
        }
        config
    }

    /// Maps an OpenAI-style `WxH` size to the nearest aspect ratio and the smallest size tier
    /// the model documents for that request (image generation guide tables).
    pub(super) fn image_config(&self, size: &str) -> Option<Map<String, Value>> {
        const RATIOS: [(u32, u32); 14] = [
            (1, 1),
            (2, 3),
            (3, 2),
            (3, 4),
            (4, 3),
            (4, 5),
            (5, 4),
            (9, 16),
            (16, 9),
            (21, 9),
            (1, 4),
            (4, 1),
            (1, 8),
            (8, 1),
        ];
        let ratios = if self.extended_ratios {
            &RATIOS[..]
        } else {
            &RATIOS[..10]
        };
        let (width, height) = size.split_once('x')?;
        let width = width
            .trim()
            .parse::<u32>()
            .ok()
            .filter(|value| *value > 0)?;
        let height = height
            .trim()
            .parse::<u32>()
            .ok()
            .filter(|value| *value > 0)?;
        let target = (f64::from(width) / f64::from(height)).ln();
        let distance = |(w, h): &(u32, u32)| ((f64::from(*w) / f64::from(*h)).ln() - target).abs();
        let (ratio_width, ratio_height) = ratios
            .iter()
            .min_by(|left, right| distance(left).total_cmp(&distance(right)))?;
        let mut config = Map::from_iter([(
            "aspectRatio".to_string(),
            json!(format!("{ratio_width}:{ratio_height}")),
        )]);
        // Longest documented edge per tier across ratios: 512 → 792, 1K → 1584, 2K → 3168.
        let wanted = match width.max(height) {
            0..=792 => "512",
            793..=1584 => "1K",
            1585..=3168 => "2K",
            _ => "4K",
        };
        let tiers = ["512", "1K", "2K", "4K"];
        let rank = |tier: &str| tiers.iter().position(|candidate| *candidate == tier);
        if let Some(tier) = self
            .image_sizes
            .iter()
            .find(|tier| rank(tier) >= rank(wanted))
            .or(self.image_sizes.last())
        {
            config.insert("imageSize".to_string(), json!(tier));
        }
        Some(config)
    }
}

fn effort_budget(effort: &str) -> Option<u64> {
    Some(match effort {
        "minimal" => 512,
        "low" => 1024,
        "medium" => 8192,
        "high" => 24576,
        "xhigh" | "max" => u64::MAX,
        _ => return None,
    })
}

/// Picks the supported level nearest to the target; ties resolve to the higher level.
fn select_level(levels: &[ThinkingLevel], target: ThinkingTarget) -> ThinkingLevel {
    let first = levels[0];
    let last = levels[levels.len() - 1];
    match target {
        ThinkingTarget::Lowest => first,
        ThinkingTarget::Highest => last,
        ThinkingTarget::Level(level) => levels
            .iter()
            .copied()
            .min_by_key(|candidate| {
                let distance = (*candidate as i8 - level as i8).unsigned_abs();
                (distance, std::cmp::Reverse(*candidate))
            })
            .unwrap_or(last),
    }
}
