use std::process::Command;

fn main() {
    // Build metadata shown in Settings → Preferences → About (issue #126). Both
    // are compile-time stamps, so the running app needs no network call and no
    // file next to the binary to know when it was built.
    //
    // Note: `tauri_build::build()` emits its own `cargo:rerun-if-changed`
    // directives, which replace cargo's "rerun when any package file changed"
    // default — so this script does not re-run on every incremental dev build
    // and the stamped date can lag under `tauri dev`. It is always correct for a
    // release build: `scripts/release.sh bump` rewrites `tauri.conf.json` and
    // `Cargo.toml`, both of which force a re-run.
    let date = time::OffsetDateTime::now_utc()
        .format(&time::macros::format_description!("[year]-[month]-[day]"))
        .expect("format build date");
    println!("cargo:rustc-env=MAIESTRO_BUILD_DATE={date}");

    // Short commit of the checkout we were built from, suffixed `-dirty` when the
    // tree had uncommitted changes. Absent when git isn't available or there's no
    // history (a source tarball), in which case `option_env!` yields None and the
    // About row simply omits it.
    if let Some(sha) = git_short_sha() {
        let suffix = if git_tree_is_dirty() { "-dirty" } else { "" };
        println!("cargo:rustc-env=MAIESTRO_GIT_SHA={sha}{suffix}");
    }

    // Whether this binary came out of the release pipeline. `scripts/release.sh
    // build` sets MAIESTRO_RELEASE=1; anything else — `tauri dev`, a hand-run
    // `pnpm tauri build` — is a development build, and the About block labels its
    // version `X.Y.Z+dev` so it is never mistaken for the shipped X.Y.Z.
    //
    // The check can't be "is HEAD tagged vX.Y.Z?": `release.sh` builds *before* it
    // tags (publish creates the tag), so a genuine release build has no tag yet.
    println!("cargo:rerun-if-env-changed=MAIESTRO_RELEASE");
    if std::env::var("MAIESTRO_RELEASE").as_deref() == Ok("1") {
        println!("cargo:rustc-env=MAIESTRO_RELEASE_BUILD=1");
    }

    tauri_build::build()
}

/// Whether the working tree had uncommitted changes at build time. A failed
/// `git status` counts as clean — the suffix is a hint, not a guarantee.
fn git_tree_is_dirty() -> bool {
    Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false)
}

/// `git rev-parse --short HEAD`, or `None` if git fails for any reason.
fn git_short_sha() -> Option<String> {
    let out = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let sha = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if sha.is_empty() {
        None
    } else {
        Some(sha)
    }
}
