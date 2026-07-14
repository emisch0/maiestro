import { useState, useEffect, useCallback, useMemo, useRef, Component, ErrorInfo, ReactNode } from "react";
import { getCurrentWindow, getAllWindows } from "@tauri-apps/api/window";
import { api, AppSettings, CredentialScope, CredentialTypeDto, DraftPreviewOutcome, GHRepo, HealthCheck, HealthReport, HealthStatus, HideState, IssueNode, PrChecks, PrLink, RepoSettings, ResolvedTool, Session, SpawnEdits, SpawnPlan, StatusRecord, WorkState } from "./api";
import { JsonForms } from "@jsonforms/react";
import {
  repoSettingsRenderers,
  repoSettingsCells,
  repoSettingsUISchema,
  sanitizeSchemaForForm,
  extractFormDefaults,
  RepoFormDefaults,
} from "./RepoSettingsForm";
import {
  appSettingsRenderers,
  appSettingsCells,
  appSettingsUISchema,
} from "./AppSettingsForm";
import { applyTheme, initTheme } from "./theme";
import LogoIcon from "./icons/logo.svg?react";
import GearIcon from "./icons/gear.svg?react";
import EyeIcon from "./icons/eye.svg?react";
import GitHubIcon from "./icons/github.svg?react";
import FolderIcon from "./icons/folder.svg?react";
import VSCodeIcon from "./icons/vscode.svg?react";
import ClaudeIcon from "./icons/claude.svg?react";
import ChevronRightIcon from "./icons/chevron-right.svg?react";
import PrOpenIcon from "./icons/pull-request-open.svg?react";
import PrDraftIcon from "./icons/pull-request-draft.svg?react";
import PrMergedIcon from "./icons/pull-request-merged.svg?react";
import PrClosedIcon from "./icons/pull-request-closed.svg?react";

// One static icon per PR state — colour is baked into each asset, so no runtime
// theming. Falls back to the open icon for any unexpected state string.
const PR_STATE_ICONS: Record<string, typeof PrOpenIcon> = {
  open: PrOpenIcon,
  draft: PrDraftIcon,
  merged: PrMergedIcon,
  closed: PrClosedIcon,
};

// Tooltip text for the PR pill's merge indicator, off GitHub's mergeable_state
// (we can't read the Checks API with a fine-grained token).
function checkLabel(checks: PrChecks): string {
  switch (checks.mergeable_state) {
    case "clean":
    case "has_hooks": return "Ready to merge";
    case "unstable": return "Mergeable — a non-required check is failing";
    case "dirty": return "Merge conflicts with the base branch";
    case "behind": return "Branch is behind the base — update it";
    case "blocked": return "Blocked by a required review or status check";
    case "draft": return "Draft — mark ready for review to merge";
    default: return "Checking mergeability…"; // unknown / not yet computed
  }
}

// A terminal reason the auto-merge can't proceed and won't self-resolve, or null
// to keep waiting (e.g. mergeability still computing, or a required check still
// running which GitHub will clear to "clean"). These need the user to act, so
// the loop stops and surfaces them. Driven purely by mergeable_state now.
function mergeBlocker(c: PrChecks): string | null {
  switch (c.mergeable_state) {
    case "dirty": return "Merge conflicts with the base branch — resolve them and push, then merge again.";
    case "behind": return "Branch is behind the base — update it (merge or rebase) and push, then merge again.";
    case "blocked": return "Blocked by a required review or status check — resolve it, then merge again.";
    default: return null;
  }
}

// The work-lifecycle phase of a session, an axis distinct from the live Claude
// busy/idle status. A merged PR wins outright; otherwise any local work (commits
// ahead of base or an uncommitted change) means Implementing, and a pristine
// branch is still Planning. Returns null while the work state is unresolved so a
// working branch isn't briefly mislabeled "Planning".
type LifecyclePhase = "planning" | "implementing" | "merged";
function lifecyclePhase(pr: PrLink | null | undefined, work: WorkState | null | undefined): LifecyclePhase | null {
  if (pr?.state === "merged") return "merged";
  if (work === undefined) return null;
  if (work && (work.ahead > 0 || work.dirty)) return "implementing";
  return "planning";
}

const LIFECYCLE_LABELS: Record<LifecyclePhase, string> = {
  planning: "Planning",
  implementing: "Implementing",
  merged: "Merged",
};

// Top-to-bottom order of the lifecycle zones within a repo group. Work items live
// in the zone matching their phase and slide between zones as the phase changes.
const ZONES: LifecyclePhase[] = ["planning", "implementing", "merged"];

// How far along each phase is, used to collapse several sessions on one issue down
// to the single most-advanced phase for that issue's pill.
const PHASE_RANK: Record<LifecyclePhase, number> = { planning: 0, implementing: 1, merged: 2 };
function furtherPhase(a: LifecyclePhase | null, b: LifecyclePhase | null): LifecyclePhase | null {
  if (a == null) return b;
  if (b == null) return a;
  return PHASE_RANK[a] >= PHASE_RANK[b] ? a : b;
}

// The workspace palette colors are dark (they're VS Code title-bar backgrounds),
// so as a thin border or an icon tint on the dark popover they read as muddy and
// hard to tell apart. Keep each color's hue but pin it to a bright, uniform
// lightness/saturation so the eight hues separate cleanly. Falls back to the raw
// value for greys or anything unparseable.
function accentColor(hex: string): string {
  const m = /^#?([0-9a-f]{6})$/i.exec(hex.trim());
  if (!m) return hex;
  const n = parseInt(m[1], 16);
  const r = (n >> 16) / 255, g = ((n >> 8) & 255) / 255, b = (n & 255) / 255;
  const max = Math.max(r, g, b), min = Math.min(r, g, b), d = max - min;
  if (d === 0) return hex;
  let h: number;
  if (max === r) h = ((g - b) / d) % 6;
  else if (max === g) h = (b - r) / d + 2;
  else h = (r - g) / d + 4;
  h = (h * 60 + 360) % 360;
  return `hsl(${Math.round(h)}, 68%, 62%)`;
}

// A pending Tear Down prompt: a warnings confirmation, or a "VS Code still open"
// block (which may offer the Accessibility shortcut so mAIestro can close it).
type TeardownPrompt =
  | { id: string; kind: "confirm"; warnings: string[] }
  | { id: string; kind: "blocked"; message: string; accessibility: boolean };

type SettingsSelection =
  | { kind: "identity"; id: string }
  | { kind: "repo"; repo: string }
  | { kind: "repo-add" }
  | { kind: "preferences" }
  | null;
type SaveStatus = "idle" | "saving" | "saved" | "clearing" | "error";

interface CredState {
  isSet: boolean;
  input: string;
  status: SaveStatus;
  error?: string;
}

// Guards the Settings detail panel so a render failure in one item (e.g. the
// JsonForms repo form) degrades to an inline message instead of unmounting the
// whole window and stranding the user with no sidebar to navigate back. Reset
// by keying it on the current selection, so picking another item remounts clean.
class DetailErrorBoundary extends Component<{ children: ReactNode }, { error: Error | null }> {
  state: { error: Error | null } = { error: null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error("Settings detail panel crashed:", error, info);
  }

  render() {
    if (this.state.error) {
      return (
        <div className="cred-list">
          <div className="cleanup-confirm">
            <p className="cleanup-lead">Couldn't display this panel</p>
            <pre className="tool-error-message">{this.state.error.message || String(this.state.error)}</pre>
            <p className="cleanup-confirm-body" style={{ opacity: 0.7 }}>
              Pick another item on the left, or reopen Settings.
            </p>
          </div>
        </div>
      );
    }
    return this.props.children;
  }
}

