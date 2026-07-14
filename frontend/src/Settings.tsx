import { useState, useEffect, useCallback, useMemo } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { api, AppSettings, CredentialScope, CredentialTypeDto, GHRepo, HealthCheck, RepoSettings, ResolvedTool } from "./api";
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
import { applyTheme } from "./theme";
import ChevronRightIcon from "./icons/chevron-right.svg?react";
import { CredRows, CredState } from "./components/CredRows";
import { DetailErrorBoundary } from "./components/DetailErrorBoundary";
import { HealthModal, HealthState } from "./components/HealthModal";
import { DismissibleError } from "./components/DismissibleError";
import { RemoveConfirm } from "./components/RemoveConfirm";
import { useHideOnClose } from "./hooks/useTauriListen";
import { useDebouncedAutosave } from "./hooks/useDebouncedAutosave";

type SettingsSelection =
  | { kind: "identity"; id: string }
  | { kind: "repo"; repo: string }
  | { kind: "repo-add" }
  | { kind: "preferences" }
  | null;

// The "couldn't load ~/.maiestro/*.json" banner shown instead of a form full of
// defaults (which would clobber the file on the next save). `hint` names how to
// recover for the specific file.
function SettingsLoadError({ lead, message, hint }: { lead: string; message: string; hint: string }) {
  return (
    <div className="cleanup-confirm">
      <p className="cleanup-confirm-body">{lead}</p>
      <p className="cleanup-confirm-body" style={{ opacity: 0.85, fontFamily: "var(--font-mono, monospace)", fontSize: 11 }}>
        {message}
      </p>
      <p className="cleanup-confirm-body" style={{ opacity: 0.7 }}>{hint}</p>
    </div>
  );
}

