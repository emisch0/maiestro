//! The instructions mAIestro Code sends to the repo's agent, and per-repo
//! override resolution (instructions and the per-agent drafting model).
//!
//! Each AI prompt is assembled as `<instruction> + <runtime context>`. The
//! **instruction** is the user-facing, per-repo-configurable part — the schema
//! `default` (see `backend/schemas/repo-settings.schema.json`), or an override
//! from `RepoSettings.prompts`. The **runtime context** (the user's idea, the
//! issue title/body, the diff) is always appended by `spawn.rs` and is never
//! configurable, so an override — even arbitrary text or a `/skill` invocation
//! — can't accidentally drop the data the draft depends on.
//!
//! The default instruction text lives in the JSON Schema only; we read it from
//! there via `repo_settings::schema_default` rather than hardcoding it.

use crate::agent::Agent;
use crate::repo_settings::{schema_default, AgentSettings, PromptOverrides};

/// An override counts only if it has non-whitespace content; otherwise the
/// schema default (addressed by `default_ptr`) is used.
fn pick(override_: &Option<String>, default_ptr: &str) -> String {
    match override_.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => s.to_string(),
        None => schema_default(default_ptr),
    }
}

/// The effective draft-issue instruction for this repo (override or default).
pub fn draft_issue(p: &PromptOverrides) -> String {
    pick(&p.draft_issue, "/properties/prompts/properties/draft_issue/default")
}

/// The effective short-label instruction for this repo (override or default).
pub fn short_label(p: &PromptOverrides) -> String {
    pick(&p.short_label, "/properties/prompts/properties/short_label/default")
}

/// The effective draft-PR instruction for this repo (override or default).
pub fn draft_pr(p: &PromptOverrides) -> String {
    pick(&p.draft_pr, "/properties/prompts/properties/draft_pr/default")
}

/// The effective drafting model for this repo's headless calls on `agent`: the
/// repo's `agent_settings.<agent>.prompt_model` override, or that entry's schema default when
/// unset/empty. `None` means "pass no `--model`" — the Codex default, which
/// defers to the model configured in Codex itself, and the Copilot default,
/// which lets Copilot's automatic routing pick a model the plan offers. Governs only mAIestro Code's
/// own drafting prompts, never the launched worktree session.
pub fn model(settings: &AgentSettings, agent: Agent) -> Option<String> {
    let ptr = format!("/properties/agent_settings/properties/{}/properties/prompt_model/default", agent.as_str());
    Some(pick(settings.prompt_model(agent), &ptr)).filter(|m| !m.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each agent reads its own entry; an unset Claude entry is `haiku`, an unset
    /// Codex entry is no model at all (Codex's configured default), an unset
    /// Antigravity entry is a cheap Gemini Flash id (`agy` has no aliases).
    fn settings(claude: Option<&str>, codex: Option<&str>, antigravity: Option<&str>, copilot: Option<&str>) -> AgentSettings {
        use crate::repo_settings::{AgentCommonSettings, ClaudeSettings};
        let common = |m: Option<&str>| AgentCommonSettings { prompt_model: m.map(Into::into) };
        AgentSettings {
            claude: ClaudeSettings { prompt_model: claude.map(Into::into), remote_control: None },
            codex: common(codex),
            antigravity: common(antigravity),
            copilot: common(copilot),
        }
    }

    #[test]
    fn model_is_chosen_per_agent() {
        let unset = AgentSettings::default();
        assert_eq!(model(&unset, Agent::Claude).as_deref(), Some("haiku"));
        assert_eq!(model(&unset, Agent::Codex), None);
        assert_eq!(model(&unset, Agent::Antigravity).as_deref(), Some("gemini-3.8-flash-low"));
        assert_eq!(model(&unset, Agent::Copilot), None, "Copilot's models depend on the plan");

        let set = settings(Some("sonnet"), Some(" gpt-5-codex "), Some("gemini-3.1-pro-low"), Some("gpt-5-mini"));
        assert_eq!(model(&set, Agent::Claude).as_deref(), Some("sonnet"));
        assert_eq!(model(&set, Agent::Codex).as_deref(), Some("gpt-5-codex"));
        assert_eq!(model(&set, Agent::Antigravity).as_deref(), Some("gemini-3.1-pro-low"));
        assert_eq!(model(&set, Agent::Copilot).as_deref(), Some("gpt-5-mini"));

        let blank = settings(Some("  "), Some(""), Some(" "), Some(" "));
        assert_eq!(model(&blank, Agent::Claude).as_deref(), Some("haiku"));
        assert_eq!(model(&blank, Agent::Codex), None);
        assert_eq!(model(&blank, Agent::Antigravity).as_deref(), Some("gemini-3.8-flash-low"));
        assert_eq!(model(&blank, Agent::Copilot), None);
    }
}