function Settings() {
  const [selection, setSelection] = useState<SettingsSelection>(null);
  const [identitiesOpen, setIdentitiesOpen] = useState(true);
  const [reposOpen, setReposOpen] = useState(true);
  const [addingIdentityInline, setAddingIdentityInline] = useState(false);
  const [identityInputInline, setIdentityInputInline] = useState("");
  const [knownIdentities, setKnownIdentities] = useState<string[]>([]);
  const [repos, setRepos] = useState<string[]>([]);
  const [credTypes, setCredTypes] = useState<CredentialTypeDto[]>([]);
  const [credStates, setCredStates] = useState<Record<string, CredState>>({});
  // Settings for the selected repo, tagged with the repo they belong to. null
  // until repo_settings_get resolves — the form must not render before then,
  // or JsonForms' initial onChange autosaves placeholder data over the real
  // file (#65).
  const [loadedRepo, setLoadedRepo] = useState<{ repo: string; settings: RepoSettings } | null>(null);
  // The hand-written JSON Schema, fetched from the backend, that drives the
  // repo-detail form. null until loaded.
  const [repoSchema, setRepoSchema] = useState<Record<string, unknown> | null>(null);
  // Default values (worktree prefix, AI prompts) read from the schema's
  // `default` keywords — shown by the custom renderers.
  const [repoFormDefaults, setRepoFormDefaults] = useState<RepoFormDefaults | null>(null);
  // Set when the backend rejects a repo's settings file on load (bad JSON or a
  // schema violation); shown as a banner instead of a form full of defaults.
  const [repoLoadError, setRepoLoadError] = useState<string | null>(null);
  // Set when an autosave write fails.
  const [repoSaveError, setRepoSaveError] = useState<string | null>(null);
  // Whether the Remove Identity confirm is showing for the selected identity.
  const [identityRemoveConfirm, setIdentityRemoveConfirm] = useState(false);
  // Set when removing the selected identity fails.
  const [identityRemoveError, setIdentityRemoveError] = useState<string | null>(null);
  // Whether the Remove Repo confirm is showing for the selected repo.
  const [repoRemoveConfirm, setRepoRemoveConfirm] = useState(false);
  // Set when removing the selected repo fails.
  const [repoRemoveError, setRepoRemoveError] = useState<string | null>(null);
  // Repo health-check modal (#93): the report to show, and its loading/error state.
  const [health, setHealth] = useState<
    { repo: string; report: HealthReport | null; loading: boolean; error: string | null } | null
  >(null);
  // Last persisted form data, to skip the no-op onChange JsonForms fires on load.
  const lastSavedRef = useRef<string>("");
  const saveTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [browse, setBrowse] = useState<{
    identityId: string | null;
    identityInput: string;
    repos: GHRepo[] | null;
    filter: string;
    loading: boolean;
    error?: string;
  }>({ identityId: null, identityInput: "", repos: null, filter: "", loading: false });
  // Global app settings ("Preferences" panel), rendered via JSON Forms (#85).
  const [appSchema, setAppSchema] = useState<Record<string, unknown> | null>(null);
  const [appSettings, setAppSettings] = useState<AppSettings | null>(null);
  const [appLoadError, setAppLoadError] = useState<string | null>(null);
  const [appSaveError, setAppSaveError] = useState<string | null>(null);
  // How each directly-invoked CLI currently resolves, for the Tool paths status line.
  const [resolvedTools, setResolvedTools] = useState<ResolvedTool[]>([]);
  const appLastSavedRef = useRef<string>("");
  const appSaveTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    api.listCredentialTypes().then((types) => {
      setCredTypes(types);
      const init: Record<string, CredState> = {};
      for (const t of types) init[t.type_id] = { isSet: false, input: "", status: "idle" };
      setCredStates(init);
    });
    api.listRepos().then(setRepos);
    api.identitiesList().then(setKnownIdentities);
    api.getDefaultIdentity().then((id) => { if (id) setSelection({ kind: "identity", id }); });
    api.repoSettingsSchema().then((s) => {
      setRepoFormDefaults(extractFormDefaults(s));
      setRepoSchema(sanitizeSchemaForForm(s));
    });
    api.appSettingsSchema().then((s) => setAppSchema(sanitizeSchemaForForm(s)));
    loadAppSettings();
  }, []);

  // Load the global settings + current tool resolution for the Preferences form.
  function loadAppSettings() {
    api.getAppSettings()
      .then((s) => {
        // Seed the baseline so JsonForms' initial onChange (same data) is a no-op.
        appLastSavedRef.current = JSON.stringify(s);
        setAppSettings(s);
        setAppLoadError(null);
      })
      .catch((e) => setAppLoadError(String(e)));
    api.toolsResolved().then(setResolvedTools).catch(() => setResolvedTools([]));
  }

  // Autosave the Preferences form, debounced, skipped while ajv reports errors.
  // Mirrors the repo-form autosave. Theme is applied to this window immediately
  // for responsiveness; the backend also broadcasts `theme-changed` to the rest.
  function handleAppFormChange(data: AppSettings, errors: unknown[] | undefined) {
    setAppSettings(data);
    if ((errors?.length ?? 0) > 0) return;
    const serialized = JSON.stringify(data);
    if (serialized === appLastSavedRef.current) return;
    const prev = appLastSavedRef.current;
    appLastSavedRef.current = serialized;
    applyTheme(data.theme ?? "system");
    if (appSaveTimerRef.current) clearTimeout(appSaveTimerRef.current);
    appSaveTimerRef.current = setTimeout(async () => {
      try {
        await api.setAppSettings(data);
        setAppSaveError(null);
        // Refresh resolution so the Tool paths status line reflects the new paths.
        api.toolsResolved().then(setResolvedTools).catch(() => {});
      } catch (e) {
        appLastSavedRef.current = prev; // let a fixed value save again
        setAppSaveError(String(e));
      }
    }, 400);
  }

  useEffect(() => {
    const win = getCurrentWindow();
    const unlisten = win.onCloseRequested((event) => {
      event.preventDefault();
      win.hide();
    });
    return () => { unlisten.then((f) => f()); };
  }, []);

  const patchCred = useCallback((type_id: string, patch: Partial<CredState>) => {
    setCredStates((prev) => ({ ...prev, [type_id]: { ...prev[type_id], ...patch } }));
  }, []);

  const activeScope = useMemo((): CredentialScope | null => {
    if (selection?.kind === "identity")
      return { kind: "identity", identity_id: selection.id };
    return null;
  }, [selection]);

  const scopeKey = activeScope ? `identity:${activeScope.identity_id}` : null;

  // Selecting a different identity dismisses any pending remove confirm/error.
  useEffect(() => {
    setIdentityRemoveConfirm(false);
    setIdentityRemoveError(null);
  }, [scopeKey]);

  useEffect(() => {
    setCredStates((prev) => {
      const next = { ...prev };
      for (const id of Object.keys(next)) next[id] = { ...next[id], isSet: false };
      return next;
    });
    if (!activeScope || credTypes.length === 0) return;
    const scope = activeScope;
    const timer = setTimeout(async () => {
      await Promise.all(
        credTypes.map(async (t) => {
          try {
            const isSet = await api.credentialExists(t.type_id, scope);
            patchCred(t.type_id, { isSet });
          } catch {
            patchCred(t.type_id, { isSet: false });
          }
        }),
      );
    }, 300);
    return () => clearTimeout(timer);
  }, [scopeKey]); // eslint-disable-line react-hooks/exhaustive-deps

  async function handleSave(type_id: string) {
    if (!activeScope) return;
    const state = credStates[type_id];
    if (!state?.input.trim()) return;
    patchCred(type_id, { status: "saving" });
    try {
      await api.setCredential(type_id, activeScope, state.input.trim());
      patchCred(type_id, { status: "saved", isSet: true, input: "" });
      api.identitiesList().then(setKnownIdentities);
      setTimeout(() => patchCred(type_id, { status: "idle" }), 2000);
    } catch (e) {
      patchCred(type_id, { status: "error", error: String(e) });
    }
  }

  const selectedRepo = selection?.kind === "repo" ? selection.repo : null;

  // Identifies the current detail selection, used to key the error boundary so
  // navigating to a different item clears any prior render error.
  const selectionKey =
    selection == null ? "none"
      : selection.kind === "repo" ? `repo:${selection.repo}`
      : selection.kind === "identity" ? `identity:${selection.id}`
      : selection.kind;

  useEffect(() => {
    if (!selectedRepo) return;
    setLoadedRepo(null);
    setRepoLoadError(null);
    setRepoSaveError(null);
    setRepoRemoveConfirm(false);
    setRepoRemoveError(null);
    api.getRepoSettings(selectedRepo)
      .then((s) => {
        // Seed the baseline so JsonForms' initial onChange (same data) is a no-op.
        lastSavedRef.current = JSON.stringify(s);
        setLoadedRepo({ repo: selectedRepo, settings: s });
      })
      .catch((e) => setRepoLoadError(String(e)));
  }, [selectedRepo]); // eslint-disable-line react-hooks/exhaustive-deps

  // Extra data the custom renderers (identity select, env-files Scan) read via
  // JsonForms' `config`. Memoized so the form isn't needlessly re-keyed.
  const repoFormConfig = useMemo(
    () => ({
      showUnfocusedDescription: true as const,
      knownIdentities,
      clonedRepoDir: loadedRepo?.settings.cloned_repo_dir ?? null,
      worktreePrefixDefault: repoFormDefaults?.worktreePrefixDefault ?? "",
      promptModelDefault: repoFormDefaults?.promptModelDefault ?? "",
      promptDefaults: repoFormDefaults?.promptDefaults ?? {},
    }),
    [knownIdentities, loadedRepo?.settings.cloned_repo_dir, repoFormDefaults],
  );

  // Config the app-settings custom renderers read (the Tool paths status line).
  const appFormConfig = useMemo(
    () => ({ showUnfocusedDescription: true as const, resolvedTools }),
    [resolvedTools],
  );

  // Autosave on change, debounced, skipped while ajv reports errors. JsonForms
  // preserves fields not in the UI schema (repo, hidden), so they round-trip.
  function handleRepoFormChange(repo: string, data: RepoSettings, errors: unknown[] | undefined) {
    // Only the currently-loaded repo may save — a stale render mid-switch must
    // not write its data under another repo's key (#65).
    if (loadedRepo?.repo !== repo) return;
    setLoadedRepo({ repo, settings: data });
    if ((errors?.length ?? 0) > 0) return;
    const serialized = JSON.stringify(data);
    if (serialized === lastSavedRef.current) return;
    lastSavedRef.current = serialized;
    if (saveTimerRef.current) clearTimeout(saveTimerRef.current);
    saveTimerRef.current = setTimeout(async () => {
      try {
        await api.setRepoSettings(repo, data);
        setRepoSaveError(null);
      } catch (e) {
        setRepoSaveError(String(e));
      }
    }, 400);
  }

  async function commitIdentityInline() {
    const trimmed = identityInputInline.trim();
    if (!trimmed) return;
    setAddingIdentityInline(false);
    setIdentityInputInline("");
    try {
      await api.identitiesAdd(trimmed);
      const list = await api.identitiesList();
      setKnownIdentities(list);
      setSelection({ kind: "identity", id: trimmed });
    } catch { /* best-effort */ }
  }

  // Remove an identity: Keychain credentials, list entry, and repo references
  // all go; worktrees and sessions are untouched.
  async function handleRemoveIdentity(id: string) {
    setIdentityRemoveConfirm(false);
    try {
      await api.identitiesRemove(id);
      setKnownIdentities((prev) => prev.filter((i) => i !== id));
      setSelection(null);
    } catch (e) {
      setIdentityRemoveError(String(e));
    }
  }

  async function handleClear(type_id: string) {
    if (!activeScope) return;
    patchCred(type_id, { status: "clearing" });
    try {
      await api.deleteCredential(type_id, activeScope);
    } catch { /* already gone */ }
    patchCred(type_id, { status: "idle", isSet: false, input: "" });
  }

  async function handleSelectBrowseIdentity(iid: string) {
    setBrowse((prev) => ({ ...prev, identityId: iid, loading: true, error: undefined }));
    try {
      const fetched = await api.githubListRepos(iid);
      setBrowse((prev) => ({ ...prev, identityId: iid, loading: false, repos: fetched, filter: "" }));
      api.identitiesList().then(setKnownIdentities);
    } catch (e) {
      setBrowse((prev) => ({ ...prev, identityId: iid, loading: false, error: String(e) }));
    }
  }

  // Untrack the selected repo (delete its settings file). Cancels any pending
  // debounced autosave first so it can't recreate the file after the delete.
  async function handleRemoveRepo(repo: string) {
    setRepoRemoveConfirm(false);
    if (saveTimerRef.current) clearTimeout(saveTimerRef.current);
    try {
      await api.removeRepo(repo);
      setRepos((prev) => prev.filter((r) => r !== repo));
      setSelection(null);
    } catch (e) {
      setRepoRemoveError(String(e));
    }
  }

  // Run the repo health check and show its report in a modal (#93).
  async function runHealthCheck(repo: string) {
    setHealth({ repo, report: null, loading: true, error: null });
    try {
      const report = await api.repoHealthCheck(repo);
      setHealth({ repo, report, loading: false, error: null });
    } catch (e) {
      setHealth({ repo, report: null, loading: false, error: String(e) });
    }
  }

  async function handleSelectRepo(repo: string) {
    const iid = browse.identityId;
    setRepos((prev) => (prev.includes(repo) ? prev : [...prev, repo]));
    setSelection({ kind: "repo", repo });
    const defaults = await api.getRepoSettings(repo);
    await api.setRepoSettings(repo, { ...defaults, identity_id: iid ?? defaults.identity_id });
  }

  function CredRows() {
    return (
      <>
        {credTypes.map((t) => {
          const state = credStates[t.type_id] ?? { isSet: false, input: "", status: "idle" as SaveStatus };
          const isBusy = state.status === "saving" || state.status === "clearing";

          return (
            <div key={t.type_id} className="cred-row">
              <div className="cred-header">
                <span className="cred-name">{t.display_name}</span>
                <span className={`cred-badge ${state.isSet ? "is-set" : "not-set"}`}>
                  {state.isSet ? "set" : "not set"}
                </span>
              </div>
              <div className="cred-controls">
                <input
                  className="text-input secret-input"
                  type="password"
                  placeholder={state.isSet ? "Update value…" : "Enter value…"}
                  value={state.input}
                  disabled={isBusy}
                  onChange={(e) => patchCred(t.type_id, { input: e.target.value, status: "idle" })}
                  onKeyDown={(e) => e.key === "Enter" && handleSave(t.type_id)}
                />
                <button
                  className={`btn-save ${state.status === "saving" ? "btn-busy" : ""}`}
                  disabled={!state.input.trim() || isBusy}
                  onClick={() => handleSave(t.type_id)}
                >
                  {state.status === "saving" ? "…" : state.status === "saved" ? "✓" : "Save"}
                </button>
                <button
                  className={`btn-clear ${!state.isSet ? "hidden" : ""}`}
                  disabled={!state.isSet || isBusy}
                  onClick={() => handleClear(t.type_id)}
                  title="Remove credential"
                >
                  ✕
                </button>
              </div>
              {state.status === "error" && <p className="cred-error">{state.error}</p>}
            </div>
          );
        })}
      </>
    );
  }

  return (
    <main className="panel panel--window">
      <div className="settings-layout">
        {/* ── Sidebar ── */}
        <div className="settings-sidebar">
          <div className="settings-tree">

            {/* Identities section */}
            <div className="tree-section">
              <div className="tree-section-header">
                <button
                  className="tree-section-toggle"
                  onClick={() => setIdentitiesOpen((v) => !v)}
                  aria-expanded={identitiesOpen}
                >
                  <ChevronRightIcon className={`tree-chevron ${identitiesOpen ? "tree-chevron--open" : ""}`} />
                  <span className="tree-section-label">Identities</span>
                </button>
                <button
                  className="tree-add-btn"
                  onClick={() => { setIdentitiesOpen(true); setAddingIdentityInline(true); setIdentityInputInline(""); }}
                  title="Add identity"
                  aria-label="Add identity"
                >+</button>
              </div>
              {identitiesOpen && (
                <div className="tree-items">
                  {addingIdentityInline && (
                    <div className="tree-add-row">
                      <input
                        className="text-input tree-add-input"
                        type="text"
                        placeholder="e.g. default"
                        value={identityInputInline}
                        autoFocus
                        onChange={(e) => setIdentityInputInline(e.target.value)}
                        onKeyDown={(e) => {
                          if (e.key === "Enter") commitIdentityInline();
                          if (e.key === "Escape") { setAddingIdentityInline(false); setIdentityInputInline(""); }
                        }}
                        spellCheck={false}
                        autoCapitalize="off"
                        autoCorrect="off"
                      />
                    </div>
                  )}
                  {knownIdentities.length === 0 && !addingIdentityInline && (
                    <p className="tree-empty-hint">No identities yet</p>
                  )}
                  {knownIdentities.map((id) => (
                    <button
                      key={id}
                      className={`tree-item${selection?.kind === "identity" && selection.id === id ? " tree-item--selected" : ""}`}
                      onClick={() => setSelection({ kind: "identity", id })}
                    >
                      {id}
                    </button>
                  ))}
                </div>
              )}
            </div>

            {/* Repos section */}
            <div className="tree-section">
              <div className="tree-section-header">
                <button
                  className="tree-section-toggle"
                  onClick={() => setReposOpen((v) => !v)}
                  aria-expanded={reposOpen}
                >
                  <ChevronRightIcon className={`tree-chevron ${reposOpen ? "tree-chevron--open" : ""}`} />
                  <span className="tree-section-label">Repos</span>
                </button>
                <button
                  className="tree-add-btn"
                  onClick={() => {
                    api.identitiesList().then(setKnownIdentities);
                    setBrowse({ identityId: null, identityInput: "", repos: null, filter: "", loading: false });
                    setReposOpen(true);
                    setSelection({ kind: "repo-add" });
                  }}
                  title="Add repo"
                  aria-label="Add repo"
                >+</button>
              </div>
              {reposOpen && (
                <div className="tree-items">
                  {repos.length === 0 && (
                    <p className="tree-empty-hint">No repos yet</p>
                  )}
                  {repos.map((repo) => (
                    <button
                      key={repo}
                      className={`tree-item${selection?.kind === "repo" && selection.repo === repo ? " tree-item--selected" : ""}`}
                      onClick={() => setSelection({ kind: "repo", repo })}
                    >
                      {repo.split("/")[1] ?? repo}
                    </button>
                  ))}
                </div>
              )}
            </div>

            {/* Preferences — non-expandable leaf */}
            <div className="tree-section">
              <button
                className={`tree-item tree-item--preferences${selection?.kind === "preferences" ? " tree-item--selected" : ""}`}
                onClick={() => setSelection({ kind: "preferences" })}
              >
                Preferences
              </button>
            </div>

          </div>
        </div>

        {/* ── Detail panel ── */}
        <div className="settings-detail">
          <DetailErrorBoundary key={selectionKey}>
          {!selection ? (
            <div className="cred-list">
              <div className="empty-state">
                <p className="empty-state-body">Select an item on the left.</p>
              </div>
            </div>
          ) : selection.kind === "preferences" ? (
            <div className="cred-list">
              {appLoadError ? (
                // The backend rejected ~/.maiestro/settings.json (bad JSON or a
                // schema violation). Show it rather than a form full of defaults
                // that would clobber the file on the next save.
                <div className="cleanup-confirm">
                  <p className="cleanup-confirm-body">Couldn't load settings:</p>
                  <p className="cleanup-confirm-body" style={{ opacity: 0.85, fontFamily: "var(--font-mono, monospace)", fontSize: 11 }}>
                    {appLoadError}
                  </p>
                  <p className="cleanup-confirm-body" style={{ opacity: 0.7 }}>
                    Fix ~/.maiestro/settings.json by hand, then reopen Preferences.
                  </p>
                </div>
              ) : appSchema && appSettings ? (
                <div className="jsf-root">
                  {appSaveError && (
                    <div className="cleanup-confirm">
                      <p className="cleanup-confirm-body">Couldn't save: {appSaveError}</p>
                    </div>
                  )}
                  <JsonForms
                    schema={appSchema}
                    uischema={appSettingsUISchema}
                    data={appSettings}
                    renderers={appSettingsRenderers}
                    cells={appSettingsCells}
                    config={appFormConfig}
                    onChange={({ data, errors }) =>
                      handleAppFormChange(data as AppSettings, errors)
                    }
                  />
                </div>
              ) : (
                <p className="session-hint" style={{ paddingTop: 2 }}>Loading…</p>
              )}
            </div>
          ) : selection.kind === "identity" ? (
            <>
              <div className="detail-header">
                <span className="detail-title">{selection.id}</span>
              </div>
              <div className="cred-list">
                <CredRows />
                <div className="settings-group">
                  {identityRemoveError && (
                    <div className="cleanup-confirm">
                      <p className="cleanup-lead">Couldn't remove identity</p>
                      <pre className="tool-error-message">{identityRemoveError}</pre>
                      <div className="issue-actions">
                        <button className="btn-ghost" onClick={() => setIdentityRemoveError(null)}>Dismiss</button>
                      </div>
                    </div>
                  )}
                  {identityRemoveConfirm ? (
                    <div className="cleanup-confirm">
                      <p className="cleanup-lead">Remove {selection.id} from mAIestro?</p>
                      <p className="cleanup-confirm-body">
                        Its Keychain credentials are deleted; repos that used it need a new identity.
                      </p>
                      <div className="issue-actions">
                        <button className="btn-danger" onClick={() => handleRemoveIdentity(selection.id)}>Remove</button>
                        <button className="btn-ghost" onClick={() => setIdentityRemoveConfirm(false)}>Cancel</button>
                      </div>
                    </div>
                  ) : (
                    <button className="btn-danger" onClick={() => setIdentityRemoveConfirm(true)}>Remove Identity</button>
                  )}
                </div>
              </div>
            </>
          ) : selection.kind === "repo" ? (
            <>
              <div className="detail-header">
                <span className="detail-title">{selection.repo}</span>
                <button
                  className="btn-ghost health-check-btn"
                  onClick={() => runHealthCheck(selection.repo)}
                >
                  Check Health
                </button>
              </div>
              <div className="cred-list">
                {repoLoadError ? (
                  // The backend rejected this repo's settings file (bad JSON or a
                  // schema violation). Show the error rather than a form full of
                  // defaults that would clobber the file on the next save.
                  <div className="cleanup-confirm">
                    <p className="cleanup-confirm-body">
                      Couldn't load settings for this repo:
                    </p>
                    <p className="cleanup-confirm-body" style={{ opacity: 0.85, fontFamily: "var(--font-mono, monospace)", fontSize: 11 }}>
                      {repoLoadError}
                    </p>
                    <p className="cleanup-confirm-body" style={{ opacity: 0.7 }}>
                      Fix the file by hand, then reselect this repo.
                    </p>
                  </div>
                ) : repoSchema && loadedRepo?.repo === selection.repo ? (
                  <div className="jsf-root">
                    {repoSaveError && (
                      <div className="cleanup-confirm">
                        <p className="cleanup-confirm-body">Couldn't save: {repoSaveError}</p>
                      </div>
                    )}
                    <JsonForms
                      key={selection.repo}
                      schema={repoSchema}
                      uischema={repoSettingsUISchema}
                      data={loadedRepo.settings}
                      renderers={repoSettingsRenderers}
                      cells={repoSettingsCells}
                      config={repoFormConfig}
                      onChange={({ data, errors }) =>
                        handleRepoFormChange(selection.repo, data as RepoSettings, errors)
                      }
                    />
                  </div>
                ) : (
                  <p className="session-hint" style={{ paddingTop: 2 }}>Loading…</p>
                )}
                {/* Removal works even when the settings file failed to load —
                    deleting the file is the fix for an unparseable one. */}
                <div className="settings-group">
                  {repoRemoveError && (
                    <div className="cleanup-confirm">
                      <p className="cleanup-lead">Couldn't remove repo</p>
                      <pre className="tool-error-message">{repoRemoveError}</pre>
                      <div className="issue-actions">
                        <button className="btn-ghost" onClick={() => setRepoRemoveError(null)}>Dismiss</button>
                      </div>
                    </div>
                  )}
                  {repoRemoveConfirm ? (
                    <div className="cleanup-confirm">
                      <p className="cleanup-lead">Remove {selection.repo} from mAIestro?</p>
                      <p className="cleanup-confirm-body">
                        Worktrees, cloned repos, and any work in them are kept on disk.
                      </p>
                      <div className="issue-actions">
                        <button className="btn-danger" onClick={() => handleRemoveRepo(selection.repo)}>Remove</button>
                        <button className="btn-ghost" onClick={() => setRepoRemoveConfirm(false)}>Cancel</button>
                      </div>
                    </div>
                  ) : (
                    <button className="btn-danger" onClick={() => setRepoRemoveConfirm(true)}>Remove Repo</button>
                  )}
                </div>
              </div>
            </>
          ) : (
            /* selection.kind === "repo-add": identity picker → GitHub repo browser */
            <>
              <div className="detail-header">
                {browse.identityId !== null ? (
                  <>
                    <button
                      className="btn-back"
                      onClick={() => setBrowse({ identityId: null, identityInput: "", repos: null, filter: "", loading: false, error: undefined })}
                    >‹</button>
                    <span className="detail-title">{browse.identityId}</span>
                  </>
                ) : (
                  <span className="detail-title">Add Repo</span>
                )}
              </div>
              {browse.repos !== null && (
                <div className="adding-row">
                  <input
                    className="text-input"
                    type="text"
                    placeholder="Filter repos…"
                    value={browse.filter}
                    autoFocus
                    onChange={(e) => setBrowse((prev) => ({ ...prev, filter: e.target.value }))}
                    spellCheck={false}
                    autoCapitalize="off"
                    autoCorrect="off"
                  />
                </div>
              )}
              <div className="cred-list">
                {browse.identityId === null && (
                  knownIdentities.length === 0 ? (
                    <div className="empty-state">
                      <p className="empty-state-title">No identities yet</p>
                      <p className="empty-state-body">
                        Add an identity in the Identities section first, then save a GitHub token for it.
                      </p>
                    </div>
                  ) : (
                    <div className="cred-controls">
                      <select
                        className="text-input profile-select"
                        value={browse.identityInput}
                        autoFocus
                        onChange={(e) => setBrowse((prev) => ({ ...prev, identityInput: e.target.value }))}
                      >
                        <option value="">Select an identity…</option>
                        {knownIdentities.map((iid) => (
                          <option key={iid} value={iid}>{iid}</option>
                        ))}
                      </select>
                      <button
                        className="btn-save"
                        disabled={!browse.identityInput.trim()}
                        onClick={() => handleSelectBrowseIdentity(browse.identityInput.trim())}
                      >
                        Fetch
                      </button>
                    </div>
                  )
                )}
                {browse.identityId !== null && browse.loading && (
                  <div className="empty-state">
                    <p className="empty-state-body">Fetching repos…</p>
                  </div>
                )}
                {browse.error && (
                  <p className="cred-error" style={{ paddingTop: 4 }}>{browse.error}</p>
                )}
                {browse.repos !== null && (() => {
                  const filtered = browse.repos.filter((r) =>
                    !browse.filter || r.full_name.toLowerCase().includes(browse.filter.toLowerCase())
                  );
                  return filtered.length === 0 ? (
                    <div className="empty-state">
                      <p className="empty-state-body">No repos match your filter.</p>
                    </div>
                  ) : filtered.map((r) => (
                    <button
                      key={r.full_name}
                      className="repo-item"
                      onClick={() => handleSelectRepo(r.full_name)}
                    >
                      <span className="repo-item-name">{r.full_name}</span>
                      {r.private
                        ? <span className="repo-item-private">private</span>
                        : <span className="repo-item-chevron">›</span>
                      }
                    </button>
                  ));
                })()}
              </div>
            </>
          )}
          </DetailErrorBoundary>
        </div>
      </div>
      {health && (
        <HealthModal
          state={health}
          onRetry={() => runHealthCheck(health.repo)}
          onClose={() => setHealth(null)}
        />
      )}
    </main>
  );
}

