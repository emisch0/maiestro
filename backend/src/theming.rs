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
    "#6c3a1a", "#1a6c2a", "#6c1a5a", "#1a5a6c",
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
        "#1a5a6c" => &["🩵", "🐬", "🧊", "🐟"],              // dark cyan
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
