// JSON Forms rendering for the per-repo settings detail view (issue #57).
//
// The JSON Schema is the backend's hand-written spec, fetched via
// `repo_settings_schema`. This module supplies the *presentation* layer: a
// hand-written UI schema that orders the fields and hides `repo`/`hidden`, plus
// two custom renderers for the cases the schema alone can't express — the
// identity select (dynamic options) and the env-files list (Scan/Add/Remove).
//
// Everything else (checkout dir, worktree prefix) falls through to the vanilla
// string-input renderer, styled in styles.css.

import { useState } from "react";
import {
  ControlProps,
  rankWith,
  scopeEndsWith,
  UISchemaElement,
} from "@jsonforms/core";
import { withJsonFormsControlProps } from "@jsonforms/react";
import { vanillaRenderers, vanillaCells } from "@jsonforms/vanilla-renderers";
import { api } from "./api";

/** Field order, with `repo` and `hidden` deliberately omitted (the latter is
 *  managed from the popover, not this form). */
export const repoSettingsUISchema = {
  type: "VerticalLayout",
  elements: [
    { type: "Control", scope: "#/properties/checkout_dir", label: "Checkout directory" },
    { type: "Control", scope: "#/properties/worktree_prefix", label: "Worktree prefix" },
    { type: "Control", scope: "#/properties/identity_id", label: "Identity" },
    { type: "Control", scope: "#/properties/env_files", label: "Environment files" },
  ],
} as unknown as UISchemaElement;

/** Extra data the custom renderers need, threaded through JsonForms' `config`. */
export interface RepoFormConfig {
  /** Identities offered by the identity select. */
  knownIdentities: string[];
  /** Current checkout dir, so the env-files Scan knows where to look. */
  checkoutDir: string | null;
  /** Always show schema descriptions as help text, not only on focus. */
  showUnfocusedDescription: true;
}

// ── Identity select ─────────────────────────────────────────────────────────
// Options come from the live identity list, which a static schema enum can't
// express, so this replaces the vanilla string input for `identity_id`.

/** Shared header: bold label over a muted description (VS Code settings style),
 *  with the control rendered below by the caller. */
function FieldHeading({ label, description }: { label?: string; description?: string }) {
  return (
    <>
      <label className="jsf-label">{label}</label>
      {description && <div className="jsf-help">{description}</div>}
    </>
  );
}

function IdentityControl(props: ControlProps) {
  const { data, handleChange, path, label, description, config } = props;
  const identities: string[] = config?.knownIdentities ?? [];
  return (
    <div className="control jsf-control">
      <FieldHeading label={label} description={description} />
      {identities.length === 0 ? (
        <p className="session-hint" style={{ paddingTop: 2 }}>
          No identities configured. Go to the Identity tab first.
        </p>
      ) : (
        <select
          className="text-input profile-select"
          value={data ?? ""}
          onChange={(e) => handleChange(path, e.target.value || null)}
        >
          <option value="">— none —</option>
          {identities.map((iid) => (
            <option key={iid} value={iid}>{iid}</option>
          ))}
        </select>
      )}
    </div>
  );
}

export const identityTester = rankWith(20, scopeEndsWith("identity_id"));
export const IdentityRenderer = withJsonFormsControlProps(IdentityControl);

// ── Plain text fields (checkout dir, worktree prefix) ───────────────────────
// A text input with the label/description heading above it. `worktree_prefix`
// surfaces its backend default (`~/src/work-`) as a disabled-looking
// placeholder so it's clear what an empty field resolves to.

const WORKTREE_PREFIX_DEFAULT = "~/src/work-";

function CheckoutDirControl(props: ControlProps) {
  const { data, handleChange, path, label, description } = props;
  return (
    <div className="control jsf-control">
      <FieldHeading label={label} description={description} />
      <input
        className="text-input"
        type="text"
        value={data ?? ""}
        placeholder="~/src/repo-name"
        onChange={(e) => handleChange(path, e.target.value || null)}
        spellCheck={false}
        autoCapitalize="off"
        autoCorrect="off"
      />
    </div>
  );
}

export const checkoutDirTester = rankWith(20, scopeEndsWith("checkout_dir"));
export const CheckoutDirRenderer = withJsonFormsControlProps(CheckoutDirControl);

function WorktreePrefixControl(props: ControlProps) {
  const { data, handleChange, path, label, description } = props;
  return (
    <div className="control jsf-control">
      <FieldHeading label={label} description={description} />
      <input
        className="text-input jsf-default-hint"
        type="text"
        value={data ?? ""}
        placeholder={WORKTREE_PREFIX_DEFAULT}
        onChange={(e) => handleChange(path, e.target.value || null)}
        spellCheck={false}
        autoCapitalize="off"
        autoCorrect="off"
      />
    </div>
  );
}