const HEALTH_ICON: Record<HealthStatus, string> = {
  pass: "✓",
  fail: "✕",
  warn: "!",
  skipped: "–",
};

function HealthCheckRow({ check, nested }: { check: HealthCheck; nested?: boolean }) {
  return (
    <>
      <div className={`health-row health-row--${check.status}${nested ? " health-row--nested" : ""}`}>
        <span className={`health-icon health-icon--${check.status}`}>{HEALTH_ICON[check.status]}</span>
        <div className="health-row-text">
          <span className="health-label">{check.label}</span>
          {check.detail && <span className="health-detail">{check.detail}</span>}
        </div>
      </div>
      {check.sub.map((s) => (
        <HealthCheckRow key={s.id} check={s} nested />
      ))}
    </>
  );
}

function HealthModal({
  state,
  onRetry,
  onClose,
}: {
  state: { repo: string; report: HealthReport | null; loading: boolean; error: string | null };
  onRetry: () => void;
  onClose: () => void;
}) {
  return (
    <div className="overlay" onClick={onClose}>
      <div className="overlay-panel health-dialog" onClick={(e) => e.stopPropagation()}>
        <div className="overlay-header">
          <span className="overlay-title">Health · {state.repo}</span>
          <button className="icon-btn" onClick={onClose} aria-label="Close">✕</button>
        </div>
        <div className="health-body">
          {state.loading ? (
            <p className="session-hint" style={{ padding: "8px 2px" }}>Running checks…</p>
          ) : state.error ? (
            <div className="cleanup-confirm">
              <p className="cleanup-lead">Couldn't run health check</p>
              <pre className="tool-error-message">{state.error}</pre>
              <div className="issue-actions">
                <button className="btn-save" onClick={onRetry}>Retry</button>
              </div>
            </div>
          ) : state.report ? (
            state.report.checks.map((c) => <HealthCheckRow key={c.id} check={c} />)
          ) : null}
        </div>
        <div className="health-footer">
          <button className="btn-save" onClick={onClose}>Done</button>
        </div>
      </div>
    </div>
  );
}

async function openSettings() {
  const settings = (await getAllWindows()).find((w) => w.label === "settings");
  if (settings) {
    await settings.show();
    await settings.setFocus();
  }
}

// The editable spawn preview, shown in the main area before a worktree is made.
// `issueNumber` is null on the create path (issue opened only on confirm).
// `orig*` capture the fetched/drafted values so we know whether to PATCH GitHub.
type Preview = {
  // "spawn" creates a worktree (session name + slugs); "create" just opens the
  // issue and returns to the list.
  mode: "spawn" | "create";
  issueNumber: number | null;
  issueTitle: string;
  issueBody: string;
  shortTitle: string;
  color: string;
  emoji: string;
  repoName: string;
  origTitle: string;
  origBody: string;
  spawning: boolean;
  // True while Claude's short-title suggestion is in flight, so the Session name
  // field shows a "generating title" rainbow indicator.
  suggesting?: boolean;
  error?: string;
};

type Picker = {
  repo: string;
  loading: boolean;
  issues: IssueNode[] | null;
  error?: string;
  query: string;
  note?: string;
  // Which create button is in flight, so we can disable both and spin the
  // active one. Undefined when idle.
  creating?: "create" | "spawn";
  // Request id of the in-flight draft, correlating `claude-activity` events so
  // the busy glow turns rainbow exactly while Claude is drafting.
  creatingRequestId?: string;
  // True while the issue list is being re-fetched via the refresh button.
  refreshing?: boolean;
  // Issue number whose spawn preview is currently being prepared (the row's
  // "Spawn Work" button glows until the preview opens or preparation fails).
  preparing?: number;
  // Set when Claude couldn't draft a clear issue: holds the original idea and
  // Claude's reply, prompting the user to confirm creating from raw text.
  // `action` records which button triggered it, so the retry repeats it.
  confirm?: { idea: string; message: string; action: "create" | "spawn" };
  // When set, the overlay shows the spawn preview instead of the issue list.
  preview?: Preview;
};

type Expand = { kind: "idea" } | { kind: "issue"; number: number } | null;

