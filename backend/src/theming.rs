//! Deterministic worktree title-bar theming: pick a palette color (and a matching
//! emoji) for a spawned workspace.
//!
//! `pick_theme` seeds off the workspace name so the same name always themes the
//! same, and avoids colors already claimed by tracked sessions where it can.
//! Extracted from `spawn.rs` (issue #99).

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

const PALETTE: &[&str] = &[
    "#1a3a6c", "#4a1a6c", "#1a6c5a", "#6c1a1a",
    "#6c3a1a", "#1a6c2a", "#6c1a5a", "#6c5a1a",
];

/// Emoji options per palette color — one is chosen (deterministically) per spawn.
fn palette_emojis(color: &str) -> &'static [&'static str] {
    match color {
        "#1a3a6c" => &["🔵", "🌊", "🫐", "🦋", "🧊", "💙"], // navy blue
        "#4a1a6c" => &["🟣", "🔮", "💜"],                    // dark purple
        "#1a6c5a" => &["🐢", "🌴", "🐠", "🍃", "🦚"],        // dark teal
        "#6c1a1a" => &["🔴", "🍒", "🌶️", "🦞"],             // dark red
        "#6c3a1a" => &["🟠", "🦊", "🍊", "🦁"],              // dark orange
        "#1a6c2a" => &["🌿", "🐸", "🍀", "🐊"],              // dark green
        "#6c1a5a" => &["🔮", "🎀"],                          // dark magenta
        "#6c5a1a" => &["🟡", "🍋", "🌻", "🐝"],              // dark yellow
        _ => &["🔵"],
    }
}

/// Choose a palette color not already claimed by a tracked session (falling back
/// to the full palette when all are taken), then an emoji within it. Seeded by
/// `seed` so the same workspace name themes consistently.
pub fn pick_theme(seed: &str) -> (&'static str, &'static str) {
    let used = crate::sessions::used_colors();
    let pool: Vec<&'static str> = PALETTE.iter().copied().filter(|c| !used.contains(&c.to_string())).collect();
    let pool: Vec<&'static str> = if pool.is_empty() { PALETTE.to_vec() } else { pool };
    let color = pool[hash_index(seed, 1, pool.len())];
    let emojis = palette_emojis(color);
    let emoji = emojis[hash_index(seed, 2, emojis.len())];
    (color, emoji)
}

/// The Claude Code session color matching a palette hex — one of the eight names
/// `/color` (and the session UI) accepts. The palette is sized and ordered so this
/// is a bijection: every palette entry gets its own Claude color and none is
/// wasted, so two live sessions never collide in the session UI while a distinct
/// color goes unused.
///
/// Keyed off the *stored* hex rather than a fresh `pick_theme`, so a session
/// recorded before a palette change still resolves — which is why `#1a5a6c`
/// (a dark cyan retired from `PALETTE` in favour of the dark yellow) still maps.
/// Anything unrecognized falls back to `default`, Claude's "no color" value.
pub fn claude_color(hex: &str) -> &'static str {
    match hex {
        "#1a3a6c" => "blue",
        "#4a1a6c" => "purple",
        "#1a6c5a" => "cyan",
        "#6c1a1a" => "red",
        "#6c3a1a" => "orange",
        "#1a6c2a" => "green",
        "#6c1a5a" => "pink",
        "#6c5a1a" => "yellow",
        // Retired palette entries, kept so existing session records still theme.
        "#1a5a6c" => "cyan",
        _ => "default",
    }
}

/// Deterministic index into a list of `len`, derived from `s` and a `salt`.
/// DefaultHasher::new() is fixed-seeded, so the same name always themes the same.
fn hash_index(s: &str, salt: u64, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let mut h = DefaultHasher::new();
    salt.hash(&mut h);
    s.hash(&mut h);
    (h.finish() % len as u64) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The eight names Claude Code's `/color` accepts (plus `default`).
    const CLAUDE_COLORS: &[&str] =
        &["red", "blue", "green", "yellow", "purple", "orange", "pink", "cyan"];

    /// Every palette entry maps to a *distinct* Claude color drawn from the
    /// accepted set — a bijection, so no two concurrently themed sessions show
    /// the same session color and none of Claude's colors goes unused.
    #[test]
    fn palette_maps_bijectively_onto_claude_colors() {
        let mut mapped: Vec<&str> = PALETTE.iter().map(|hex| claude_color(hex)).collect();
        for name in &mapped {
            assert!(CLAUDE_COLORS.contains(name), "{name} is not a Claude color");
        }
        mapped.sort_unstable();
        mapped.dedup();
        assert_eq!(mapped.len(), PALETTE.len(), "two palette colors share a Claude color");
        assert_eq!(mapped.len(), CLAUDE_COLORS.len(), "a Claude color is unused");
    }

    /// Every palette entry has its own emoji arm (none falls through to the
    /// default), so a new palette color can't silently ship with the wrong emoji.
    #[test]
    fn every_palette_color_has_emojis() {
        for hex in PALETTE {
            assert_ne!(palette_emojis(hex), &["\u{1f535}"], "{hex} has no emoji arm");
        }
    }

    /// A color recorded before it was retired from the palette still resolves,
    /// so reopening an older worktree themes it rather than clearing the color.
    #[test]
    fn retired_palette_color_still_maps() {
        assert_eq!(claude_color("#1a5a6c"), "cyan");
        assert_eq!(claude_color("#nonsense"), "default");
    }
}
