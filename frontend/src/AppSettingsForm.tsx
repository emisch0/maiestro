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
import { RevealButton, PathMissingHint, usePathExists } from "./PathField";

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
  const opts: Theme[] = ["light", "dark", "system"];
  return (
    <div className="control jsf-control">
      <label className="jsf-label">Theme</label>
      {description && <div className="jsf-help">{description}</div>}
      <div className="theme-options" role="radiogroup" aria-label="Theme">
        {opts.map((opt, idx) => (
          <button
            key={opt}
            className={`theme-option ${value === opt ? "active" : ""}`}
            role="radio"
            aria-checked={value === opt}
            // Roving tabindex + arrow navigation so the group is one tab stop and
            // the arrow keys move between options like a native radio group.
            tabIndex={value === opt ? 0 : -1}
            onKeyDown={(e) => {
              const dir = e.key === "ArrowRight" || e.key === "ArrowDown" ? 1
                : e.key === "ArrowLeft" || e.key === "ArrowUp" ? -1 : 0;
              if (!dir) return;
              e.preventDefault();
              const next = (idx + dir + opts.length) % opts.length;
              handleChange(path, opts[next]);
              (e.currentTarget.parentElement?.children[next] as HTMLElement | undefined)?.focus();
            }}
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
      {TOOL_FIELDS.map((field) => (
        <ToolPathRow
          key={field.key}
          field={field}
          override={paths[field.key]}
          resolved={resolved.find((t) => t.tool === field.key)}
          onChange={(v) => setPath(field.key, v)}
          onReset={() => handleChange(path, { ...paths, [field.key]: null })}
        />
      ))}
    </div>
  );
}

/** One CLI path override: the input, a reveal button, a reset-to-auto action,
 *  and the resolved-path status. When overridden, the missing-path hint validates
 *  the override; when empty, the existing auto-resolution status line shows. */
function ToolPathRow({
  field,
  override,
  resolved,
  onChange,
  onReset,
}: {
  field: { key: ToolKey; label: string; help: string };
  override: string | null | undefined;
  resolved: ResolvedTool | undefined;
  onChange: (v: string) => void;
  onReset: () => void;
}) {
  const { label: name, help } = field;
  const isOverridden = override != null && override !== "";
  const placeholder = resolved?.exists ? resolved.path : "auto-detect";
  // Validate the override the user typed. When empty, the auto-resolution status
  // line below covers "found / not found", so no separate hint is needed.
  const overrideExists = usePathExists(isOverridden ? (override as string) : "");
  return (
    <div className="jsf-prompt-field">
      <div className="jsf-prompt-head">
        <span className="jsf-prompt-name">{name}</span>
        {isOverridden && (
          <button type="button" className="jsf-prompt-reset" onClick={onReset}>
            Reset to auto
          </button>
        )}
      </div>
      <div className="jsf-help">{help}</div>
      <div className="path-field-row">
        <input
          className="text-input"
          type="text"
          value={override ?? ""}
          placeholder={placeholder}
          onChange={(e) => onChange(e.target.value)}
          spellCheck={false}
          autoCapitalize="off"
          autoCorrect="off"
        />
        <RevealButton
          path={isOverridden ? (override as string) : resolved?.exists ? resolved.path : ""}
          exists={isOverridden ? overrideExists : resolved?.exists ?? false}
        />
      </div>
      {isOverridden ? (
        <PathMissingHint
          path={override}
          exists={overrideExists}
          label="Not found at this path."
        />
      ) : resolved?.exists ? (
        <div className="jsf-help">
          Using <code>{resolved.path}</code>
        </div>
      ) : (
        <div className="jsf-help jsf-tool-missing">
          Not found — set a path above, or install it on your PATH.
        </div>
      )}
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