// Slug for branch/worktree names — mirrors the backend's slugify (lowercase,
// non-alphanumerics collapsed to single dashes, trimmed, cut at a dash boundary)
// so the preview shows what the backend will actually use. The backend re-derives
// authoritatively on spawn (and may add a -2 suffix to dodge collisions).
function slugify(s: string, maxLen = 25): string {
  let slug = "";
  let prevDash = false;
  for (const c of s.toLowerCase()) {
    if (/[a-z0-9]/.test(c)) {
      slug += c;
      prevDash = false;
    } else if (!prevDash) {
      slug += "-";
      prevDash = true;
    }
  }
  slug = slug.replace(/^-+|-+$/g, "");
  if (slug.length <= maxLen) return slug;
  const cut = slug.slice(0, maxLen);
  const i = cut.lastIndexOf("-");
  return (i > 0 ? cut.slice(0, i) : cut).replace(/-+$/, "");
}

// Prune the tree to nodes that match the query, keeping any ancestor that has a
// matching descendant so hierarchy/context is preserved.
function filterIssues(nodes: IssueNode[], query: string): IssueNode[] {
  const q = query.trim().toLowerCase();
  if (!q) return nodes;
  const keep = (n: IssueNode): IssueNode | null => {
    const children = n.children.map(keep).filter((c): c is IssueNode => c !== null);
    const self = n.title.toLowerCase().includes(q) || String(n.number).includes(q);
    return self || children.length ? { ...n, children } : null;
  };
  return nodes.map(keep).filter((n): n is IssueNode => n !== null);
}

// Issue number → its active session(s): the workspace color (for the dot) and a
// title to surface on hover. Issues present here have a spawned worktree.
type ActiveSessions = Record<number, { color: string; title: string; phase: LifecyclePhase | null }>;

interface IssueRowProps {
  node: IssueNode;
  depth: number;
  expandedNumber: number | null;
  // Issue number whose spawn preview is being prepared, so its row's button glows.
  preparingNumber: number | null;
  active: ActiveSessions;
  onExpand: (n: number) => void;
  onCollapse: () => void;
  onSpawn: (n: IssueNode) => void;
}

function IssueRow({ node, depth, expandedNumber, preparingNumber, active, onExpand, onCollapse, onSpawn }: IssueRowProps) {
  const indent = 10 + depth * 16;
  const isExpanded = expandedNumber === node.number;
  const session = active[node.number];
  // A "working" pill tinted with the workspace color, marking issues that
  // already have a session. Such issues can't be spawned again, so they don't
  // expand into the Spawn Work action; the hover explains why.
  const workingHint = "There is already a work session for this issue";
  // The pill is tinted with the workspace color (its identity) and labelled with
  // the work's lifecycle phase, falling back to "working" until the phase loads.
  const workingPill = session && (
    <span className="issue-working-pill" style={{ background: session.color }} title={workingHint}>
      {session.phase ? LIFECYCLE_LABELS[session.phase] : "working"}
    </span>
  );
  return (
    <>
      {isExpanded && !session ? (
        <div className={`issue-row issue-row--expanded ${session ? "issue-row--active" : ""}`} style={{ paddingLeft: indent }}>
          <div className="issue-expanded-head">
            <a
              className="issue-number issue-number--link"
              href={node.html_url}
              title={node.html_url}
              onClick={(e) => {
                e.preventDefault();
                api.openUrl(node.html_url);
              }}
            >
              #{node.number}
            </a>
            <span className="issue-title-full">{node.title}</span>
            {workingPill}
          </div>
          <div className="issue-actions">
            <button
              className={`btn-save ${preparingNumber === node.number ? "btn-busy" : ""}`}
              disabled={preparingNumber !== null}
              onClick={() => onSpawn(node)}
            >
              Spawn Work
            </button>
            <button className="btn-ghost" onClick={onCollapse}>Cancel</button>
          </div>
        </div>
      ) : (
        <div className={`issue-row ${session ? "issue-row--active" : ""}`} style={{ paddingLeft: indent }}>
          <a
            className="issue-number issue-number--link"
            href={node.html_url}
            title={node.html_url}
            onClick={(e) => {
              e.preventDefault();
              e.stopPropagation();
              api.openUrl(node.html_url);
            }}
          >
            #{node.number}
          </a>
          {session ? (
            // Already has a session — not expandable; show the hint on hover.
            <span className="issue-title issue-title--static" title={workingHint}>
              {node.title}
            </span>
          ) : (
            <button
              className="issue-title issue-row-expand"
              onClick={() => onExpand(node.number)}
              title={node.title}
            >
              {node.title}
            </button>
          )}
          {workingPill}
        </div>
      )}
      {node.children.map((c) => (
        <IssueRow
          key={c.number}
          node={c}
          depth={depth + 1}
          expandedNumber={expandedNumber}
          preparingNumber={preparingNumber}
          active={active}
          onExpand={onExpand}
          onCollapse={onCollapse}
          onSpawn={onSpawn}
        />
      ))}
    </>
  );
}

// How each status state renders in a work-item row. `running`/`idle` are quiet;
// `busy` and `needs_you` draw attention. States not in the map render nothing.
// The `creating` state (worktree being built after a spawn, issue #77) is
// deliberately absent: it renders as a `workspace-op-pill` ("Creating…") next to
// the title — like "Merging…"/"Tearing down…" — not as a Claude status pill, so
// ClaudePill renders nothing for it.
const STATUS_LABELS: Record<string, string> = {
  running: "Ready",
  busy: "Working",
  needs_you: "Needs you",
  idle: "Idle",
};

// The Claude session pill: the Claude mark tints by live state (green=working,
// amber=needs you, muted=ready/idle), with the status word beside it. Clicking
// jumps to where the session lives — the worktree's VS Code window (there is no
// deep link to the remote-controlled session itself). Renders nothing until a
// status exists, and once the session has ended.
function ClaudePill({ status, onClick }: { status?: StatusRecord; onClick: () => void }) {
  if (!status || status.state === "ended") return null;
  const label = STATUS_LABELS[status.state];
  if (!label) return null;
  // `needs_you` carries the reason (e.g. the permission request) in `detail`.
  const title = status.detail ? `Claude · ${label} — ${status.detail}` : `Claude · ${label}`;
  // A *surfaced* failed tool tints the pill red; a pending/transient one Claude
  // may still recover from doesn't. The error itself lives in the dismissible row
  // block, not this tooltip.
  const cls = `claude-pill claude-pill--${status.state}${status.state === "busy" ? " busy-ring busy-ring--ai" : ""}${status.last_error?.surfaced ? " claude-pill--error" : ""}`;
  return (
    <button
      className={cls}
      onClick={onClick}
      title={title}
      aria-label={title}
    >
      <ClaudeIcon className="claude-pill-mark" />
      <span className="claude-pill-label">{label}</span>
    </button>
  );
}

// ── Hide / snooze ──────────────────────────────────────────────────────────

type HideTarget =
  | { kind: "repo"; repo: string }
  | { kind: "session"; session: Session };

// An item is effectively hidden when it has a HideState that is either indefinite
// (no snooze) or a snooze whose deadline hasn't passed. An expired snooze reads as
// visible — that's the automatic un-snooze, resolved at render time (no live tick).
function effectiveHidden(h: HideState | null | undefined, now: number): boolean {
  if (!h) return false;
  return h.snooze_until == null || h.snooze_until > now;
}

