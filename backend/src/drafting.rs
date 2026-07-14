//! AI drafting: turn a free-text idea into a GitHub issue (title + body + short
//! label), and compress an existing issue into a short session label — all via
//! headless `claude -p` calls.
//!
//! These calls run `claude` in the repo for context but with `--tools ""` (no
//! file/Bash/edit/fetch access), so prompt injection from the input or repo files
//! is contained to, at worst, a bad title the user reviews — never code execution.
//! `ClaudeActivity` correlates a run with the UI action that started it so its
//! busy glow can switch to the rainbow (AI) variant. Extracted from `spawn.rs`
//! (issue #99).

use std::path::Path;

use crate::naming::{default_short_title, trim_to_word};
use crate::plugins::GitHub;
use crate::repo_context::{repo_context, validated_cloned_repo};
use crate::tools::snippet;

/// Pull the inner `{title, body, short_title}` out of claude's reply text (which
/// may wrap it in code fences or prose) and extract a non-empty title, body, and
/// a short branch-friendly label. Falls back to the title for `short_title` when
/// the model omits it.
pub fn parse_issue_draft(text: &str) -> Result<(String, String, String), String> {
    let braces = text.find('{').zip(text.rfind('}')).filter(|(s, e)| e > s);
    let Some((start, end)) = braces else {
        // No JSON object: claude replied conversationally (e.g. asking the user
        // to clarify a vague idea). Surface that reply verbatim so the caller
        // can show it and let the user decide.
        return Err(text.trim().chars().take(400).collect());
    };
    let v: serde_json::Value = serde_json::from_str(&text[start..=end])
        .map_err(|e| format!("could not parse claude reply as JSON ({e}): {}", snippet(text)))?;
    let title = v["title"].as_str().unwrap_or("").trim().to_string();
    let body = v["body"].as_str().unwrap_or("").trim().to_string();
    if title.is_empty() {
        return Err("claude returned an empty title".into());
    }
    let short_title = v["short_title"].as_str().unwrap_or("").trim().to_string();
    let short_title = if short_title.is_empty() { default_short_title(&title) } else { short_title };
    Ok((title, body, short_title))
}

/// Correlates a Claude run with the UI action that initiated it. The frontend
/// generates a `request_id` per command invocation and listens for the
/// `claude-activity` event: while a run is in flight for that id, the action's
/// busy glow switches to the rainbow (AI) variant, and back to the monochrome
/// one when it ends — so a mixed script/AI action changes color mid-flight.
pub struct ClaudeActivity {
    app: tauri::AppHandle,
    request_id: String,
}

impl ClaudeActivity {
    pub fn new(app: tauri::AppHandle, request_id: String) -> Self {
        Self { app, request_id }
    }

    /// Emit `active: true` now and `active: false` when the returned guard
    /// drops — every exit path (success, error, cancellation) ends the
    /// activity, so a failed draft never leaves a button stuck rainbow.
    fn begin(&self) -> ClaudeActivityGuard<'_> {
        self.emit(true);
        ClaudeActivityGuard(self)
    }

    fn emit(&self, active: bool) {
        use tauri::Emitter;
        let _ = self.app.emit(
            "claude-activity",
            serde_json::json!({ "request_id": self.request_id, "active": active }),
        );
    }
}

struct ClaudeActivityGuard<'a>(&'a ClaudeActivity);

impl Drop for ClaudeActivityGuard<'_> {
    fn drop(&mut self) {
        self.0.emit(false);
    }
}

