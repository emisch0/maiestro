import { useState, useEffect, useCallback, useMemo, useRef } from "react";
import { getCurrentWindow, getAllWindows } from "@tauri-apps/api/window";
import { api, CredentialScope, CredentialTypeDto, DraftPreviewOutcome, GHRepo, HideState, IssueNode, PrLink, RepoSettings, Session, SpawnEdits, SpawnPlan } from "./api";
import GearIcon from "./icons/gear.svg?react";
import EyeIcon from "./icons/eye.svg?react";
import LogsIcon from "./icons/logs.svg?react";
import GitHubIcon from "./icons/github.svg?react";
import FolderIcon from "./icons/folder.svg?react";
import VSCodeIcon from "./icons/vscode.svg?react";
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

// A pending Tear Down prompt: a warnings confirmation, or a "VS Code still open"
// block (which may offer the Accessibility shortcut so mAIestro can close it).
type TeardownPrompt =
  | { id: string; kind: "confirm"; warnings: string[] }
  | { id: string; kind: "blocked"; message: string; accessibility: boolean };

type Tab = "identity" | "repo";
type SaveStatus = "idle" | "saving" | "saved" | "clearing" | "error";
type RepoView =
  | { mode: "list" }
  | { mode: "browsing"; identityId: string | null; identityInput: string; repos: GHRepo[] | null; filter: string; loading: boolean; error?: string }
  | { mode: "detail"; repo: string };

interface CredState {
  isSet: boolean;
  input: string;
  status: SaveStatus;
  error?: string;
}