export function Settings() {
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
  // Repo health-check modal (#93). Checks stream in one at a time via the
  // `health-check` event; `total` (known once the first event lands) lets the
  // modal stop the spinner. `loading` stays true until the command resolves.
  const [health, setHealth] = useState<HealthState | null>(null);
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

  // Debounced autosave for the two JsonForms panels. The repo form saves to the
  // repo carried in its own data (the hidden `repo` field round-trips); the app
  // form applies the theme immediately, refreshes tool resolution after a save,
  // and rolls back its baseline on failure so a fixed value can save again.
  const repoAutosave = useDebouncedAutosave<RepoSettings>({
    save: (d) => api.setRepoSettings(d.repo, d),
    onError: setRepoSaveError,
  });
  const appAutosave = useDebouncedAutosave<AppSettings>({
    save: (d) => api.setAppSettings(d),
    onError: setAppSaveError,
    rollbackOnError: true,
    onChange: (d) => applyTheme(d.theme ?? "system"),
    onSaved: () => { api.toolsResolved().then(setResolvedTools).catch(() => {}); },
  });

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
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  // Load the global settings + current tool resolution for the Preferences form.
  function loadAppSettings() {
    api.getAppSettings()
      .then((s) => {
        appAutosave.seed(s);
        setAppSettings(s);
        setAppLoadError(null);
      })
      .catch((e) => setAppLoadError(String(e)));
    api.toolsResolved().then(setResolvedTools).catch(() => setResolvedTools([]));
  }

  // Autosave the Preferences form, debounced, skipped while ajv reports errors.
  function handleAppFormChange(data: AppSettings, errors: unknown[] | undefined) {
    setAppSettings(data);
    if ((errors?.length ?? 0) > 0) return;
    appAutosave.schedule(data);
  }

  useHideOnClose();

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
    // `credTypes` must be a dep: on mount it races the default-identity fetch,
    // and if it lands after scopeKey settles the badges would stay "not set".
  }, [scopeKey, credTypes]); // eslint-disable-line react-hooks/exhaustive-deps

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
        repoAutosave.seed(s);
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
    repoAutosave.schedule(data);
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
    repoAutosave.cancel();
    try {
      await api.removeRepo(repo);
      setRepos((prev) => prev.filter((r) => r !== repo));
      setSelection(null);
    } catch (e) {
      setRepoRemoveError(String(e));
    }
  }

  // Run the repo health check and stream results into the modal (#93). Each
  // check arrives as a `health-check` event; the command's return value is the
  // authoritative final list (reconciles any missed event).
  async function runHealthCheck(repo: string) {
    setHealth({ repo, checks: [], total: null, running: null, loading: true, error: null });
    const win = getCurrentWindow();
    // A check announces itself (running) before it executes, then reports its
    // result — subscribe to both. Both are scoped to `repo`.
    const unlistenRunning = await win.listen<{ repo: string; total: number; label: string }>(
      "health-check-running",
      (e) => {
        if (e.payload.repo !== repo) return;
        setHealth((h) =>
          h && h.repo === repo ? { ...h, running: e.payload.label, total: e.payload.total } : h
        );
      }
    );
    const unlistenDone = await win.listen<{ repo: string; total: number; check: HealthCheck }>(
      "health-check",
      (e) => {
        if (e.payload.repo !== repo) return;
        setHealth((h) =>
          h && h.repo === repo
            ? { ...h, checks: [...h.checks, e.payload.check], total: e.payload.total }
            : h
        );
      }
    );
    try {
      const report = await api.repoHealthCheck(repo);
      setHealth((h) =>
        h && h.repo === repo
          ? { ...h, checks: report.checks, total: report.checks.length, running: null, loading: false }
          : h
      );
    } catch (e) {
      setHealth((h) =>
        h && h.repo === repo ? { ...h, running: null, loading: false, error: String(e) } : h
      );
    } finally {
      unlistenRunning();
      unlistenDone();
    }
  }

  async function handleSelectRepo(repo: string) {
    const iid = browse.identityId;
    setRepos((prev) => (prev.includes(repo) ? prev : [...prev, repo]));
    // Persist the identity BEFORE selecting: selection triggers the detail-form
    // load, and a load that races an in-flight write reads the pre-write file —
    // the form then shows no identity and the next autosave clobbers it.
    try {
      const defaults = await api.getRepoSettings(repo);
      await api.setRepoSettings(repo, { ...defaults, identity_id: iid ?? defaults.identity_id });
    } catch (e) {
      setBrowse((b) => ({ ...b, error: `Added ${repo}, but couldn't assign the identity: ${String(e)}` }));
    }
    setSelection({ kind: "repo", repo });
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
                        aria-label="New identity name"
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
                <SettingsLoadError
                  lead="Couldn't load settings:"
                  message={appLoadError}
                  hint="Fix ~/.maiestro/settings.json by hand, then reopen Preferences."
                />
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
                <CredRows
                  credTypes={credTypes}
                  credStates={credStates}
                  patchCred={patchCred}
                  onSave={handleSave}
                  onClear={handleClear}
                />
                <div className="settings-group">
                  {identityRemoveError && (
                    <DismissibleError
                      lead="Couldn't remove identity"
                      message={identityRemoveError}
                      onDismiss={() => setIdentityRemoveError(null)}
                    />
                  )}
                  {identityRemoveConfirm ? (
                    <RemoveConfirm
                      name={selection.id}
                      body="Its Keychain credentials are deleted; repos that used it need a new identity."
                      onRemove={() => handleRemoveIdentity(selection.id)}
                      onCancel={() => setIdentityRemoveConfirm(false)}
                    />
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
                  disabled={!!health?.loading}
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
                  <SettingsLoadError
                    lead="Couldn't load settings for this repo:"
                    message={repoLoadError}
                    hint="Fix the file by hand, then reselect this repo."
                  />
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
                    <DismissibleError
                      lead="Couldn't remove repo"
                      message={repoRemoveError}
                      onDismiss={() => setRepoRemoveError(null)}
                    />
                  )}
                  {repoRemoveConfirm ? (
                    <RemoveConfirm
                      name={selection.repo}
                      body="Worktrees, cloned repos, and any work in them are kept on disk."
                      onRemove={() => handleRemoveRepo(selection.repo)}
                      onCancel={() => setRepoRemoveConfirm(false)}
                    />
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
                    aria-label="Filter repos"
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
                        aria-label="Identity"
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