/// Run Claude headlessly with `prompt`, in `dir` for repo context, and return its
/// reply text. Uses `--output-format json` so we parse a stable envelope rather
/// than guessing at raw text, and surfaces stdout/stderr in errors when something
/// goes wrong.
///
/// `--tools ""` disables ALL tools: these calls only need to generate text, so
/// the model must not be able to read arbitrary files, run Bash, edit, or fetch
/// URLs — even though it runs in the real cloned repo/worktree with the user's
/// ambient permissions. That contains prompt injection from the input text (or
/// from repo files like CLAUDE.md, which is still loaded as context) to, at
/// worst, a bad title/body the user reviews — not code execution or exfiltration.
pub async fn claude_text(
    dir: &Path,
    prompt: &str,
    model: &str,
    what: &str,
    activity: Option<&ClaudeActivity>,
) -> Result<String, String> {
    let _active = activity.map(|a| a.begin());
    let run = crate::tools::tokio_command("claude")
        .current_dir(dir)
        .args(["-p", prompt, "--model", model, "--output-format", "json", "--tools", ""])
        // Kill the probe if the timeout below fires — a dropped future must not
        // leave a headless claude burning quota in the background.
        .kill_on_drop(true)
        .output();
    let output = tokio::time::timeout(std::time::Duration::from_secs(90), run)
        .await
        .map_err(|_| format!("claude timed out while {what}"))?
        .map_err(|e| format!("could not run claude (is it installed and on PATH?): {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        return Err(format!("claude exited with an error: {}", snippet(&stderr)));
    }

    // `--output-format json` wraps the reply in a result envelope.
    let envelope: serde_json::Value = serde_json::from_str(stdout.trim()).map_err(|e| {
        format!("could not parse claude output ({e}); stdout: {}; stderr: {}", snippet(&stdout), snippet(&stderr))
    })?;
    if envelope["is_error"].as_bool().unwrap_or(false) {
        return Err(format!("claude reported an error: {}", snippet(envelope["result"].as_str().unwrap_or(""))));
    }
    Ok(envelope["result"].as_str().unwrap_or("").to_string())
}

/// Ask Claude, running in the cloned repo for context, to turn the user's
/// free-text idea into an issue title + markdown body + a short label. The
/// `short_title` is produced in the *same* call (no extra Claude run): it's a
/// punchy branch/session label, distinct from the full issue title.
async fn draft_issue(
    cloned_repo: &Path,
    idea: &str,
    instruction: &str,
    model: &str,
    activity: &ClaudeActivity,
) -> Result<(String, String, String), String> {
    // Instruction (default or per-repo override) first; the idea is appended
    // here so an override can't drop it. See prompts.rs.
    let prompt = format!("{instruction}\n\nIdea: {idea}");
    let reply = claude_text(cloned_repo, &prompt, model, "drafting the issue", Some(activity)).await?;
    parse_issue_draft(&reply)
}

/// Pull a usable short label out of Claude's reply to `suggest_short_label`.
/// The prompt asks for the bare label, but the model may still wrap it in
/// quotes, backticks, or a code fence — strip those. A multi-line or over-long
/// reply means it rambled instead of labeling: error, the caller keeps the
/// heuristic label.
fn parse_short_label(text: &str) -> Result<String, String> {
    let mut lines = text.trim().lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with("```"));
    let Some(line) = lines.next() else {
        return Err("claude returned an empty label".into());
    };
    if lines.next().is_some() {
        return Err(format!("claude replied with prose, not a label: {}", snippet(text)));
    }
    let label = line.trim_matches(|c| matches!(c, '"' | '\'' | '`')).trim();
    let label = label.trim_end_matches(|c: char| !c.is_alphanumeric()).trim();
    if label.is_empty() {
        return Err("claude returned an empty label".into());
    }
    if label.chars().count() > 60 {
        return Err(format!("claude's label is too long: {}", snippet(label)));
    }
    Ok(label.to_string())
}

/// Ask Claude, running in the cloned repo for context, to compress an existing
/// issue's title + body into a short session label. The spawn preview opens
/// immediately with the heuristic label and swaps this in when it arrives; any
/// error here just leaves the heuristic in place.
async fn suggest_short_label(
    cloned_repo: &Path,
    title: &str,
    body: &str,
    instruction: &str,
    model: &str,
) -> Result<String, String> {
    // Issue bodies can be arbitrarily long; the label only needs the gist.
    let body: String = body.chars().take(4000).collect();
    // Instruction (default or per-repo override) first; the issue text is
    // appended here so an override can't drop it. See prompts.rs.
    let prompt = format!("{instruction}\n\nIssue title: {title}\n\nIssue body:\n{body}");
    // No activity signal: this is a fire-and-forget label swap with no busy
    // element in the UI to color.
    let reply = claude_text(cloned_repo, &prompt, model, "summarizing the issue", None).await?;
    parse_short_label(&reply)
}

/// Suggest an AI short label for an existing issue (title + body via Claude).
/// Called fire-and-forget by the spawn preview after it opens with the
/// heuristic label; the frontend swallows errors, so failures here are benign.
#[tauri::command]
pub async fn suggest_short_title(repo: String, issue_number: u64) -> Result<String, String> {
    crate::log_invoke!("suggest_short_title", repo = %repo, issue = issue_number);
    let (settings, gh) = repo_context(&repo).await?;
    let cloned_repo = validated_cloned_repo(&settings)?;
    let (issue_title, _issue_url, issue_body) = crate::spawn::issue_facts(&gh, &repo, issue_number).await?;
    let instruction = crate::prompts::short_label(&settings.prompts);
    let model = crate::prompts::model(&settings.prompt_model);
    suggest_short_label(&cloned_repo, &issue_title, &issue_body, &instruction, &model).await
}

