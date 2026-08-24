//! Shared case-sensitive glob matching for model patterns.
//!
//! Semantics (spec: `urp-transform-system.spec.md` TF-4a and
//! `monoize-upstream-routing.spec.md` api_type_overrides): matching is
//! case-sensitive and anchored to the full value; `*` matches any sequence of
//! zero or more characters; `?` matches exactly one character; every other
//! character matches only itself. Both wildcards match any Unicode scalar
//! value, including newline.
//!
//! This replaces per-call `Regex::new` translation on the routing and stream
//! hot paths: the two-pointer backtracking scan is allocation-light and never
//! compiles a regular expression.

/// Returns true when `value` matches the anchored, case-sensitive glob
/// `pattern` (`*` = any sequence, `?` = exactly one character).
pub fn case_sensitive_glob_match(pattern: &str, value: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    // Char-level indices so `?` consumes one Unicode scalar value, not one byte.
    let pattern: Vec<char> = pattern.chars().collect();
    let value: Vec<char> = value.chars().collect();
    let mut pattern_index = 0;
    let mut value_index = 0;
    // Backtracking bookmarks for the most recent `*`: on a mismatch, retry the
    // suffix after letting the star absorb one more character.
    let mut last_star_index = None;
    let mut last_star_match_index = 0;

    while value_index < value.len() {
        if pattern_index < pattern.len()
            && pattern[pattern_index] != '*'
            && (pattern[pattern_index] == '?' || pattern[pattern_index] == value[value_index])
        {
            pattern_index += 1;
            value_index += 1;
        } else if pattern_index < pattern.len() && pattern[pattern_index] == '*' {
            last_star_index = Some(pattern_index);
            pattern_index += 1;
            last_star_match_index = value_index;
        } else if let Some(star_index) = last_star_index {
            last_star_match_index += 1;
            value_index = last_star_match_index;
            pattern_index = star_index + 1;
        } else {
            return false;
        }
    }

    while pattern_index < pattern.len() && pattern[pattern_index] == '*' {
        pattern_index += 1;
    }
    pattern_index == pattern.len()
}

#[cfg(test)]
mod tests {
    use super::case_sensitive_glob_match;

    #[test]
    fn literal_patterns_are_anchored_and_case_sensitive() {
        assert!(case_sensitive_glob_match("gpt-4o", "gpt-4o"));
        assert!(!case_sensitive_glob_match("gpt-4o", "GPT-4o"));
        assert!(!case_sensitive_glob_match("gpt-4o", "gpt-4o-mini"));
        assert!(!case_sensitive_glob_match("gpt-4o", "my-gpt-4o"));
    }

    #[test]
    fn star_matches_any_sequence_including_empty() {
        assert!(case_sensitive_glob_match("*", ""));
        assert!(case_sensitive_glob_match("*", "anything"));
        assert!(case_sensitive_glob_match("gpt-*", "gpt-"));
        assert!(case_sensitive_glob_match("gpt-*", "gpt-4o-mini"));
        assert!(case_sensitive_glob_match("*-mini", "gpt-4o-mini"));
        assert!(case_sensitive_glob_match("g*o*i", "gpt-4o-mini"));
        assert!(!case_sensitive_glob_match("gpt-*", "claude-3"));
    }

    #[test]
    fn question_mark_matches_exactly_one_character() {
        assert!(case_sensitive_glob_match("gpt-?", "gpt-4"));
        assert!(!case_sensitive_glob_match("gpt-?", "gpt-"));
        assert!(!case_sensitive_glob_match("gpt-?", "gpt-45"));
        assert!(case_sensitive_glob_match("??", "ab"));
    }

    #[test]
    fn wildcards_match_multibyte_characters_and_newlines() {
        assert!(case_sensitive_glob_match("?", "模"));
        assert!(case_sensitive_glob_match("m?del", "m模del"));
        assert!(case_sensitive_glob_match("?", "\n"));
        assert!(case_sensitive_glob_match("a*b", "a\nb"));
    }

    #[test]
    fn regex_metacharacters_are_literal() {
        assert!(case_sensitive_glob_match("a.b", "a.b"));
        assert!(!case_sensitive_glob_match("a.b", "axb"));
        assert!(case_sensitive_glob_match("a+b(c)", "a+b(c)"));
        assert!(!case_sensitive_glob_match("a+b", "aab"));
    }

    #[test]
    fn empty_pattern_matches_only_empty_value() {
        assert!(case_sensitive_glob_match("", ""));
        assert!(!case_sensitive_glob_match("", "x"));
    }

    #[test]
    fn adjacent_stars_and_backtracking_terminate() {
        let value = "a".repeat(512) + "x";
        assert!(case_sensitive_glob_match("*a*a*a*?", &value));
        assert!(!case_sensitive_glob_match("*a*a*a*y", &value));
        assert!(case_sensitive_glob_match("**a***x**", &value));
    }
}