// Remaining-time label for a snooze: days + hours normally, dropping to minutes
// only under an hour. Empty string once expired.
function formatSnoozeRemaining(snoozeUntil: number, now: number): string {
  const ms = snoozeUntil - now;
  if (ms <= 0) return "";
  const mins = Math.floor(ms / 60000);
  if (mins < 60) return `${Math.max(1, mins)}m`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours}h`;
  const days = Math.floor(hours / 24);
  const remHours = hours % 24;
  return remHours ? `${days}d ${remHours}h` : `${days}d`;
}

// Default the snooze picker to ~24h out, formatted for a datetime-local input
// (local time, no timezone suffix).
function defaultSnoozeLocal(): string {
  const d = new Date(Date.now() + 24 * 60 * 60 * 1000);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

// Quick snooze presets, resolved against `now`: 1 hour out, tomorrow at 8am
// local, and next Monday at 8am local. Anything else uses the custom picker.
function snoozePresets(now: number): { label: string; hint: string; at: number }[] {
  const timeFmt = (d: Date) => d.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
  const dayTimeFmt = (d: Date) => d.toLocaleString([], { weekday: "short", hour: "numeric", minute: "2-digit" });

  const oneHour = new Date(now + 60 * 60 * 1000);

  const tomorrow = new Date(now);
  tomorrow.setDate(tomorrow.getDate() + 1);
  tomorrow.setHours(8, 0, 0, 0);

  const nextWeek = new Date(now);
  // Days until the *next* Monday — always strictly in the future (1–7).
  const daysUntilMon = ((8 - nextWeek.getDay()) % 7) || 7;
  nextWeek.setDate(nextWeek.getDate() + daysUntilMon);
  nextWeek.setHours(8, 0, 0, 0);

  return [
    { label: "Snooze for 1 hour", hint: timeFmt(oneHour), at: oneHour.getTime() },
    { label: "Snooze until tomorrow", hint: dayTimeFmt(tomorrow), at: tomorrow.getTime() },
    { label: "Snooze until next week", hint: dayTimeFmt(nextWeek), at: nextWeek.getTime() },
  ];
}

function HideSnoozeDialog({ title, onConfirm, onClose }: {
  title: string;
  onConfirm: (hidden: HideState) => void;
  onClose: () => void;
}) {
  const [custom, setCustom] = useState(false);
  const [until, setUntil] = useState(defaultSnoozeLocal);
  const presets = useMemo(() => snoozePresets(Date.now()), []);

  function confirmCustom() {
    const ms = new Date(until).getTime();
    if (Number.isNaN(ms)) return;
    onConfirm({ snooze_until: ms });
  }

  return (
    <div className="overlay" onClick={onClose}>
      <div className="overlay-panel hide-dialog" onClick={(e) => e.stopPropagation()}>
        <div className="overlay-header">
          <span className="overlay-title">Hide · {title}</span>
          <button className="icon-btn" onClick={onClose} aria-label="Close">✕</button>
        </div>
        <div className="hide-dialog-body">
          <button className="hide-choice" onClick={() => onConfirm({ snooze_until: null })}>
            <span className="hide-choice-label">Hide</span>
            <span className="hide-choice-hint">Until you unhide it</span>
          </button>
          {presets.map((p) => (
            <button key={p.label} className="hide-choice" onClick={() => onConfirm({ snooze_until: p.at })}>
              <span className="hide-choice-label">{p.label}</span>
              <span className="hide-choice-hint">{p.hint}</span>
            </button>
          ))}
          {custom ? (
            <div className="hide-custom">
              <input
                className="text-input"
                type="datetime-local"
                value={until}
                autoFocus
                onChange={(e) => setUntil(e.target.value)}
              />
              <div className="issue-actions">
                <button className="btn-save" onClick={confirmCustom}>Confirm</button>
                <button className="btn-ghost" onClick={() => setCustom(false)}>Back</button>
              </div>
            </div>
          ) : (
            <button className="hide-choice" onClick={() => setCustom(true)}>
              <span className="hide-choice-label">Custom…</span>
              <span className="hide-choice-hint">Pick a date &amp; time</span>
            </button>
          )}
        </div>
      </div>
    </div>
  );
}

/**
 * Invisible drag grips along the popover's edges and corners. The `main` window
 * is undecorated and transparent (see tauri.conf.json), so the OS draws no resize
 * border — these thin overlays give the user something to grab, forwarding to
 * Tauri's `startResizeDragging`. Backend persists the resulting size on blur.
 */
// Tauri's `startResizeDragging` direction. The enum is declared but not exported
// from @tauri-apps/api/window, so we mirror its string union locally.
type ResizeDirection =
  | "North"
  | "South"
  | "East"
  | "West"
  | "NorthEast"
  | "NorthWest"
  | "SouthEast"
  | "SouthWest";

function ResizeGrips() {
  const startResize = (direction: ResizeDirection) => (e: React.PointerEvent) => {
    // Only the primary button starts a resize; ignore right/middle clicks.
    if (e.button !== 0) return;
    e.preventDefault();
    void getCurrentWindow().startResizeDragging(direction);
  };
  const grips: { cls: string; dir: ResizeDirection }[] = [
    { cls: "resize-grip--n", dir: "North" },
    { cls: "resize-grip--s", dir: "South" },
    { cls: "resize-grip--e", dir: "East" },
    { cls: "resize-grip--w", dir: "West" },
    { cls: "resize-grip--ne", dir: "NorthEast" },
    { cls: "resize-grip--nw", dir: "NorthWest" },
    { cls: "resize-grip--se", dir: "SouthEast" },
    { cls: "resize-grip--sw", dir: "SouthWest" },
  ];
  return (
    <>
      {grips.map((g) => (
        <div
          key={g.cls}
          className={`resize-grip ${g.cls}`}
          onPointerDown={startResize(g.dir)}
        />
      ))}
    </>
  );
}

function MainView() {
  const [repos, setRepos] = useState<string[]>([]);
  const [picker, setPicker] = useState<Picker | null>(null);
  // Which thing in the overlay is expanded: the idea box, one issue row, or none.
  const [expanded, setExpanded] = useState<Expand>(null);
  // The multi-line idea box; resized to fit its content (capped in CSS).
  const ideaRef = useRef<HTMLTextAreaElement | null>(null);

  const [sessions, setSessions] = useState<Session[]>([]);
  // PR link per session id, discovered live from GitHub. `null` = looked up, none
  // found (or the lookup failed); absent key = not looked up yet.
  const [prs, setPrs] = useState<Record<string, PrLink | null>>({});
  // Local git state per session id (commits ahead / dirty), used to derive the
  // work-lifecycle phase. Refreshed alongside the PR lookup on popover open.
  const [workStates, setWorkStates] = useState<Record<string, WorkState | null>>({});
  // Live status per workspace id (busy / needs_you / idle / …), seeded on open
  // and kept current by the backend's `session-status` event.
  const [statuses, setStatuses] = useState<Record<string, StatusRecord>>({});
  // Session id whose inline command strip is expanded (only one at a time).
  const [commandsOpen, setCommandsOpen] = useState<string | null>(null);
  // Pending Tear Down prompt for a session: either a warnings confirmation, or a
  // "VS Code still open" block that can offer the Accessibility shortcut.
  const [teardownConfirm, setTeardownConfirm] = useState<TeardownPrompt | null>(null);
  // Session ids whose teardown call is currently in flight, so the triggering
  // button glows while the backend removes the worktree.
  const [teardownBusy, setTeardownBusy] = useState<Record<string, boolean>>({});
  // Create-PR progress/error per session id: `{ creating }` while in flight,
  // `{ error }` after a failure. Absent = idle. `requestId` correlates
  // `claude-activity` events to this action's busy glow.
  const [prCreate, setPrCreate] = useState<Record<string, { creating?: boolean; requestId?: string; error?: string }>>({});
  // PR check status per session id, polled from GitHub while the popover is open.
  // Absent = not yet fetched; null = no open PR (or lookup failed).
  const [prChecks, setPrChecks] = useState<Record<string, PrChecks | null>>({});
  // Merge-PR state per session id. `intent` keeps the auto-merge watcher armed
  // until the PR lands; `merging` guards against overlapping merge attempts.
  // `requestId` correlates `claude-activity` events (the merge drafts the PR
  // via Claude when none exists yet) to this action's busy glow.
  const [prMerge, setPrMerge] = useState<Record<string, { intent?: boolean; merging?: boolean; requestId?: string; error?: string }>>({});
  // Request ids with a Claude call currently in flight (`claude-activity`
  // events). A busy button whose request id is here glows rainbow instead of
  // the monochrome sweep; absent = plain. Missed events degrade to monochrome.
  const [aiActive, setAiActive] = useState<Record<string, boolean>>({});
  // Whether the menu-bar popover is currently open. Gates check polling so we
  // don't hit GitHub while the window is hidden.
  const [popoverOpen, setPopoverOpen] = useState(true);
  // Global toggle: reveal hidden/snoozed repos and work items (dimmed).
  const [showHidden, setShowHidden] = useState(false);
  // Per-repo settings, keyed by repo full_name — the source of repo-level hide state.
  const [repoSettings, setRepoSettings] = useState<Record<string, RepoSettings>>({});
  // Open repo options menu (repo full_name); at most one at a time.
  const [repoMenuOpen, setRepoMenuOpen] = useState<string | null>(null);
  // Per-repo "Open in VS Code" launch error, keyed by repo full_name.
  const [openRepoErr, setOpenRepoErr] = useState<Record<string, string>>({});
  // Target of the hide/snooze dialog, or null when closed.
  const [hideTarget, setHideTarget] = useState<HideTarget | null>(null);
  // Repo (full_name) awaiting remove confirmation; at most one at a time.
  const [removeConfirm, setRemoveConfirm] = useState<string | null>(null);
  // Per-repo removal failure message, keyed by repo full_name.
  const [removeErr, setRemoveErr] = useState<Record<string, string>>({});

  const refreshSessions = useCallback(() => {
    api.sessionsList().then((list) => {
      setSessions(list);
      // Fire PR lookups in parallel; each settles its own entry and soft-fails,
      // so rows render immediately and a PR button appears as its lookup lands.
      for (const s of list) {
        api.sessionPr(s.id)
          .then((pr) => setPrs((prev) => ({ ...prev, [s.id]: pr })))
          .catch(() => setPrs((prev) => ({ ...prev, [s.id]: null })));
        api.sessionWorkState(s.id)
          .then((ws) => setWorkStates((prev) => ({ ...prev, [s.id]: ws })))
          .catch(() => setWorkStates((prev) => ({ ...prev, [s.id]: null })));
      }
    }).catch(() => {});
  }, []);

  // Replace the status map with a fresh snapshot from disk. Called on open so a
  // reopened/reloaded popover reflects current state even if it missed events.
  const refreshStatuses = useCallback(() => {
    api.sessionsStatusList().then((list) => {
      const next: Record<string, StatusRecord> = {};
      for (const s of list) next[s.workspace] = s;
      setStatuses(next);
    }).catch(() => {});
  }, []);

  const refreshAll = useCallback(() => {
    api.listRepos().then((list) => {
      setRepos(list);
      // Fan out per-repo settings so repo-level hide state lands as each settles,
      // mirroring how PR lookups are fired in refreshSessions.
      for (const repo of list) {
        api.getRepoSettings(repo)
          .then((s) => setRepoSettings((prev) => ({ ...prev, [repo]: s })))
          .catch(() => {});
      }
    }).catch(() => {});
    refreshSessions();
    refreshStatuses();
  }, [refreshSessions, refreshStatuses]);

  useEffect(() => {
    refreshAll();
  }, [refreshAll]);

  // The popover hides on blur, so each tray-icon click is a fresh open. Re-fetch
  // on the backend's "popover-shown" event so it never displays stale sessions
  // or PR state after work happened (or windows closed) while it was hidden.
  useEffect(() => {
    const unlisten = getCurrentWindow().listen("popover-shown", refreshAll);
    return () => { unlisten.then((f) => f()); };
  }, [refreshAll]);

  // Live status updates from the backend's status-file watcher. Each event is one
  // workspace's latest record; `ended` clears its row indicator.
  useEffect(() => {
    const unlisten = getCurrentWindow().listen<StatusRecord>("session-status", (e) => {
      const rec = e.payload;
      setStatuses((prev) => {
        if (rec.state === "ended") {
          const { [rec.workspace]: _drop, ...rest } = prev;
          return rest;
        }
        return { ...prev, [rec.workspace]: rec };
      });
    });
    return () => { unlisten.then((f) => f()); };
  }, []);

  // Live Claude-call signal from the backend: while a request id is active its
  // button's busy glow turns rainbow (AI), reverting to the monochrome sweep
  // when the call ends — so mixed script/AI actions change color mid-flight.
  useEffect(() => {
    const unlisten = getCurrentWindow().listen<{ request_id: string; active: boolean }>("claude-activity", (e) => {
      const { request_id, active } = e.payload;
      setAiActive((prev) => {
        if (!active) {
          const { [request_id]: _drop, ...rest } = prev;
          return rest;
        }
        return { ...prev, [request_id]: true };
      });
    });
    return () => { unlisten.then((f) => f()); };
  }, []);

  // Busy classes for a button whose backend command can run Claude: rainbow
  // while its request id has a Claude call in flight, monochrome otherwise.
  const busyCls = (requestId?: string) =>
    requestId && aiActive[requestId] ? "btn-busy btn-busy--ai" : "btn-busy";

  // Busy-ring classes for the row's "working" pill: rainbow (AI) while the
  // operation's Claude call is in flight, single-hue (non-AI) otherwise — so a
  // Create PR glows rainbow while Claude drafts the body, then reverts for the
  // git push/merge. Teardown passes no request id and stays monochrome.
  const busyRingCls = (requestId?: string) =>
    requestId && aiActive[requestId] ? "busy-ring busy-ring--ai" : "busy-ring";

  // Also refresh whenever the window itself regains focus — covers any path that
  // re-focuses the popover without a fresh "popover-shown" emit. Refresh is
  // idempotent, so overlapping with the event above is harmless.
  useEffect(() => {
    const unlisten = getCurrentWindow().onFocusChanged(({ payload: focused }) => {
      if (focused) refreshAll();
    });
    return () => { unlisten.then((f) => f()); };
  }, [refreshAll]);

  // Grow the idea box to fit its content (reset to auto first so it can also
  // shrink), capped by the CSS max-height which then scrolls.
  useEffect(() => {
    const el = ideaRef.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${el.scrollHeight}px`;
  }, [picker?.query, expanded]);

  // Tracked sessions grouped by repo full_name.
  const sessionsByRepo = useMemo(() => {
    const grouped: Record<string, Session[]> = {};
    for (const s of sessions) (grouped[s.repo] ??= []).push(s);
    return grouped;
  }, [sessions]);

  function closePicker() {
    setPicker(null);
    setExpanded(null);
  }

  async function openStartWork(repo: string) {
    setExpanded(null);
    setPicker({ repo, loading: true, issues: null, query: "" });
    try {
      const settings = await api.getRepoSettings(repo);
      if (!settings.identity_id) {
        setPicker({ repo, loading: false, issues: null, query: "", error: "No identity assigned. Set one in Settings → Repo." });
        return;
      }
      const issues = await api.githubListIssues(settings.identity_id, repo);
      setPicker({ repo, loading: false, issues, query: "" });
    } catch (e) {
      setPicker({ repo, loading: false, issues: null, query: "", error: String(e) });
    }
  }

  // Re-fetch the repo's open issues (and sessions, so "working" pills are
  // current) without closing the overlay. Best-effort: keep the list on failure.
  async function refreshIssues() {
    if (!picker) return;
    const repo = picker.repo;
    refreshSessions();
    setPicker((p) => (p ? { ...p, refreshing: true } : p));
    try {
      const settings = await api.getRepoSettings(repo);
      if (settings.identity_id) {
        const issues = await api.githubListIssues(settings.identity_id, repo);
        setPicker((p) => (p && p.repo === repo ? { ...p, issues } : p));
      }
    } catch { /* keep the existing list */ }
    setPicker((p) => (p ? { ...p, refreshing: false } : p));
  }

  // Turn a backend SpawnPlan into the editable preview state. `mode` decides
  // what the preview offers: "spawn" shows the session name + derived slugs and
  // creates a worktree; "create" just opens the issue (no worktree).
  function planToPreview(plan: SpawnPlan, mode: "spawn" | "create"): Preview {
    return {
      mode,
      issueNumber: plan.issue_number,
      issueTitle: plan.issue_title,
      issueBody: plan.issue_body,
      shortTitle: plan.short_title,
      color: plan.color,
      emoji: plan.emoji,
      repoName: plan.repo_name,
      origTitle: plan.issue_title,
      origBody: plan.issue_body,
      spawning: false,
    };
  }

  // Patch fields of the open preview.
  function setPreview(patch: Partial<Preview>) {
    setPicker((p) => (p && p.preview ? { ...p, preview: { ...p.preview, ...patch } } : p));
  }

  // Existing issue → open the spawn preview (no worktree yet; fetches the body).
  async function spawnIssue(node: IssueNode) {
    if (!picker) return;
    const repo = picker.repo;
    setPicker((p) => (p ? { ...p, preparing: node.number, note: `Preparing #${node.number}…` } : p));
    try {
      const plan = await api.prepareSpawn(repo, node.number);
      // `suggesting: true` shows the "generating title" indicator on the Session
      // name field until the AI suggestion lands (or the call fails).
      setPicker((p) => (p ? { ...p, preparing: undefined, note: undefined, preview: { ...planToPreview(plan, "spawn"), suggesting: true } } : p));
      // Fire-and-forget: upgrade the heuristic label to an AI suggestion once
      // Claude replies. The preview is already open and usable meanwhile.
      void suggestLabel(repo, node.number, plan.short_title);
    } catch (e) {
      setPicker((p) => (p ? { ...p, preparing: undefined, note: `Failed to prepare #${node.number}: ${String(e)}` } : p));
    }
  }

  // Swap the spawn preview's short label for Claude's suggestion — only if the
  // preview is still open for this same issue and the field still holds the
  // heuristic value (the user hasn't typed). Failures are silent: the
  // heuristic label is a fine fallback.
  async function suggestLabel(repo: string, issueNumber: number, heuristic: string) {
    let suggestion = "";
    try {
      suggestion = await api.suggestShortTitle(repo, issueNumber);
    } catch { /* keep the heuristic label */ }
    // Clear the "generating title" indicator and, if the preview is still open
    // for this same issue and untouched, swap in the suggestion. Runs on both
    // success and failure so the indicator never sticks.
    setPicker((p) => {
      const pv = p?.preview;
      if (!p || !pv || p.repo !== repo || pv.mode !== "spawn" || pv.issueNumber !== issueNumber) return p;
      const useSuggestion = !!suggestion.trim() && pv.shortTitle === heuristic;
      return { ...p, preview: { ...pv, suggesting: false, shortTitle: useSuggestion ? suggestion : pv.shortTitle } };
    });
  }

  function applyDraftPreview(res: DraftPreviewOutcome, idea: string, mode: "spawn" | "create") {
    if (res.status === "needs_confirmation") {
      setPicker((p) => (p ? { ...p, creating: undefined, creatingRequestId: undefined, note: undefined, confirm: { idea, message: res.message, action: mode } } : p));
    } else {
      setPicker((p) => (p ? { ...p, creating: undefined, creatingRequestId: undefined, note: undefined, confirm: undefined, preview: planToPreview(res, mode) } : p));
    }
  }

  // Cancel the preview → back to the issue list (overlay stays open).
  function cancelPreview() {
    setPicker((p) => (p ? { ...p, preview: undefined, note: undefined } : p));
  }

  // Confirm a spawn preview: create/update the issue, then spawn. On success the
  // overlay closes and we land back in the main window.
  async function confirmSpawnNow() {
    if (!picker?.preview) return;
    const repo = picker.repo;
    const pv = picker.preview;
    if (!pv.shortTitle.trim() || !pv.issueTitle.trim()) return;
    setPreview({ spawning: true, error: undefined });
    const edits: SpawnEdits = {
      issue_number: pv.issueNumber,
      issue_title: pv.issueTitle,
      issue_body: pv.issueBody,
      short_title: pv.shortTitle,
      color: pv.color,
      emoji: pv.emoji,
      update_issue: pv.issueNumber !== null && (pv.issueTitle !== pv.origTitle || pv.issueBody !== pv.origBody),
    };
    try {
      // confirm_spawn now returns as soon as the worktree build is handed to a
      // background task, so this resolves quickly: close the overlay and land on
      // the dashboard, where the new row shows a "Creating…" pill (driven by the
      // backend's `creating` status) until the worktree is ready. A thrown error
      // here is a synchronous failure (issue create/update, identity) — keep the
      // overlay open and show it.
      await api.confirmSpawn(repo, edits);
      refreshSessions();
      refreshStatuses();
      closePicker();
    } catch (e) {
      setPreview({ spawning: false, error: String(e) });
    }
  }

  // Confirm a create-only preview: open the issue from the edited title/body,
  // surface it in the list (optimistically + a reconciling refetch), and return
  // to the list. No worktree is created.
  async function createFromPreview() {
    if (!picker?.preview) return;
    const repo = picker.repo;
    const pv = picker.preview;
    if (!pv.issueTitle.trim()) return;
    setPreview({ spawning: true, error: undefined });
    try {
      const res = await api.createIssueDirect(repo, pv.issueTitle, pv.issueBody);
      if (res.status !== "created") {
        setPreview({ spawning: false, error: "Unexpected response while creating the issue." });
        return;
      }
      const node: IssueNode = { number: res.number, title: pv.issueTitle, html_url: res.issue_url, children: [] };
      setPicker((p) => {
        if (!p) return p;
        // Insert the new issue locally — we know exactly what it is, and GitHub's
        // list endpoint lags, so refetching would briefly drop it. The next
        // popover refresh reconciles with GitHub.
        const present = (p.issues ?? []).some((n) => n.number === res.number);
        const issues = present ? p.issues : [node, ...(p.issues ?? [])];
        return { ...p, preview: undefined, query: "", note: `Created ${res.issue_url}`, issues };
      });
    } catch (e) {
      setPreview({ spawning: false, error: String(e) });
    }
  }

  // The preview's primary button: spawn or create-only depending on its mode.
  function confirmPreview() {
    if (picker?.preview?.mode === "create") createFromPreview();
    else confirmSpawnNow();
  }

  // Idea → AI draft → create-only preview (issue isn't opened until confirm).
  async function createIssueOnly() {
    if (!picker) return;
    const repo = picker.repo;
    const idea = picker.query.trim();
    if (!idea) return;
    const requestId = crypto.randomUUID();
    setPicker((p) => (p ? { ...p, creating: "create", creatingRequestId: requestId, note: `Drafting an issue for “${idea}”…`, confirm: undefined } : p));
    try {
      applyDraftPreview(await api.draftSpawnPreview(repo, idea, requestId), idea, "create");
    } catch (e) {
      setPicker((p) => (p ? { ...p, creating: undefined, creatingRequestId: undefined, note: `Failed to draft issue: ${String(e)}` } : p));
    }
  }

  // Idea → AI draft → spawn preview (issue isn't opened until confirm).
  async function createAndSpawn() {
    if (!picker) return;
    const repo = picker.repo;
    const idea = picker.query.trim();
    if (!idea) return;
    const requestId = crypto.randomUUID();
    setPicker((p) => (p ? { ...p, creating: "spawn", creatingRequestId: requestId, note: `Drafting an issue for “${idea}”…`, confirm: undefined } : p));
    try {
      applyDraftPreview(await api.draftSpawnPreview(repo, idea, requestId), idea, "spawn");
    } catch (e) {
      setPicker((p) => (p ? { ...p, creating: undefined, creatingRequestId: undefined, note: `Failed to draft issue: ${String(e)}` } : p));
    }
  }

  // User chose to proceed from their raw text despite Claude's prompt. Repeats
  // whichever action raised it, landing on its preview built from the raw text.
  async function confirmRaw() {
    if (!picker?.confirm) return;
    const repo = picker.repo;
    const { idea, action } = picker.confirm;
    const requestId = crypto.randomUUID();
    setPicker((p) => (p ? { ...p, creating: action, creatingRequestId: requestId, note: "Drafting from your text…", confirm: undefined } : p));
    try {
      applyDraftPreview(await api.draftSpawnPreview(repo, idea, requestId, true), idea, action);
    } catch (e) {
      setPicker((p) => (p ? { ...p, creating: undefined, creatingRequestId: undefined, note: `Failed: ${String(e)}` } : p));
    }
  }

  // "Create PR": push the branch, draft a description with Claude, and open a
  // draft PR. On success the PR pill refreshes to link the new PR (we don't
  // open it in the browser — the pill is the entry point).
  async function createPr(s: Session) {
    const requestId = crypto.randomUUID();
    setPrCreate((prev) => ({ ...prev, [s.id]: { creating: true, requestId } }));
    try {
      const pr = await api.createPr(s.id, requestId);
      setPrs((prev) => ({ ...prev, [s.id]: pr }));
      setPrCreate((prev) => ({ ...prev, [s.id]: {} }));
    } catch (e) {
      setPrCreate((prev) => ({ ...prev, [s.id]: { error: String(e) } }));
    }
  }

  // One merge attempt for a session: ensure-create + mark-ready + merge-if-clean
  // happen in the backend. A landed PR (`state === "merged"`) clears the intent;
  // a PR that isn't mergeable yet comes back unmerged and stays armed for the
  // next poll to retry; a hard failure (conflict, auth) surfaces in the panel.
  const runMerge = useCallback(async (id: string) => {
    const requestId = crypto.randomUUID();
    setPrMerge((prev) => ({ ...prev, [id]: { ...prev[id], intent: true, merging: true, requestId, error: undefined } }));
    try {
      const pr = await api.mergePr(id, requestId);
      setPrs((prev) => ({ ...prev, [id]: pr }));
      setPrMerge((prev) =>
        pr.state === "merged"
          ? { ...prev, [id]: {} }
          : { ...prev, [id]: { ...prev[id], merging: false } },
      );
    } catch (e) {
      setPrMerge((prev) => ({ ...prev, [id]: { error: String(e) } }));
    }
  }, []);

  // "Merge PR": arm the auto-merge watcher and take the first attempt now, which
  // creates the PR if missing and promotes a draft so its checks start running.
  function startMerge(s: Session) {
    void runMerge(s.id);
  }

  // Latest sessions/PR/merge state reachable from the polling interval without
  // re-arming it on every keystroke of state.
  const pollRef = useRef({ sessions, prs, prMerge });
  pollRef.current = { sessions, prs, prMerge };

  // Poll check status for every session that has a PR or an armed merge, and let
  // a poll that finds the PR mergeable drive the next auto-merge attempt (so
  // retries are naturally paced to the poll, not a render loop).
  const pollChecks = useCallback(() => {
    const { sessions, prs, prMerge } = pollRef.current;
    for (const s of sessions) {
      if (!prs[s.id] && !prMerge[s.id]?.intent) continue;
      api.sessionPrChecks(s.id)
        .then((c) => {
          setPrChecks((prev) => ({ ...prev, [s.id]: c }));
          const m = pollRef.current.prMerge[s.id];
          if (!c || !m?.intent || m.merging) return;
          if (c.ready_to_merge) {
            void runMerge(s.id);
            return;
          }
          // Stop waiting on a blocker the user must clear; otherwise keep polling.
          const blocker = mergeBlocker(c);
          if (blocker) setPrMerge((prev) => ({ ...prev, [s.id]: { error: blocker } }));
        })
        .catch(() => {});
    }
  }, [runMerge]);

  // Run the poll on an interval, but only while the popover is open.
  useEffect(() => {
    if (!popoverOpen) return;
    pollChecks();
    const id = setInterval(pollChecks, 6000);
    return () => clearInterval(id);
  }, [popoverOpen, pollChecks]);

  // Track popover open/close: the backend emits "popover-shown" on each show,
  // and the window blurs (hides) when it closes.
  useEffect(() => {
    const win = getCurrentWindow();
    const shown = win.listen("popover-shown", () => setPopoverOpen(true));
    const focus = win.onFocusChanged(({ payload }) => { if (!payload) setPopoverOpen(false); });
    return () => { shown.then((f) => f()); focus.then((f) => f()); };
  }, []);

  // Run teardown and route its outcome to the right prompt: a warnings
  // confirmation, a "VS Code still open" block, or success (dismiss + refresh).
  // `confirmed` skips the work-state checks; `force` skips closing the editor.
  async function runTeardown(id: string, confirmed: boolean, force: boolean) {
    setTeardownBusy((prev) => ({ ...prev, [id]: true }));
    try {
      const res = await api.teardown(id, confirmed, force);
      if (res.status === "needs_confirmation") {
        setTeardownConfirm({ id, kind: "confirm", warnings: res.warnings });
      } else if (res.status === "blocked_by_editor") {
        setTeardownConfirm({ id, kind: "blocked", message: res.message, accessibility: res.accessibility });
      } else {
        setTeardownConfirm(null);
        setCommandsOpen((open) => (open === id ? null : open));
        refreshSessions();
      }
    } catch (e) {
      setTeardownConfirm({ id, kind: "confirm", warnings: [`Teardown failed: ${String(e)}`] });
    } finally {
      setTeardownBusy((prev) => {
        const { [id]: _done, ...rest } = prev;
        return rest;
      });
    }
  }

  // "Tear Down": remove the worktree. Confirms first unless the backend says
  // it's safe (PR merged, nothing new).
  function tearDown(s: Session) {
    setTeardownConfirm(null);
    runTeardown(s.id, false, false);
  }

  // Untrack a repo (delete its settings file). Worktrees and session records
  // stay on disk; the repo and its work items just drop out of the dashboard.
  async function removeRepo(repo: string) {
    setRemoveConfirm(null);
    setRemoveErr((e) => { const { [repo]: _, ...rest } = e; return rest; });
    try {
      await api.removeRepo(repo);
      setRepos((prev) => prev.filter((r) => r !== repo));
    } catch (err) {
      setRemoveErr((e) => ({ ...e, [repo]: String(err) }));
    }
    refreshAll();
  }

  // Set or clear (hidden = null → unhide) hide/snooze state for a repo or work
  // item, then refresh so the dimming/filtering reflects the new state.
  async function applyVisibility(target: HideTarget, hidden: HideState | null) {
    try {
      if (target.kind === "repo") {
        await api.setRepoVisibility(target.repo, hidden);
      } else {
        await api.setSessionVisibility(target.session.id, hidden);
      }
    } catch { /* best-effort; the refresh below reflects the real state */ }
    setHideTarget(null);
    setRepoMenuOpen(null);
    setCommandsOpen(null);
    refreshAll();
  }

  const now = Date.now();

  // The lifecycle zone a session sorts into. An unresolved work state defaults to
  // Planning until the local-git lookup lands (then the item slides if it moved).
  const zoneOf = (s: Session): LifecyclePhase =>
    lifecyclePhase(prs[s.id], workStates[s.id]) ?? "planning";

  return (
    <main className="panel">
      <ResizeGrips />
      <header className="panel-header">
        <LogoIcon className="panel-logo" aria-hidden="true" />
        <h1>m<span className="ai">AI</span>estro</h1>
        <button
          className={`icon-btn ${showHidden ? "icon-btn--active" : ""}`}
          onClick={() => setShowHidden((v) => !v)}
          title={showHidden ? "Hide hidden items" : "Show hidden items"}
          aria-label="Toggle hidden items"
          aria-pressed={showHidden}
        >
          <EyeIcon />
        </button>
        <button className="icon-btn" onClick={openSettings} title="Settings" aria-label="Settings">
          <GearIcon />
        </button>
      </header>

      <div className="work-list">
        {repos.length === 0 ? (
          <div className="empty-state">
            <p className="empty-state-title">No repos yet</p>
            <p className="empty-state-body">Add a repo in Settings → Repo, then start work on its issues.</p>
          </div>
        ) : (
          repos.map((repo) => {
            const repoSessions = sessionsByRepo[repo] ?? [];
            const repoHide = repoSettings[repo]?.hidden ?? null;
            const repoHidden = effectiveHidden(repoHide, now);
            // With "Show hidden" off, a hidden repo drops out entirely (its work
            // items go with it). With it on, the repo and its items render dimmed.
            if (repoHidden && !showHidden) return null;
            const repoSnoozeLabel = repoHide?.snooze_until
              ? formatSnoozeRemaining(repoHide.snooze_until, now)
              : "";
            const repoMenu = repoMenuOpen === repo;
            const visibleSessions = showHidden
              ? repoSessions
              : repoSessions.filter((s) => !effectiveHidden(s.hidden, now));
            return (
              <div key={repo} className={`repo-group ${repoHidden ? "repo-group--hidden" : ""}`}>
                <div className="repo-group-header">
                  <span className="repo-group-name">{repo}</span>
                  {repoHidden && (
                    <button
                      className="snooze-label"
                      onClick={() => setRepoMenuOpen((r) => (r === repo ? null : repo))}
                      title="Manage visibility"
                      aria-expanded={repoMenu}
                    >
                      {repoSnoozeLabel ? `Snoozed · ${repoSnoozeLabel}` : "Hidden"}
                    </button>
                  )}
                  <button
                    className="pill-btn repo-open-editor"
                    onClick={async () => {
                      setOpenRepoErr((e) => { const { [repo]: _, ...rest } = e; return rest; });
                      try {
                        await api.openRepoInEditor(repo);
                      } catch (err) {
                        setOpenRepoErr((e) => ({ ...e, [repo]: String(err) }));
                      }
                    }}
                    title="Open cloned repo in VS Code"
                    aria-label="Open cloned repo in VS Code"
                  >
                    <VSCodeIcon />
                  </button>
                  <button className="btn-add" onClick={() => openStartWork(repo)}>
                    Start Work
                  </button>
                  <button
                    className={`row-expander ${repoMenu ? "row-expander--open" : ""}`}
                    onClick={() => setRepoMenuOpen((r) => (r === repo ? null : repo))}
                    title="Repo options"
                    aria-label="Repo options"
                    aria-expanded={repoMenu}
                  >
                    <ChevronRightIcon />
                  </button>
                </div>
                <div className={`command-strip ${repoMenu ? "command-strip--open" : ""}`}>
                  {repoHidden ? (
                    <button className="command-btn" onClick={() => applyVisibility({ kind: "repo", repo }, null)}>
                      Unhide
                    </button>
                  ) : (
                    <button className="command-btn" onClick={() => { setRepoMenuOpen(null); setHideTarget({ kind: "repo", repo }); }}>
                      Hide…
                    </button>
                  )}
                  <button className="command-btn" onClick={() => { setRepoMenuOpen(null); setRemoveConfirm(repo); }}>
                    Remove…
                  </button>
                </div>
                {removeConfirm === repo && (
                  <div className="cleanup-confirm">
                    <p className="cleanup-lead">Remove {repo} from mAIestro?</p>
                    <p className="cleanup-confirm-body">
                      Worktrees, cloned repos, and any work in them are kept on disk.
                    </p>
                    <div className="issue-actions">
                      <button className="btn-danger" onClick={() => removeRepo(repo)}>Remove</button>
                      <button className="btn-ghost" onClick={() => setRemoveConfirm(null)}>Cancel</button>
                    </div>
                  </div>
                )}
                {removeErr[repo] && (
                  <div className="cleanup-confirm">
                    <p className="cleanup-lead">Couldn't remove repo</p>
                    <pre className="tool-error-message">{removeErr[repo]}</pre>
                    <div className="issue-actions">
                      <button
                        className="btn-ghost"
                        onClick={() => setRemoveErr((e) => { const { [repo]: _, ...rest } = e; return rest; })}
                      >
                        Dismiss
                      </button>
                    </div>
                  </div>
                )}
                {openRepoErr[repo] && (
                  <div className="cleanup-confirm">
                    <p className="cleanup-lead">Couldn't open in VS Code</p>
                    <pre className="tool-error-message">{openRepoErr[repo]}</pre>
                    <div className="issue-actions">
                      <button
                        className="btn-ghost"
                        onClick={() => setOpenRepoErr((e) => { const { [repo]: _, ...rest } = e; return rest; })}
                      >
                        Dismiss
                      </button>
                    </div>
                  </div>
                )}

                {visibleSessions.length === 0 ? (
                  <p className="repo-group-empty">No active work</p>
                ) : (
                  ZONES.map((zone) => {
                    const zoneItems = visibleSessions.filter((s) => zoneOf(s) === zone);
                    if (zoneItems.length === 0) return null;
                    return (
                      <div key={zone} className={`lifecycle-zone lifecycle-zone--${zone}`}>
                        <div className="lifecycle-zone-header">
                          <span className="lifecycle-zone-dot" />
                          <span className="lifecycle-zone-name">{LIFECYCLE_LABELS[zone]}</span>
                        </div>
                        {zoneItems.map((s) => {
                    const cmdOpen = commandsOpen === s.id;
                    // While the worktree is still being built in the background
                    // (issue #77), actions that need it to exist are disabled.
                    const creating = statuses[s.id]?.state === "creating";
                    // Only surfaced errors render; pending/transient ones (Claude
                    // may still recover) stay hidden until promoted (issue #48).
                    const toolErr = statuses[s.id]?.last_error?.surfaced
                      ? statuses[s.id]?.last_error
                      : undefined;
                    const pr = prs[s.id];
                    const PrIcon = pr ? (PR_STATE_ICONS[pr.state] ?? PrOpenIcon) : null;
                    const sessHidden = effectiveHidden(s.hidden, now);
                    const sessSnoozeLabel = s.hidden?.snooze_until
                      ? formatSnoozeRemaining(s.hidden.snooze_until, now)
                      : "";
                    // An active PR (open or still a draft) already covers this branch,
                    // so disable Create PR; the pill links to it.
                    const prOpen = pr?.state === "open" || pr?.state === "draft";
                    const prc = prCreate[s.id];
                    const checks = prChecks[s.id];
                    const pm = prMerge[s.id];
                    // A mAIestro operation in flight on this row. The rainbow
                    // "working" pill keeps the feedback visible after the command
                    // strip collapses on click; mirrors the command buttons' busy
                    // flags so it clears on completion or failure. `creating` is
                    // the background worktree build (issue #77).
                    const opLabel = creating
                      ? "Creating…"
                      : prc?.creating
                        ? "Creating PR…"
                        : pm?.intent && pr?.state !== "merged"
                          ? "Merging…"
                          : teardownBusy[s.id]
                            ? "Tearing down…"
                            : null;
                    // Request id of the in-flight op (if it can run Claude), so the
                    // op-pill glows rainbow only while that AI call is active.
                    // Teardown is pure git, so it has none.
                    const opRequestId = prc?.creating
                      ? prc.requestId
                      : pm?.intent && pr?.state !== "merged"
                        ? pm.requestId
                        : undefined;
                    return (
                    <div key={s.id} className={`workspace-item ${sessHidden || repoHidden ? "workspace-item--hidden" : ""}`}>
                      <div className="workspace-row" style={{ borderLeft: `3px solid ${accentColor(s.color)}` }}>
                        <ClaudePill status={statuses[s.id]} onClick={() => { if (!creating) api.openInEditor(s.work_dir); }} />
                        <span className="workspace-title">{s.session_title}</span>
                        {opLabel && <span className={`workspace-op-pill ${busyRingCls(opRequestId)}`}>{opLabel}</span>}
                        {sessHidden && (
                          <button
                            className="snooze-label"
                            onClick={() => setCommandsOpen((id) => (id === s.id ? null : s.id))}
                            title="Manage visibility"
                            aria-expanded={cmdOpen}
                          >
                            {sessSnoozeLabel ? `Snoozed · ${sessSnoozeLabel}` : "Hidden"}
                          </button>
                        )}
                        <div className="session-pill">
                          {pr && PrIcon && (
                            <button
                              className="pill-btn pill-btn--pr"
                              onClick={() => api.openUrl(pr.html_url)}
                              title={`Open PR #${pr.number} (${pr.state}) on GitHub`}
                              aria-label="Open pull request on GitHub"
                            >
                              <PrIcon />
                              #{pr.number}
                              {checks && checks.state !== "none" && (
                                <span
                                  className={`check-dot check-dot--${checks.state}${checks.running ? " check-dot--spin" : ""}`}
                                  title={checkLabel(checks)}
                                  aria-label={checkLabel(checks)}
                                />
                              )}
                            </button>
                          )}
                          <button
                            className="pill-btn"
                            onClick={() => api.openUrl(s.issue_url)}
                            title={`Open issue #${s.issue_number} on GitHub`}
                          >
                            <GitHubIcon />
                            #{s.issue_number}
                          </button>
                          <button
                            className="pill-btn"
                            onClick={() => api.openPath(s.work_dir)}
                            disabled={creating}
                            title={creating ? "Still creating this workspace…" : `Reveal in Finder: ${s.work_dir}`}
                            aria-label="Open folder in Finder"
                          >
                            <FolderIcon />
                          </button>
                          <button
                            className="pill-btn"
                            onClick={() => api.openInEditor(s.work_dir)}
                            disabled={creating}
                            title={creating ? "Still creating this workspace…" : "Open in VS Code"}
                            aria-label="Open in VS Code"
                          >
                            <VSCodeIcon style={{ color: accentColor(s.color) }} />
                          </button>
                        </div>
                        <button
                          className={`row-expander ${cmdOpen ? "row-expander--open" : ""}`}
                          onClick={() => setCommandsOpen((id) => (id === s.id ? null : s.id))}
                          title="Commands"
                          aria-label="Commands"
                          aria-expanded={cmdOpen}
                        >
                          <ChevronRightIcon />
                        </button>
                      </div>
                      <div className={`command-strip ${cmdOpen ? "command-strip--open" : ""}`}>
                        <button
                          className={`command-btn ${prc?.creating ? busyCls(prc.requestId) : ""}`}
                          onClick={() => { setCommandsOpen(null); createPr(s); }}
                          disabled={prc?.creating || prOpen || creating}
                          title={creating ? "Still creating this workspace…" : prOpen ? `PR #${pr?.number} is already open` : undefined}
                        >
                          {prc?.creating ? "Creating PR…" : "Create PR"}
                        </button>
                        <button
                          className={`command-btn ${pm?.intent && pr?.state !== "merged" ? busyCls(pm.requestId) : ""}`}
                          onClick={() => { setCommandsOpen(null); startMerge(s); }}
                          disabled={pm?.intent || pr?.state === "merged" || creating}
                          title={
                            creating
                              ? "Still creating this workspace…"
                              : pr?.state === "merged"
                                ? `PR #${pr.number} is already merged`
                                : "Create the PR if needed, wait for checks, then merge"
                          }
                        >
                          {pr?.state === "merged" ? "Merged" : pm?.intent ? "Merging…" : "Merge PR"}
                        </button>
                        {sessHidden ? (
                          <button className="command-btn" onClick={() => applyVisibility({ kind: "session", session: s }, null)}>Unhide</button>
                        ) : (
                          <button className="command-btn" onClick={() => { setCommandsOpen(null); setHideTarget({ kind: "session", session: s }); }}>Hide…</button>
                        )}
                        <button
                          className={`command-btn ${teardownBusy[s.id] ? "btn-busy" : ""}`}
                          disabled={teardownBusy[s.id] || creating}
                          title={creating ? "Still creating this workspace…" : undefined}
                          onClick={() => { setCommandsOpen(null); tearDown(s); }}
                        >
                          Tear Down
                        </button>
                      </div>
                      {prc?.error && (
                        <div className="cleanup-confirm">
                          <p className="cleanup-lead">Couldn't create the PR</p>
                          <ul className="cleanup-warnings">
                            <li>{prc.error}</li>
                          </ul>
                          <div className="issue-actions">
                            <button className="btn-ghost" onClick={() => setPrCreate((prev) => ({ ...prev, [s.id]: {} }))}>Dismiss</button>
                          </div>
                        </div>
                      )}
                      {pm?.error && (
                        <div className="cleanup-confirm">
                          <p className="cleanup-lead">Couldn't merge the PR</p>
                          <ul className="cleanup-warnings">
                            <li>{pm.error}</li>
                          </ul>
                          <div className="issue-actions">
                            <button className="btn-ghost" onClick={() => setPrMerge((prev) => ({ ...prev, [s.id]: {} }))}>Dismiss</button>
                          </div>
                        </div>
                      )}
                      {toolErr && (
                        <div className="cleanup-confirm">
                          <p className="cleanup-lead">{toolErr.tool ? `${toolErr.tool} failed` : "A tool call failed"}</p>
                          <pre className="tool-error-message">{toolErr.message}</pre>
                          <div className="issue-actions">
                            <button className="btn-ghost" onClick={() => api.clearSessionError(s.id)}>Dismiss</button>
                          </div>
                        </div>
                      )}
                      {teardownConfirm?.id === s.id && teardownConfirm.kind === "confirm" && (
                        <div className="cleanup-confirm">
                          <p className="cleanup-lead">Remove this workspace?</p>
                          <ul className="cleanup-warnings">
                            {teardownConfirm.warnings.map((w, i) => (
                              <li key={i}>{w}</li>
                            ))}
                          </ul>
                          <div className="issue-actions">
                            <button
                              className={`btn-danger ${teardownBusy[s.id] ? "btn-busy" : ""}`}
                              disabled={teardownBusy[s.id]}
                              onClick={() => runTeardown(s.id, true, false)}
                            >
                              Remove anyway
                            </button>
                            <button className="btn-ghost" onClick={() => setTeardownConfirm(null)}>Cancel</button>
                          </div>
                        </div>
                      )}
                      {teardownConfirm?.id === s.id && teardownConfirm.kind === "blocked" && (
                        <div className="cleanup-confirm">
                          <p className="cleanup-lead" style={{ whiteSpace: "pre-line" }}>{teardownConfirm.message}</p>
                          <div className="issue-actions">
                            <button className="btn-ghost" onClick={() => api.openInEditor(s.work_dir)}>Open in VS Code</button>
                            {teardownConfirm.accessibility && (
                              <button className="btn-ghost" onClick={() => api.openAccessibilitySettings()}>Open Accessibility Options</button>
                            )}
                            <button
                              className={`btn-danger ${teardownBusy[s.id] ? "btn-busy" : ""}`}
                              disabled={teardownBusy[s.id]}
                              onClick={() => runTeardown(s.id, true, true)}
                            >
                              Delete anyway
                            </button>
                            <button className="btn-ghost" onClick={() => setTeardownConfirm(null)}>Cancel</button>
                          </div>
                        </div>
                      )}
                    </div>
                    );
                        })}
                      </div>
                    );
                  })
                )}
              </div>
            );
          })
        )}
      </div>

      {picker && (() => {
        const filtered = filterIssues(picker.issues ?? [], picker.query);
        const ideaOpen = expanded?.kind === "idea";
        const busy = !!picker.creating;
        // Issues in this repo that already have a session, keyed by issue number,
        // carrying the workspace color and (joined) session title(s).
        const active: ActiveSessions = {};
        for (const s of sessionsByRepo[picker.repo] ?? []) {
          const phase = lifecyclePhase(prs[s.id], workStates[s.id]);
          const existing = active[s.issue_number];
          active[s.issue_number] = existing
            ? { color: existing.color, title: `${existing.title}, ${s.session_title}`, phase: furtherPhase(existing.phase, phase) }
            : { color: s.color, title: s.session_title, phase };
        }
        // Issues already being worked on sink to the end (stable sort keeps the
        // backend's most-recently-modified order within each group).
        const ordered = [...filtered].sort((a, b) => (active[a.number] ? 1 : 0) - (active[b.number] ? 1 : 0));
        return (
          <div className="overlay" onClick={closePicker}>
            <div className="overlay-panel" onClick={(e) => e.stopPropagation()}>
              <div className="overlay-header">
                <span className="overlay-title">{picker.preview ? (picker.preview.mode === "create" ? "New issue" : "Review & spawn") : "Start work"} · {picker.repo}</span>
                <button className="icon-btn" onClick={closePicker} aria-label="Close">✕</button>
              </div>
              {picker.preview ? (() => {
                const pv = picker.preview;
                const isSpawn = pv.mode === "spawn";
                // Derived names update live as the session name is edited. The
                // backend re-derives authoritatively (and resolves collisions).
                const slug = slugify(pv.shortTitle) || "…";
                // On the create path the issue number isn't known yet, so show a
                // `{issue #}` placeholder where it will be inserted on spawn.
                const numPart = pv.issueNumber != null ? String(pv.issueNumber) : "{issue #}";
                const workspace = `${numPart}-${slug}`;
                const issueDirty = pv.issueNumber != null && (pv.issueTitle !== pv.origTitle || pv.issueBody !== pv.origBody);
                const canConfirm = !!pv.issueTitle.trim() && (!isSpawn || !!pv.shortTitle.trim());
                return (
                  <div className="overlay-body preview-body">
                    {isSpawn && (
                      <>
                        <label className="field-label field-label--row">
                          Session name
                          {pv.suggesting && <span className="field-busy-note">Generating title…</span>}
                        </label>
                        <div className={`title-field${pv.suggesting ? " busy-ring busy-ring--ai" : ""}`}>
                          <input
                            className="text-input"
                            type="text"
                            value={pv.shortTitle}
                            autoFocus
                            placeholder="short session label"
                            disabled={pv.spawning}
                            onChange={(e) => setPreview({ shortTitle: e.target.value })}
                          />
                        </div>
                      </>
                    )}

                    <label className="field-label">Issue title</label>
                    <input
                      className="text-input"
                      value={pv.issueTitle}
                      autoFocus={!isSpawn}
                      disabled={pv.spawning}
                      onChange={(e) => setPreview({ issueTitle: e.target.value })}
                      spellCheck={false}
                    />

                    <label className="field-label">Issue description</label>
                    <textarea
                      className="text-input preview-desc"
                      value={pv.issueBody}
                      rows={5}
                      disabled={pv.spawning}
                      onChange={(e) => setPreview({ issueBody: e.target.value })}
                      spellCheck={false}
                    />

                    {isSpawn && (
                      <div className="preview-derived">
                        <div className="preview-derived-row">
                          <span className="preview-derived-label">Session</span>
                          <span className="preview-session">
                            <span className="preview-swatch" style={{ background: pv.color }} />
                            {pv.emoji} #{numPart} — {pv.shortTitle || "…"}
                          </span>
                        </div>
                        <div className="preview-derived-row">
                          <span className="preview-derived-label">Workspace</span>
                          <code>{workspace}</code>
                        </div>
                        <div className="preview-derived-row">
                          <span className="preview-derived-label">Branch</span>
                          <code>feature/{workspace}</code>
                        </div>
                        <div className="preview-derived-row">
                          <span className="preview-derived-label">Worktree</span>
                          <code>~/src/work-{workspace}/{pv.repoName}</code>
                        </div>
                      </div>
                    )}

                    {!isSpawn ? (
                      <p className="session-hint">A new issue will be created on GitHub.</p>
                    ) : pv.issueNumber == null ? (
                      <p className="session-hint">A new issue is created on spawn; its number is prefixed to the branch and worktree names.</p>
                    ) : issueDirty ? (
                      <p className="session-hint">Saving will update issue #{pv.issueNumber} on GitHub.</p>
                    ) : null}
                    {pv.error && <p className="cred-error">{pv.error}</p>}

                    <div className="issue-actions">
                      <button
                        className={`btn-save ${pv.spawning ? "btn-busy" : ""}`}
                        disabled={pv.spawning || !canConfirm}
                        onClick={confirmPreview}
                      >
                        {isSpawn ? (pv.issueNumber == null ? "Create & Spawn" : "Spawn") : "Create Issue"}
                      </button>
                      <button className="btn-ghost" disabled={pv.spawning} onClick={cancelPreview}>Cancel</button>
                    </div>
                  </div>
                );
              })() : (
                <>
                  <div className={`overlay-search-wrap ${ideaOpen ? "idea-open" : ""}`}>
                    <textarea
                      ref={ideaRef}
                      className="text-input idea-textarea"
                      placeholder="Write your own idea…"
                      rows={1}
                      value={picker.query}
                      onFocus={() => setExpanded({ kind: "idea" })}
                      onChange={(e) => setPicker((p) => (p ? { ...p, query: e.target.value } : p))}
                      onKeyDown={(e) => {
                        // ⌘/Ctrl+Enter submits the primary action; plain Enter is a newline.
                        if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
                          e.preventDefault();
                          if (picker.query.trim() && !busy) createAndSpawn();
                        }
                      }}
                      spellCheck={false}
                      autoCapitalize="off"
                      autoCorrect="off"
                    />
                    {ideaOpen && (
                      <div className="issue-actions">
                        <button
                          className={`btn-ghost ${picker.creating === "create" ? busyCls(picker.creatingRequestId) : ""}`}
                          disabled={!picker.query.trim() || busy}
                          onClick={createIssueOnly}
                        >
                          Create Issue
                        </button>
                        <button
                          className={`btn-save ${picker.creating === "spawn" ? busyCls(picker.creatingRequestId) : ""}`}
                          disabled={!picker.query.trim() || busy}
                          onClick={createAndSpawn}
                        >
                          Create Issue and Spawn
                        </button>
                        <button
                          className="btn-ghost"
                          disabled={busy}
                          onClick={() => {
                            setExpanded(null);
                            setPicker((p) => (p ? { ...p, query: "" } : p));
                          }}
                        >
                          Cancel
                        </button>
                      </div>
                    )}
                  </div>
                  <div className="overlay-body">
                    <div className="issue-list-toolbar">
                      <span className="issue-list-label">Open issues</span>
                      <button
                        className={`btn-add ${picker.refreshing ? "btn-busy" : ""}`}
                        onClick={refreshIssues}
                        disabled={picker.loading || picker.refreshing}
                        title="Refresh issues"
                      >
                        {picker.refreshing ? "Refreshing…" : "↻ Refresh"}
                      </button>
                    </div>
                    {picker.loading ? (
                      <p className="repo-group-empty">Loading issues…</p>
                    ) : picker.error ? (
                      <p className="cred-error">{picker.error}</p>
                    ) : ordered.length > 0 ? (
                      <div className="issue-tree">
                        {ordered.map((n) => (
                          <IssueRow
                            key={n.number}
                            node={n}
                            depth={0}
                            expandedNumber={expanded?.kind === "issue" ? expanded.number : null}
                            preparingNumber={picker.preparing ?? null}
                            active={active}
                            onExpand={(num) => setExpanded({ kind: "issue", number: num })}
                            onCollapse={() => setExpanded(null)}
                            onSpawn={spawnIssue}
                          />
                        ))}
                      </div>
                    ) : (
                      <p className="repo-group-empty">{picker.query ? "No matching issues." : "No open issues."}</p>
                    )}
                    {picker.confirm && (
                      <div className="confirm-block">
                        <p className="confirm-lead">Claude couldn’t turn this into a clear issue:</p>
                        <p className="confirm-msg">{picker.confirm.message}</p>
                        <div className="issue-actions">
                          <button className="btn-save" onClick={confirmRaw}>Create issue from my text</button>
                          <button
                            className="btn-ghost"
                            onClick={() => setPicker((p) => (p ? { ...p, confirm: undefined } : p))}
                          >
                            Cancel
                          </button>
                        </div>
                      </div>
                    )}
                    {picker.note && <p className="issue-note">{picker.note}</p>}
                  </div>
                </>
              )}
            </div>
          </div>
        );
      })()}

      {hideTarget && (
        <HideSnoozeDialog
          title={hideTarget.kind === "repo" ? hideTarget.repo : hideTarget.session.session_title}
          onConfirm={(hidden) => applyVisibility(hideTarget, hidden)}
          onClose={() => setHideTarget(null)}
        />
      )}
    </main>
  );
}

