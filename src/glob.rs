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
