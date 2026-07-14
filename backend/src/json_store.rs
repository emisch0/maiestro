//! Atomic JSON file writes shared by the `~/.maiestro/` config/state stores.
//!
//! `std::fs::write` truncates the destination before writing, so a crash
//! mid-write leaves a truncated (invalid) file. For repo settings that then makes
//! `load_validated` fail loudly and *blocks spawn/teardown* until the file is
//! hand-repaired. Writing to a sibling temp file and renaming over the
//! destination makes the replacement atomic — a reader ever sees either the old
//! or the new complete file, never a half-written one. (`status.rs` already did
//! this by hand; this collapses the four settings/session save fns onto one impl.)

use std::path::Path;

/// Serialize `value` as pretty JSON and write it to `path` atomically (temp file
/// in the same directory, then rename). Creates the parent directory if needed.
///
/// The temp file is `<path>.tmp`; callers that could write the *same* path
/// concurrently must serialize those writes (e.g. behind a mutex), which the
/// settings stores do. Same-directory temp keeps the rename on one filesystem so
/// it stays atomic.
pub fn write_json_atomic<T: serde::Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_vec_pretty(value)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, &data)?;
    std::fs::rename(&tmp, path)
}