/// A drafted issue ready to create, or a signal that Claude couldn't produce a
/// clear draft and the user must confirm creating from raw text.
pub enum DraftStep {
    Ready {
        title: String,
        body: String,
        /// Punchy branch/session label produced in the same draft call.
        short_title: String,
        /// Non-fatal note to surface alongside the created issue.
        warning: Option<String>,
    },
    NeedsConfirmation { message: String },
}

/// Resolve the repo's identity + cloned repo, then turn the idea into an issue
/// draft — via Claude, or (when `use_raw_fallback`) straight from the raw text.
/// Returns the authenticated GitHub client alongside the draft so callers can
/// create the issue. Shared by `create_issue`, `create_issue_and_spawn`, and
/// `draft_spawn_preview`.
pub async fn resolve_draft(
    repo: &str,
    idea: &str,
    use_raw_fallback: bool,
    activity: &ClaudeActivity,
) -> Result<(GitHub, DraftStep), String> {
    let idea = idea.trim();
    if idea.is_empty() {
        return Err("Describe what you want to work on first.".into());
    }

    let (settings, gh) = repo_context(repo).await?;
    let cloned_repo = validated_cloned_repo(&settings)?;

    let step = if use_raw_fallback {
        let title = trim_to_word(idea, 70);
        let short_title = default_short_title(&title);
        DraftStep::Ready {
            title,
            body: idea.to_string(),
            short_title,
            warning: Some("created from your text without an AI draft".to_string()),
        }
    } else {
        let instruction = crate::prompts::draft_issue(&settings.prompts);
        let model = crate::prompts::model(&settings.prompt_model);
        match draft_issue(&cloned_repo, idea, &instruction, &model, activity).await {
            Ok((title, body, short_title)) => DraftStep::Ready { title, body, short_title, warning: None },
            // Couldn't draft: let the user confirm before creating anything.
            Err(message) => DraftStep::NeedsConfirmation { message },
        }
    };
    Ok((gh, step))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bare label parses; quote/backtick/fence wrapping and trailing
    /// punctuation are stripped.
    #[test]
    fn short_label_parses_and_unwraps() {
        assert_eq!(parse_short_label("Auth token refresh").unwrap(), "Auth token refresh");
        assert_eq!(parse_short_label("  \"Auth token refresh.\"  ").unwrap(), "Auth token refresh");
        assert_eq!(parse_short_label("`Resizable popover`").unwrap(), "Resizable popover");
        assert_eq!(parse_short_label("```\nAI workspace labels\n```").unwrap(), "AI workspace labels");
    }

    /// Empty, multi-line (prose), and over-long replies are rejected — the
    /// caller falls back to the heuristic label.
    #[test]
    fn short_label_rejects_prose() {
        assert!(parse_short_label("").is_err());
        assert!(parse_short_label("\"\"").is_err());
        assert!(parse_short_label("Here are some options:\n- Auth refresh\n- Token renewal").is_err());
        assert!(parse_short_label(&"long ".repeat(20)).is_err());
    }

    /// A JSON draft (bare or fenced) yields title/body/short_title; a missing
    /// short_title falls back to a heuristic from the title; no-JSON prose and an
    /// empty title are rejected.
    #[test]
    fn parse_issue_draft_extracts_or_falls_back() {
        let (t, b, s) = parse_issue_draft(r#"{"title":"Add foo","body":"Do it","short_title":"foo"}"#).unwrap();
        assert_eq!((t.as_str(), b.as_str(), s.as_str()), ("Add foo", "Do it", "foo"));

        // Fenced + prose around it, and a missing short_title → derived from title.
        let (t, _b, s) = parse_issue_draft("Sure!\n```json\n{\"title\":\"Fix the bug!\",\"body\":\"x\"}\n```").unwrap();
        assert_eq!(t, "Fix the bug!");
        assert_eq!(s, "Fix the bug");

        // No JSON at all: the conversational reply is surfaced as the error.
        assert!(parse_issue_draft("Can you clarify what you mean?").is_err());
        // JSON present but empty title.
        assert!(parse_issue_draft(r#"{"title":"","body":"x"}"#).is_err());
    }
}
