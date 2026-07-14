//! Pure text helpers for deriving workspace/branch names and short labels from
//! (possibly long, messy) issue titles.
//!
//! `slugify` builds the branch-safe slug, `default_short_title` a human short
//! label, and `trim_to_word` the word-boundary truncation both rely on. Extracted
//! from `spawn.rs` (issue #99) so the spawn core and the AI-drafting path share
//! one implementation — and so these pure functions are unit-tested in one place.

/// Trim `s` to at most `max` chars, ending at a word boundary when one fits.
pub fn trim_to_word(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max).collect();
    match cut.rfind(' ') {
        Some(i) if i > 0 => cut[..i].trim_end().to_string(),
        _ => cut.trim_end().to_string(),
    }
}

/// A reasonable default short label from a (possibly long) issue title when no
/// AI-generated or user-edited label is available: trimmed to a word boundary
/// and stripped of trailing punctuation. Never empty.
pub fn default_short_title(title: &str) -> String {
    let s = trim_to_word(title, 50);
    let s = s.trim_end_matches(|c: char| !c.is_alphanumeric()).trim();
    if s.is_empty() { "work".to_string() } else { s.to_string() }
}

/// Turn a title into a `-`-separated, lowercase, alphanumeric slug capped at
/// `max_len`, breaking at a `-` boundary when the cap falls mid-word.
pub fn slugify(title: &str, max_len: usize) -> String {
    let mut slug = String::new();
    let mut prev_dash = false;
    for c in title.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
            prev_dash = false;
        } else if !prev_dash {
            slug.push('-');
            prev_dash = true;
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.len() <= max_len {
        return slug;
    }
    let cut = &slug[..max_len];
    match cut.rfind('-') {
        Some(i) if i > 0 => cut[..i].trim_end_matches('-').to_string(),
        _ => cut.trim_end_matches('-').to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trim_to_word_breaks_on_word_boundary() {
        assert_eq!(trim_to_word("short", 50), "short");
        assert_eq!(trim_to_word("  padded  ", 50), "padded");
        // Cuts back to the last whole word that fits.
        assert_eq!(trim_to_word("one two three four", 9), "one two");
        // No space in range: hard cut.
        assert_eq!(trim_to_word("supercalifragilistic", 5), "super");
    }

    #[test]
    fn default_short_title_strips_trailing_punctuation_and_never_empties() {
        assert_eq!(default_short_title("Fix the login bug!"), "Fix the login bug");
        assert_eq!(default_short_title("   "), "work");
        assert_eq!(default_short_title("...!!!"), "work");
        // Long titles are word-trimmed at 50 chars.
        let long = "Add a really thorough and quite verbose configuration option here";
        assert!(default_short_title(long).chars().count() <= 50);
    }

    #[test]
    fn slugify_lowercases_hyphenates_and_caps() {
        assert_eq!(slugify("Add Foo Bar", 25), "add-foo-bar");
        assert_eq!(slugify("  Multiple   spaces & symbols!! ", 25), "multiple-spaces-symbols");
        // Cap breaks at a dash boundary rather than mid-word.
        assert_eq!(slugify("alpha beta gamma delta", 12), "alpha-beta");
        assert_eq!(slugify("!!!", 25), "");
    }
}
