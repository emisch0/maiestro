// JSON Forms rendering for the global app-settings ("Preferences") panel (issue
// #85). Mirrors RepoSettingsForm: the JSON Schema is the backend's hand-written
// spec (fetched via `app_settings_schema`); this module supplies the UI schema
// (which fields to show, in what order — `window`/`settings_window` are hidden as
// machine-managed) and two custom renderers the schema alone can't express:
//   - Theme: the segmented light/dark/system control (a plain enum would render
//     as a dropdown).
//   - Tool paths: one input per CLI (claude/git/code) with a live resolved-path
//     status line.

import {
  ControlProps,
  rankWith,
  scopeEndsWith,
  UISchemaElement,
} from "@jsonforms/core";
import { withJsonFormsControlProps } from "@jsonforms/react";
import { vanillaRenderers, vanillaCells } from "@jsonforms/vanilla-renderers";
import { ResolvedTool, Theme } from "./api";

/** Field order; `window`/`settings_window` are deliberately omitted (machine-managed). */
export const appSettingsUISchema = {
  type: "VerticalLayout",
  elements: [
    { type: "Control", scope: "#/properties/theme", label: "Theme" },
    { type: "Control", scope: "#/properties/tool_paths", label: "Tool paths" },
  ],
} as unknown as UISchemaElement;

/** Extra data the custom renderers read via JsonForms' `config`. */
export interface AppFormConfig {
  showUnfocusedDescription: true;
  /** How each tool currently resolves (from `tools_resolved`), for the status line. */
  resolvedTools: ResolvedTool[];
}

// ── Theme (segmented control) ────────────────────────────────────────────────
// Stored value is "light" | "dark" | "system"; null is treated as "system".

function ThemeControl(props: ControlProps) {
  const { data, handleChange, path, description } = props;
  const value = (data ?? "system") as Theme;
  return (
    <div className="control jsf-control">
      <label className="jsf-label">Theme</label>
      {description && <div className="jsf-help">{description}</div>}
      <div className="theme-options" role="radiogroup" aria-label="Theme">
        {(["light", "dark", "system"] as Theme[]).map((opt) => (
          <button
            key={opt}
            className={`theme-option ${value === opt ? "active" : ""}`}
            role="radio"
            aria-checked={value === opt}
            onClick={() => handleChange(path, opt)}
          >
            {opt === "light" ? "Light" : opt === "dark" ? "Dark" : "System"}
          </button>
        ))}
      </div>
      <p className="session-hint" style={{ paddingTop: 2 }}>
        System follows your macOS appearance.
      </p>
    </div>
  );
}

export const themeTester = rankWith(20, scopeEndsWith("theme"));
export const ThemeRenderer = withJsonFormsControlProps(ThemeControl);

// ── Tool paths ───────────────────────────────────────────────────────────────
// One input per directly-invoked CLI. Empty = auto-resolve; the resolved path (or
// "Not found") is shown beneath each input, so it's clear what an empty field
// falls back to and whether the current setting actually points at a real binary.

type ToolKey = "claude" | "git" | "code";

const TOOL_FIELDS: { key: ToolKey; label: string; help: string }[] = [
  { key: "claude", label: "claude", help: "Used for AI drafting of issues, labels, and PRs." },
  { key: "git", label: "git", help: "Used for worktree creation and branch checks." },
  { key: "code", label: "code", help: "The VS Code CLI, used to open spawned worktrees." },
];

function ToolPathsControl(props: ControlProps) {
  const { data, handleChange, path, label, description, config } = props;
  const paths = (data ?? {}) as Partial<Record<ToolKey, string | null>>;
  const resolved: ResolvedTool[] = config?.resolvedTools ?? [];

  const setPath = (key: ToolKey, v: string) => {
    const next = v.trim() === "" ? null : v;
    handleChange(path, { ...paths, [key]: next });
  };

  return (
    <div className="control jsf-control">
      <label className="jsf-label">{label}</label>
      <div className="jsf-help">
        {description ??
          "Explicit paths for the CLIs mAIestro runs itself. Leave empty to auto-detect; set one if the app can't find a tool."}
      </div>
      {TOOL_FIELDS.map(({ key, label: name, help }) => {
        const override = paths[key];
        const isOverridden = override != null && override !== "";
        const r = resolved.find((t) => t.tool === key);
        const placeholder = r?.exists ? r.path : "auto-detect";
        return (
          <div key={key} className="jsf-prompt-field">
            <div className="jsf-prompt-head">
              <span className="jsf-prompt-name">{name}</span>
              {isOverridden && (
                <button
                  type="button"
                  className="jsf-prompt-reset"
                  onClick={() => handleChange(path, { ...paths, [key]: null })}
                >
                  Reset to auto
                </button>
              )}
            </div>
            <div className="jsf-help">{help}</div>
            <input
              className="text-input"
              type="text"
              value={override ?? ""}
              placeholder={placeholder}
              onChange={(e) => setPath(key, e.target.value)}
              spellCheck={false}
              autoCapitalize="off"
              autoCorrect="off"
            />
            {!isOverridden &&
              (r?.exists ? (
                <div className="jsf-help">
                  Using <code>{r.path}</code>
                </div>
              ) : (
                <div className="jsf-help jsf-tool-missing">
                  Not found — set a path above, or install it on your PATH.
                </div>
              ))}
          </div>
        );
      })}
    </div>
  );
}

export const toolPathsTester = rankWith(20, scopeEndsWith("tool_paths"));
export const ToolPathsRenderer = withJsonFormsControlProps(ToolPathsControl);

// Custom renderers first so they out-rank the vanilla defaults for their scopes.
export const appSettingsRenderers = [
  { tester: themeTester, renderer: ThemeRenderer },
  { tester: toolPathsTester, renderer: ToolPathsRenderer },
  ...vanillaRenderers,
];

export const appSettingsCells = vanillaCells;