// ── Logs window ─────────────────────────────────────────────────────────────────

const LOG_POLL_MS = 2000;
const LOG_LEVELS = ["ERROR", "WARN", "INFO", "DEBUG", "TRACE"] as const;
type LogLevel = (typeof LOG_LEVELS)[number];

// The level sits right after the (whitespace-free) timestamp, e.g.
// "2026-06-02T22:15:03.1Z  WARN span{…}: …" — anchor there so the word "INFO"
// inside a message body can't mis-colour a line.
const LEVEL_RE = /^\S+\s+(ERROR|WARN|INFO|DEBUG|TRACE)\b/;

/** Pull the level token out of a tracing line so we can colour it. */
function lineLevel(line: string): LogLevel | null {
  const m = LEVEL_RE.exec(line);
  return m ? (m[1] as LogLevel) : null;
}

function LogsView() {
  const [text, setText] = useState("");
  const [filter, setFilter] = useState("");
  const [error, setError] = useState<string | null>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  // Only auto-scroll when the user is already pinned to the bottom, so reading
  // back through history isn't yanked away on each 2s refresh.
  const pinnedRef = useRef(true);

  // Hide (keep alive) on close, mirroring the Settings window.
  useEffect(() => {
    const win = getCurrentWindow();
    const unlisten = win.onCloseRequested((event) => {
      event.preventDefault();
      win.hide();
    });
    return () => { unlisten.then((f) => f()); };
  }, []);

  // Poll the tail every couple seconds while the window is shown.
  useEffect(() => {
    let active = true;
    const tick = async () => {
      try {
        const t = await api.logsRead();
        if (active) { setText(t); setError(null); }
      } catch (e) {
        if (active) setError(String(e));
      }
    };
    tick();
    const timer = setInterval(tick, LOG_POLL_MS);
    return () => { active = false; clearInterval(timer); };
  }, []);

  const lines = useMemo(() => {
    const all = text.length ? text.split("\n") : [];
    const f = filter.trim().toLowerCase();
    return f ? all.filter((l) => l.toLowerCase().includes(f)) : all;
  }, [text, filter]);

  // After each render that changed the visible lines, stick to the bottom if the
  // user hasn't scrolled up.
  useEffect(() => {
    const el = scrollRef.current;
    if (el && pinnedRef.current) el.scrollTop = el.scrollHeight;
  }, [lines]);

  function onScroll() {
    const el = scrollRef.current;
    if (!el) return;
    pinnedRef.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
  }

  return (
    <main className="panel panel--window logs-panel">
      <header className="panel-header">
        <h1>m<span className="ai">AI</span>estro</h1>
        <span className="panel-subtitle">Logs</span>
      </header>

      <div className="logs-toolbar">
        <input
          className="text-input logs-filter"
          type="text"
          placeholder="Filter… e.g. session=28-add-foo or ERROR"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          spellCheck={false}
          autoCapitalize="off"
          autoCorrect="off"
        />
        {filter && (
          <button className="btn-clear" onClick={() => setFilter("")} title="Clear filter" aria-label="Clear filter">✕</button>
        )}
        <button className="btn-add" onClick={() => api.logsReveal()} title="Reveal in Finder">Reveal</button>
      </div>

      <div className="logs-view" ref={scrollRef} onScroll={onScroll}>
        {error ? (
          <div className="logs-empty">Couldn’t read logs: {error}</div>
        ) : lines.length === 0 ? (
          <div className="logs-empty">{text.length ? "No lines match the filter." : "No log entries yet today."}</div>
        ) : (
          lines.map((line, i) => {
            const lvl = lineLevel(line);
            return (
              <div key={i} className={`logs-line${lvl ? ` logs-line--${lvl.toLowerCase()}` : ""}`}>
                {line || " "}
              </div>
            );
          })
        )}
      </div>
    </main>
  );
}

export default function App() {
  // Apply the persisted theme to this window and keep it in sync with the
  // backend's `theme-changed` broadcast. Runs in every window (popover,
  // settings, logs) since each is a separate webview rendering this bundle.
  useEffect(() => {
    const unlisten = initTheme();
    return () => { unlisten.then((f) => f()); };
  }, []);

  const label = getCurrentWindow().label;
  if (label === "settings") return <Settings />;
  if (label === "logs") return <LogsView />;
  return <MainView />;
}