function Settings() {
  const [tab, setTab] = useState<Tab>("identity");
  const [identityId, setIdentityId] = useState("");
  const [knownIdentities, setKnownIdentities] = useState<string[]>([]);
  const [addingIdentity, setAddingIdentity] = useState(false);
  const [identityInput, setIdentityInput] = useState("");
  const [repos, setRepos] = useState<string[]>([]);
  const [repoView, setRepoView] = useState<RepoView>({ mode: "list" });
  const [credTypes, setCredTypes] = useState<CredentialTypeDto[]>([]);
  const [credStates, setCredStates] = useState<Record<string, CredState>>({});
  const [repoSettings, setRepoSettings] = useState<RepoSettings>({ checkout_dir: null, worktree_prefix: null, env_files: [], identity_id: null, hidden: null });
  const [checkoutDraft, setCheckoutDraft] = useState("");
  const [worktreePrefixDraft, setWorktreePrefixDraft] = useState("");
  const [addingEnvFile, setAddingEnvFile] = useState(false);
  const [envFileInput, setEnvFileInput] = useState("");
  const [scanStatus, setScanStatus] = useState<"idle" | "scanning" | "done">("idle");

  useEffect(() => {
    api.listCredentialTypes().then((types) => {
      setCredTypes(types);
      const init: Record<string, CredState> = {};
      for (const t of types) init[t.type_id] = { isSet: false, input: "", status: "idle" };
      setCredStates(init);
    });
    api.listRepos().then(setRepos);
    api.identitiesList().then(setKnownIdentities);
    api.getDefaultIdentity().then((id) => { if (id) setIdentityId(id); });
  }, []);

  // Closing the Settings window hides it (keeping it alive for reuse) rather
  // than destroying it, so the gear icon can reopen the same window.
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
    if (tab === "identity" && identityId.trim())
      return { kind: "identity", identity_id: identityId.trim() };
    return null;
  }, [tab, identityId]);

  const scopeKey = activeScope ? `identity:${activeScope.identity_id}` : null;

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
            await api.getCredential(t.type_id, scope);
            patchCred(t.type_id, { isSet: true });
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

  useEffect(() => {
    if (repoView.mode !== "detail") return;
    const repo = repoView.repo;
    api.getRepoSettings(repo).then((s) => {
      setRepoSettings(s);
      setCheckoutDraft(s.checkout_dir ?? "");
      setWorktreePrefixDraft(s.worktree_prefix ?? "");
    });
    setScanStatus("idle");
    setAddingEnvFile(false);
    setEnvFileInput("");
  }, [repoView.mode === "detail" ? repoView.repo : null]); // eslint-disable-line react-hooks/exhaustive-deps

  async function saveRepoSettings(next: RepoSettings) {
    if (repoView.mode !== "detail") return;
    await api.setRepoSettings(repoView.repo, next);
    setRepoSettings(next);
  }

  async function handleCheckoutDirCommit() {
    const dir = checkoutDraft.trim() || null;
    await saveRepoSettings({ ...repoSettings, checkout_dir: dir });
  }

  async function handleWorktreePrefixCommit() {
    const prefix = worktreePrefixDraft.trim() || null;
    await saveRepoSettings({ ...repoSettings, worktree_prefix: prefix });
  }

  async function handleScanEnvFiles() {
    if (!repoSettings.checkout_dir) return;
    setScanStatus("scanning");
    try {
      const files = await api.scanEnvFiles(repoSettings.checkout_dir);
      await saveRepoSettings({ ...repoSettings, env_files: files });
      setScanStatus("done");
      setTimeout(() => setScanStatus("idle"), 2000);
    } catch {
      setScanStatus("idle");
    }
  }

  async function handleAddEnvFile() {
    const path = envFileInput.trim();
    if (!path) return;
    const files = repoSettings.env_files.includes(path)
      ? repoSettings.env_files
      : [...repoSettings.env_files, path];
    await saveRepoSettings({ ...repoSettings, env_files: files });
    setEnvFileInput("");
    setAddingEnvFile(false);
  }

  async function handleRemoveEnvFile(path: string) {
    await saveRepoSettings({
      ...repoSettings,
      env_files: repoSettings.env_files.filter((f) => f !== path),
    });
  }

  // Adopt an identity as the active one and remember it as the default so it is
  // restored on the next launch. Used by both the picker and the add-new flow.
  async function commitIdentity(id: string) {
    const trimmed = id.trim();
    if (!trimmed) return;
    setIdentityId(trimmed);
    setAddingIdentity(false);
    setIdentityInput("");
    try {
      await api.setDefaultIdentity(trimmed);
      api.identitiesList().then(setKnownIdentities);
    } catch { /* best-effort: selection still works for this session */ }
  }

  async function handleClear(type_id: string) {
    if (!activeScope) return;
    patchCred(type_id, { status: "clearing" });
    try {
      await api.deleteCredential(type_id, activeScope);
    } catch { /* already gone */ }
    patchCred(type_id, { status: "idle", isSet: false, input: "" });
  }

  async function handleSelectIdentity(iid: string) {
    if (repoView.mode !== "browsing") return;
    setRepoView({ ...repoView, identityId: iid, loading: true, error: undefined });
    try {
      const fetched = await api.githubListRepos(iid);
      setRepoView({ ...repoView, identityId: iid, loading: false, repos: fetched, filter: "" });
      api.identitiesList().then(setKnownIdentities);
    } catch (e) {
      setRepoView({ ...repoView, identityId: iid, loading: false, error: String(e) });
    }
  }

  async function handleSelectRepo(repo: string) {
    const identityId = repoView.mode === "browsing" ? repoView.identityId : null;
    setRepos((prev) => prev.includes(repo) ? prev : [...prev, repo]);
    setRepoView({ mode: "detail", repo });
    const defaults = await api.getRepoSettings(repo);
    await api.setRepoSettings(repo, { ...defaults, identity_id: identityId ?? defaults.identity_id });
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
                  className="btn-save"
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
      <header className="panel-header">
        <h1>m<span className="ai">AI</span>estro</h1>
        <span className="panel-subtitle">Settings</span>
      </header>

      <div className="tabs">
        <button
          className={`tab ${tab === "identity" ? "active" : ""}`}
          onClick={() => setTab("identity")}
        >
          Identity
        </button>
        <button
          className={`tab ${tab === "repo" ? "active" : ""}`}
          onClick={() => { setTab("repo"); setRepoView({ mode: "list" }); }}
        >
          Repo
        </button>
      </div>

      {tab === "identity" ? (
        <>
          <div className="context-section">
            <label className="field-label">Identity</label>
            {knownIdentities.length === 0 || addingIdentity ? (
              <div className="cred-controls">
                <input
                  className="text-input"
                  type="text"
                  placeholder="e.g. default"
                  value={identityInput}
                  autoFocus={addingIdentity}
                  onChange={(e) => setIdentityInput(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") commitIdentity(identityInput);
                    if (e.key === "Escape") { setAddingIdentity(false); setIdentityInput(""); }
                  }}
                  spellCheck={false}
                  autoCapitalize="off"
                  autoCorrect="off"
                />
                <button className="btn-save" disabled={!identityInput.trim()} onClick={() => commitIdentity(identityInput)}>Add</button>
                {knownIdentities.length > 0 && (
                  <button className="btn-clear" onClick={() => { setAddingIdentity(false); setIdentityInput(""); }}>✕</button>
                )}
              </div>
            ) : (
              <div className="cred-controls">
                <select
                  className="text-input profile-select"
                  value={identityId}
                  onChange={(e) => commitIdentity(e.target.value)}
                >
                  {!identityId && <option value="">Select an identity…</option>}
                  {identityId && !knownIdentities.includes(identityId) && (
                    <option value={identityId}>{identityId}</option>
                  )}
                  {knownIdentities.map((iid) => (
                    <option key={iid} value={iid}>{iid}</option>
                  ))}
                </select>
                <button className="btn-add" onClick={() => { setAddingIdentity(true); setIdentityInput(""); }}>+ New</button>
              </div>
            )}
          </div>
          <div className="cred-list">
            {identityId.trim() ? <CredRows /> : (
              <div className="empty-state">
                <p className="empty-state-body">Select or add an identity to manage its credentials.</p>
              </div>
            )}
          </div>
        </>
      ) : repoView.mode === "detail" ? (
        <>
          <div className="detail-header">
            <button className="btn-back" onClick={() => setRepoView({ mode: "list" })}>‹</button>
            <span className="detail-title">{repoView.repo}</span>
          </div>
          <div className="cred-list">

            {/* ── Identity ── */}
            <div className="settings-group">
              <div className="settings-group-header">
                <span className="field-label" style={{ marginBottom: 0 }}>Identity</span>
              </div>
              {knownIdentities.length === 0 ? (
                <p className="session-hint" style={{ paddingTop: 2 }}>
                  No identities configured. Go to the Identity tab first.
                </p>
              ) : (
                <select
                  className="text-input profile-select"
                  value={repoSettings.identity_id ?? ""}
                  onChange={(e) => saveRepoSettings({ ...repoSettings, identity_id: e.target.value || null })}
                >
                  <option value="">— none —</option>
                  {knownIdentities.map((iid) => (
                    <option key={iid} value={iid}>{iid}</option>
                  ))}
                </select>
              )}
            </div>

            {/* ── Checkout directory ── */}
            <div className="settings-group">
              <div className="settings-group-header">
                <span className="field-label" style={{ marginBottom: 0 }}>Checkout directory</span>
              </div>
              <div className="cred-controls">
                <input
                  className="text-input"
                  type="text"
                  placeholder="~/src/repo-name"
                  value={checkoutDraft}
                  onChange={(e) => setCheckoutDraft(e.target.value)}
                  onBlur={handleCheckoutDirCommit}
                  onKeyDown={(e) => e.key === "Enter" && handleCheckoutDirCommit()}
                  spellCheck={false}
                  autoCapitalize="off"
                  autoCorrect="off"
                />
              </div>
            </div>

            {/* ── Worktree prefix ── */}
            <div className="settings-group">
              <div className="settings-group-header">
                <span className="field-label" style={{ marginBottom: 0 }}>Worktree prefix</span>
              </div>
              <div className="cred-controls">
                <input
                  className="text-input"
                  type="text"
                  placeholder="~/src/work-"
                  value={worktreePrefixDraft}
                  onChange={(e) => setWorktreePrefixDraft(e.target.value)}
                  onBlur={handleWorktreePrefixCommit}
                  onKeyDown={(e) => e.key === "Enter" && handleWorktreePrefixCommit()}
                  spellCheck={false}
                  autoCapitalize="off"
                  autoCorrect="off"
                />
              </div>
            </div>

            {/* ── Env files ── */}
            <div className="settings-group">
              <div className="settings-group-header">
                <span className="field-label" style={{ marginBottom: 0 }}>Environment files</span>
                <div style={{ display: "flex", gap: 4 }}>
                  {repoSettings.checkout_dir && (
                    <button
                      className="btn-add"
                      disabled={scanStatus === "scanning"}
                      onClick={handleScanEnvFiles}
                    >
                      {scanStatus === "scanning" ? "…" : scanStatus === "done" ? "✓ Scanned" : "Scan"}
                    </button>
                  )}
                  <button className="btn-add" onClick={() => setAddingEnvFile(true)}>+ Add</button>
                </div>
              </div>

              {addingEnvFile && (
                <div className="cred-controls">
                  <input
                    className="text-input"
                    type="text"
                    placeholder=".env or subdir/.env"
                    value={envFileInput}
                    autoFocus
                    onChange={(e) => setEnvFileInput(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") handleAddEnvFile();
                      if (e.key === "Escape") { setAddingEnvFile(false); setEnvFileInput(""); }
                    }}
                    spellCheck={false}
                    autoCapitalize="off"
                    autoCorrect="off"
                  />
                  <button className="btn-save" disabled={!envFileInput.trim()} onClick={handleAddEnvFile}>Add</button>
                  <button className="btn-clear" onClick={() => { setAddingEnvFile(false); setEnvFileInput(""); }}>✕</button>
                </div>
              )}

              {repoSettings.env_files.length === 0 && !addingEnvFile ? (
                <p className="session-hint" style={{ paddingTop: 2 }}>
                  No env files. Use Scan to find .env files in the checkout directory.
                </p>
              ) : (
                <div className="env-file-list">
                  {repoSettings.env_files.map((f) => (
                    <div key={f} className="env-file-row">
                      <span className="env-file-path">{f}</span>
                      <button
                        className="btn-clear"
                        onClick={() => handleRemoveEnvFile(f)}
                        title="Remove"
                      >✕</button>
                    </div>
                  ))}
                </div>
              )}
            </div>
          </div>
        </>
      ) : (
        <>
          <div className="list-header">
            <span className="field-label">
              {repoView.mode === "browsing" && repoView.identityId
                ? repoView.identityId
                : repoView.mode === "browsing"
                ? "Select identity"
                : "Repositories"}
            </span>
            {repoView.mode === "list" ? (
              <button
                className="btn-add"
                onClick={() => {
                  api.identitiesList().then(setKnownIdentities);
                  setRepoView({ mode: "browsing", identityId: null, identityInput: "", repos: null, filter: "", loading: false });
                }}
              >
                + Add
              </button>
            ) : repoView.mode === "browsing" && repoView.identityId !== null ? (
              <button
                className="btn-clear"
                onClick={() => setRepoView({ ...repoView, identityId: null, identityInput: "", repos: null, filter: "", loading: false, error: undefined })}
              >
                ‹ Back
              </button>
            ) : (
              <button className="btn-clear" onClick={() => setRepoView({ mode: "list" })}>Cancel</button>
            )}
          </div>

          {repoView.mode === "browsing" && repoView.repos !== null && (
            <div className="adding-row" style={{ paddingTop: 0 }}>
              <input
                className="text-input"
                type="text"
                placeholder="Filter repos…"
                value={repoView.filter}
                autoFocus
                onChange={(e) => setRepoView({ ...repoView, filter: e.target.value })}
                spellCheck={false}
                autoCapitalize="off"
                autoCorrect="off"
              />
            </div>
          )}

          <div className="cred-list">
            {repoView.mode === "list" && (
              repos.length === 0 ? (
                <div className="empty-state">
                  <p className="empty-state-title">No repos configured</p>
                  <p className="empty-state-body">
                    Add a repo to assign it an identity and configure its workspace.
                  </p>
                </div>
              ) : (
                repos.map((repo) => (
                  <button
                    key={repo}
                    className="repo-item"
                    onClick={() => setRepoView({ mode: "detail", repo })}
                  >
                    <span className="repo-item-name">{repo}</span>
                    <span className="repo-item-chevron">›</span>
                  </button>
                ))
              )
            )}

            {repoView.mode === "browsing" && repoView.identityId === null && (
              knownIdentities.length === 0 ? (
                <div className="empty-state">
                  <p className="empty-state-title">No identities yet</p>
                  <p className="empty-state-body">
                    Go to the Identity tab, enter an identity ID, and save a GitHub token. Then come back here.
                  </p>
                </div>
              ) : (
                <div className="adding-row">
                  <select
                    className="text-input profile-select"
                    value={repoView.identityInput}
                    autoFocus
                    onChange={(e) => setRepoView({ ...repoView, identityInput: e.target.value })}
                  >
                    <option value="">Select an identity…</option>
                    {knownIdentities.map((iid) => (
                      <option key={iid} value={iid}>{iid}</option>
                    ))}
                  </select>
                  <button
                    className="btn-save"
                    disabled={!repoView.identityInput.trim()}
                    onClick={() => handleSelectIdentity(repoView.identityInput.trim())}
                  >
                    Fetch
                  </button>
                </div>
              )
            )}

            {repoView.mode === "browsing" && repoView.identityId !== null && repoView.loading && (
              <div className="empty-state">
                <p className="empty-state-body">Fetching repos…</p>
              </div>
            )}

            {repoView.mode === "browsing" && repoView.error && (
              <p className="cred-error" style={{ paddingTop: 4 }}>{repoView.error}</p>
            )}

            {repoView.mode === "browsing" && repoView.repos !== null && (() => {
              const filtered = repoView.repos.filter((r) =>
                !repoView.filter || r.full_name.toLowerCase().includes(repoView.filter.toLowerCase())
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
    </main>
  );
}

async function openSettings() {
  const settings = (await getAllWindows()).find((w) => w.label === "settings");
  if (settings) {
    await settings.show();
    await settings.setFocus();
  }
}

async function openLogs() {
  const logs = (await getAllWindows()).find((w) => w.label === "logs");
  if (logs) {
    await logs.show();
    await logs.setFocus();
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
  // True while the issue list is being re-fetched via the refresh button.
  refreshing?: boolean;
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
type ActiveSessions = Record<number, { color: string; title: string }>;

interface IssueRowProps {
  node: IssueNode;
  depth: number;
  expandedNumber: number | null;
  active: ActiveSessions;
  onExpand: (n: number) => void;
  onCollapse: () => void;
  onSpawn: (n: IssueNode) => void;
}

function IssueRow({ node, depth, expandedNumber, active, onExpand, onCollapse, onSpawn }: IssueRowProps) {
  const indent = 10 + depth * 16;
  const isExpanded = expandedNumber === node.number;
  const session = active[node.number];
  // A "working" pill tinted with the workspace color, marking issues that
  // already have a session. Such issues can't be spawned again, so they don't
  // expand into the Spawn Work action; the hover explains why.
  const workingHint = "There is already a work session for this issue";
  const workingPill = session && (
    <span className="issue-working-pill" style={{ background: session.color }} title={workingHint}>working</span>
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
            <button className="btn-save" onClick={() => onSpawn(node)}>Spawn Work</button>
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
          active={active}
          onExpand={onExpand}
          onCollapse={onCollapse}
          onSpawn={onSpawn}
        />
      ))}
    </>
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
  // Session id whose inline command strip is expanded (only one at a time).
  const [commandsOpen, setCommandsOpen] = useState<string | null>(null);
  // Pending Tear Down prompt for a session: either a warnings confirmation, or a
  // "VS Code still open" block that can offer the Accessibility shortcut.
  const [teardownConfirm, setTeardownConfirm] = useState<TeardownPrompt | null>(null);
  // Create-PR progress/error per session id: `{ creating }` while in flight,
  // `{ error }` after a failure. Absent = idle.
  const [prCreate, setPrCreate] = useState<Record<string, { creating?: boolean; error?: string }>>({});
  // Global toggle: reveal hidden/snoozed repos and work items (dimmed).
  const [showHidden, setShowHidden] = useState(false);
  // Per-repo settings, keyed by repo full_name — the source of repo-level hide state.
  const [repoSettings, setRepoSettings] = useState<Record<string, RepoSettings>>({});
  // Open repo options menu (repo full_name); at most one at a time.
  const [repoMenuOpen, setRepoMenuOpen] = useState<string | null>(null);
  // Target of the hide/snooze dialog, or null when closed.
  const [hideTarget, setHideTarget] = useState<HideTarget | null>(null);

  const refreshSessions = useCallback(() => {
    api.sessionsList().then((list) => {
      setSessions(list);
      // Fire PR lookups in parallel; each settles its own entry and soft-fails,
      // so rows render immediately and a PR button appears as its lookup lands.
      for (const s of list) {
        api.sessionPr(s.id)
          .then((pr) => setPrs((prev) => ({ ...prev, [s.id]: pr })))
          .catch(() => setPrs((prev) => ({ ...prev, [s.id]: null })));
      }
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
  }, [refreshSessions]);

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
    setPicker((p) => (p ? { ...p, note: `Preparing #${node.number}…` } : p));
    try {
      const plan = await api.prepareSpawn(repo, node.number);
      setPicker((p) => (p ? { ...p, note: undefined, preview: planToPreview(plan, "spawn") } : p));
    } catch (e) {
      setPicker((p) => (p ? { ...p, note: `Failed to prepare #${node.number}: ${String(e)}` } : p));
    }
  }

  function applyDraftPreview(res: DraftPreviewOutcome, idea: string, mode: "spawn" | "create") {
    if (res.status === "needs_confirmation") {
      setPicker((p) => (p ? { ...p, creating: undefined, note: undefined, confirm: { idea, message: res.message, action: mode } } : p));
    } else {
      setPicker((p) => (p ? { ...p, creating: undefined, note: undefined, confirm: undefined, preview: planToPreview(res, mode) } : p));
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
      await api.confirmSpawn(repo, edits);
      refreshSessions();
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
    setPicker((p) => (p ? { ...p, creating: "create", note: `Drafting an issue for “${idea}”…`, confirm: undefined } : p));
    try {
      applyDraftPreview(await api.draftSpawnPreview(repo, idea), idea, "create");
    } catch (e) {
      setPicker((p) => (p ? { ...p, creating: undefined, note: `Failed to draft issue: ${String(e)}` } : p));
    }
  }

  // Idea → AI draft → spawn preview (issue isn't opened until confirm).
  async function createAndSpawn() {
    if (!picker) return;
    const repo = picker.repo;
    const idea = picker.query.trim();
    if (!idea) return;
    setPicker((p) => (p ? { ...p, creating: "spawn", note: `Drafting an issue for “${idea}”…`, confirm: undefined } : p));
    try {
      applyDraftPreview(await api.draftSpawnPreview(repo, idea), idea, "spawn");
    } catch (e) {
      setPicker((p) => (p ? { ...p, creating: undefined, note: `Failed to draft issue: ${String(e)}` } : p));
    }
  }

  // User chose to proceed from their raw text despite Claude's prompt. Repeats
  // whichever action raised it, landing on its preview built from the raw text.
  async function confirmRaw() {
    if (!picker?.confirm) return;
    const repo = picker.repo;
    const { idea, action } = picker.confirm;
    setPicker((p) => (p ? { ...p, creating: action, note: "Drafting from your text…", confirm: undefined } : p));
    try {
      applyDraftPreview(await api.draftSpawnPreview(repo, idea, true), idea, action);
    } catch (e) {
      setPicker((p) => (p ? { ...p, creating: undefined, note: `Failed: ${String(e)}` } : p));
    }
  }

  // "Create PR": push the branch, draft a description with Claude, and open a
  // draft PR. On success the PR pill refreshes to link the new PR (we don't
  // open it in the browser — the pill is the entry point).
  async function createPr(s: Session) {
    setPrCreate((prev) => ({ ...prev, [s.id]: { creating: true } }));
    try {
      const pr = await api.createPr(s.id);
      setPrs((prev) => ({ ...prev, [s.id]: pr }));
      setPrCreate((prev) => ({ ...prev, [s.id]: {} }));
    } catch (e) {
      setPrCreate((prev) => ({ ...prev, [s.id]: { error: String(e) } }));
    }
  }

  // Run teardown and route its outcome to the right prompt: a warnings
  // confirmation, a "VS Code still open" block, or success (dismiss + refresh).
  // `confirmed` skips the work-state checks; `force` skips closing the editor.
  async function runTeardown(id: string, confirmed: boolean, force: boolean) {
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
    }
  }

  // "Tear Down": remove the worktree. Confirms first unless the backend says
  // it's safe (PR merged, nothing new).
  function tearDown(s: Session) {
    setTeardownConfirm(null);
    runTeardown(s.id, false, false);
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
    refreshAll();
  }

  const now = Date.now();

  return (
    <main className="panel">
      <header className="panel-header">
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
        <button className="icon-btn" onClick={openLogs} title="Logs" aria-label="Logs">
          <LogsIcon />
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
                    <span className="snooze-label">
                      {repoSnoozeLabel ? `Snoozed · ${repoSnoozeLabel}` : "Hidden"}
                    </span>
                  )}
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
                </div>

                {visibleSessions.length === 0 ? (
                  <p className="repo-group-empty">No active work</p>
                ) : (
                  visibleSessions.map((s) => {
                    const cmdOpen = commandsOpen === s.id;
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
                    return (
                    <div key={s.id} className={`workspace-item ${sessHidden || repoHidden ? "workspace-item--hidden" : ""}`}>
                      <div className="workspace-row" style={{ borderLeft: `3px solid ${s.color}` }}>
                        <span className="workspace-title">{s.session_title}</span>
                        {sessHidden && sessSnoozeLabel && (
                          <span className="snooze-label">Snoozed · {sessSnoozeLabel}</span>
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
                            title={`Reveal in Finder: ${s.work_dir}`}
                            aria-label="Open folder in Finder"
                          >
                            <FolderIcon />
                          </button>
                          <button
                            className="pill-btn"
                            onClick={() => api.openInEditor(s.work_dir)}
                            title="Open in VS Code"
                            aria-label="Open in VS Code"
                          >
                            <VSCodeIcon style={{ color: s.color }} />
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
                          className="command-btn"
                          onClick={() => createPr(s)}
                          disabled={prc?.creating || prOpen}
                          title={prOpen ? `PR #${pr?.number} is already open` : undefined}
                        >
                          {prc?.creating ? "Creating PR…" : "Create PR"}
                        </button>
                        {/* Merge PR is UI-only for now. */}
                        <button className="command-btn" onClick={() => {}}>Merge PR</button>
                        {sessHidden ? (
                          <button className="command-btn" onClick={() => applyVisibility({ kind: "session", session: s }, null)}>Unhide</button>
                        ) : (
                          <button className="command-btn" onClick={() => { setCommandsOpen(null); setHideTarget({ kind: "session", session: s }); }}>Hide…</button>
                        )}
                        <button className="command-btn" onClick={() => tearDown(s)}>Tear Down</button>
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
                      {teardownConfirm?.id === s.id && teardownConfirm.kind === "confirm" && (
                        <div className="cleanup-confirm">
                          <p className="cleanup-lead">Remove this workspace?</p>
                          <ul className="cleanup-warnings">
                            {teardownConfirm.warnings.map((w, i) => (
                              <li key={i}>{w}</li>
                            ))}
                          </ul>
                          <div className="issue-actions">
                            <button className="btn-danger" onClick={() => runTeardown(s.id, true, false)}>Remove anyway</button>
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
                            <button className="btn-danger" onClick={() => runTeardown(s.id, true, true)}>Delete anyway</button>
                            <button className="btn-ghost" onClick={() => setTeardownConfirm(null)}>Cancel</button>
                          </div>
                        </div>
                      )}
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
          active[s.issue_number] = active[s.issue_number]
            ? { color: active[s.issue_number].color, title: `${active[s.issue_number].title}, ${s.session_title}` }
            : { color: s.color, title: s.session_title };
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
                        <label className="field-label">Session name</label>
                        <input
                          className="text-input"
                          type="text"
                          value={pv.shortTitle}
                          autoFocus
                          placeholder="short session label"
                          disabled={pv.spawning}
                          onChange={(e) => setPreview({ shortTitle: e.target.value })}
                        />
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
                        className={`btn-save ${pv.spawning ? "btn-loading" : ""}`}
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
                          className={`btn-ghost ${picker.creating === "create" ? "btn-loading" : ""}`}
                          disabled={!picker.query.trim() || busy}
                          onClick={createIssueOnly}
                        >
                          Create Issue
                        </button>
                        <button
                          className={`btn-save ${picker.creating === "spawn" ? "btn-loading" : ""}`}
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
                        className="btn-add"
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
  const label = getCurrentWindow().label;
  if (label === "settings") return <Settings />;
  if (label === "logs") return <LogsView />;
  return <MainView />;
}