export const worktreePrefixTester = rankWith(20, scopeEndsWith("worktree_prefix"));
export const WorktreePrefixRenderer = withJsonFormsControlProps(WorktreePrefixControl);

// ── Env files list ──────────────────────────────────────────────────────────
// A list with Scan / Add / Remove, preserving the existing scan flow. The array
// is written back wholesale via handleChange.

function EnvFilesControl(props: ControlProps) {
  const { data, handleChange, path, label, description, config } = props;
  const files: string[] = Array.isArray(data) ? data : [];
  const checkoutDir: string | null = config?.checkoutDir ?? null;

  const [adding, setAdding] = useState(false);
  const [input, setInput] = useState("");
  const [scan, setScan] = useState<"idle" | "scanning" | "done">("idle");

  const setFiles = (next: string[]) => handleChange(path, next);

  async function doScan() {
    if (!checkoutDir) return;
    setScan("scanning");
    try {
      const found = await api.scanEnvFiles(checkoutDir);
      setFiles(found);
      setScan("done");
      setTimeout(() => setScan("idle"), 2000);
    } catch {
      setScan("idle");
    }
  }

  function addFile() {
    const p = input.trim();
    if (!p) return;
    if (!files.includes(p)) setFiles([...files, p]);
    setInput("");
    setAdding(false);
  }

  function removeFile(p: string) {
    setFiles(files.filter((f) => f !== p));
  }

  return (
    <div className="control jsf-control">
      <div className="settings-group-header">
        <span className="jsf-label" style={{ marginBottom: 0 }}>{label}</span>
        <div style={{ display: "flex", gap: 4 }}>
          {checkoutDir && (
            <button
              className={`btn-add ${scan === "scanning" ? "btn-busy" : ""}`}
              disabled={scan === "scanning"}
              onClick={doScan}
            >
              {scan === "scanning" ? "…" : scan === "done" ? "✓ Scanned" : "Scan"}
            </button>
          )}
          <button className="btn-add" onClick={() => setAdding(true)}>+ Add</button>
        </div>
      </div>

      {description && <div className="jsf-help">{description}</div>}

      {adding && (
        <div className="cred-controls">
          <input
            className="text-input"
            type="text"
            placeholder=".env or subdir/.env"
            value={input}
            autoFocus
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") addFile();
              if (e.key === "Escape") { setAdding(false); setInput(""); }
            }}
            spellCheck={false}
            autoCapitalize="off"
            autoCorrect="off"
          />
          <button className="btn-save" disabled={!input.trim()} onClick={addFile}>Add</button>
          <button className="btn-clear" onClick={() => { setAdding(false); setInput(""); }}>✕</button>
        </div>
      )}

      {files.length === 0 && !adding ? (
        <p className="session-hint" style={{ paddingTop: 2 }}>
          No env files. Use Scan to find .env files in the checkout directory.
        </p>
      ) : (
        <div className="env-file-list">
          {files.map((f) => (
            <div key={f} className="env-file-row">
              <span className="env-file-path">{f}</span>
              <button className="btn-clear" onClick={() => removeFile(f)} title="Remove">✕</button>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

export const envFilesTester = rankWith(20, scopeEndsWith("env_files"));
export const EnvFilesRenderer = withJsonFormsControlProps(EnvFilesControl);

// Custom renderers first so they out-rank the vanilla defaults for their scopes.
export const repoSettingsRenderers = [
  { tester: identityTester, renderer: IdentityRenderer },
  { tester: checkoutDirTester, renderer: CheckoutDirRenderer },
  { tester: worktreePrefixTester, renderer: WorktreePrefixRenderer },
  { tester: envFilesTester, renderer: EnvFilesRenderer },
  ...vanillaRenderers,
];

export const repoSettingsCells = vanillaCells;

/** Strip draft/meta keys JsonForms' bundled ajv (draft-07) doesn't recognize.
 *  The backend validates with the real draft 2020-12; the constructs we use
 *  (nullable types, array items, integer) are draft-07 compatible, so dropping
 *  `$schema`/`$id` lets the in-form ajv compile the same shape. */
export function sanitizeSchemaForForm(
  schema: Record<string, unknown>,
): Record<string, unknown> {
  const { $schema, $id, ...rest } = schema;
  void $schema;
  void $id;
  return rest;
}
