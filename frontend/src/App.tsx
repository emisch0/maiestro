import { useState, useEffect, useCallback, useMemo } from "react";
import { getCurrentWindow, getAllWindows } from "@tauri-apps/api/window";
import { api, CreateAndSpawnOutcome, CredentialScope, CredentialTypeDto, GHRepo, IssueNode, PrLink, RepoSettings, Session } from "./api";
import GearIcon from "./icons/gear.svg?react";
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
  const [repoSettings, setRepoSettings] = useState<RepoSettings>({ checkout_dir: null, env_files: [], identity_id: null });
  const [checkoutDraft, setCheckoutDraft] = useState("");
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

type Picker = {
  repo: string;
  loading: boolean;
  issues: IssueNode[] | null;
  error?: string;
  query: string;
  note?: string;
  // Set when Claude couldn't draft a clear issue: holds the original idea and
  // Claude's reply, prompting the user to confirm creating from raw text.
  confirm?: { idea: string; message: string };
};

type Expand = { kind: "idea" } | { kind: "issue"; number: number } | null;

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

interface IssueRowProps {
  node: IssueNode;
  depth: number;
  expandedNumber: number | null;
  onExpand: (n: number) => void;
  onCollapse: () => void;
  onSpawn: (n: IssueNode) => void;
}

function IssueRow({ node, depth, expandedNumber, onExpand, onCollapse, onSpawn }: IssueRowProps) {
  const indent = 10 + depth * 16;
  const isExpanded = expandedNumber === node.number;
  return (
    <>
      {isExpanded ? (
        <div className="issue-row issue-row--expanded" style={{ paddingLeft: indent }}>
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
          </div>
          <div className="issue-actions">
            <button className="btn-save" onClick={() => onSpawn(node)}>Spawn Work</button>
            <button className="btn-ghost" onClick={onCollapse}>Cancel</button>
          </div>
        </div>
      ) : (
        <div className="issue-row" style={{ paddingLeft: indent }}>
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
          <button
            className="issue-title issue-row-expand"
            onClick={() => onExpand(node.number)}
            title={node.title}
          >
            {node.title}
          </button>
        </div>
      )}
      {node.children.map((c) => (
        <IssueRow
          key={c.number}
          node={c}
          depth={depth + 1}
          expandedNumber={expandedNumber}
          onExpand={onExpand}
          onCollapse={onCollapse}
          onSpawn={onSpawn}
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

  const [sessions, setSessions] = useState<Session[]>([]);
  // PR link per session id, discovered live from GitHub. `null` = looked up, none
  // found (or the lookup failed); absent key = not looked up yet.
  const [prs, setPrs] = useState<Record<string, PrLink | null>>({});
  // Session id whose inline command strip is expanded (only one at a time).
  const [commandsOpen, setCommandsOpen] = useState<string | null>(null);
  // Pending Clean Up confirmation: the session id and the warnings to show.
  const [cleanupConfirm, setCleanupConfirm] = useState<{ id: string; warnings: string[] } | null>(null);

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

  useEffect(() => {
    api.listRepos().then(setRepos);
    refreshSessions();
  }, [refreshSessions]);

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

  async function spawnIssue(node: IssueNode) {
    if (!picker) return;
    const repo = picker.repo;
    setPicker((p) => (p ? { ...p, note: `Spawning #${node.number}…` } : p));
    try {
      const res = await api.spawnWork(repo, node.number);
      const summary = res.reused
        ? `Reused workspace for #${node.number} → ${res.work_dir}`
        : `Spawned #${node.number} on ${res.branch} → ${res.work_dir}`;
      const note = res.warnings.length ? `${summary}\n⚠ ${res.warnings.join("; ")}` : summary;
      setPicker((p) => (p ? { ...p, note } : p));
      refreshSessions();
    } catch (e) {
      setPicker((p) => (p ? { ...p, note: `Failed to spawn #${node.number}: ${String(e)}` } : p));
    }
  }

  function applyOutcome(res: CreateAndSpawnOutcome, idea: string) {
    if (res.status === "needs_confirmation") {
      setPicker((p) => (p ? { ...p, note: undefined, confirm: { idea, message: res.message } } : p));
    } else {
      const summary = `Spawned ${res.issue_url} on ${res.branch} → ${res.work_dir}`;
      const note = res.warnings.length ? `${summary}\n⚠ ${res.warnings.join("; ")}` : summary;
      setPicker((p) => (p ? { ...p, note, confirm: undefined } : p));
      refreshSessions();
    }
  }

  async function createAndSpawn() {
    if (!picker) return;
    const repo = picker.repo;
    const idea = picker.query.trim();
    if (!idea) return;
    setPicker((p) => (p ? { ...p, note: `Drafting an issue for “${idea}” and spawning…`, confirm: undefined } : p));
    try {
      applyOutcome(await api.createIssueAndSpawn(repo, idea), idea);
    } catch (e) {
      setPicker((p) => (p ? { ...p, note: `Failed to create issue and spawn: ${String(e)}` } : p));
    }
  }

  // User chose to create the issue from their raw text despite Claude's prompt.
  async function confirmRawSpawn() {
    if (!picker?.confirm) return;
    const repo = picker.repo;
    const idea = picker.confirm.idea;
    setPicker((p) => (p ? { ...p, note: "Creating issue from your text…", confirm: undefined } : p));
    try {
      applyOutcome(await api.createIssueAndSpawn(repo, idea, true), idea);
    } catch (e) {
      setPicker((p) => (p ? { ...p, note: `Failed to create issue and spawn: ${String(e)}` } : p));
    }
  }

  // "Clean Up": tear down the worktree. Confirms first unless the backend says
  // it's safe (PR merged, nothing new).
  async function cleanUp(s: Session) {
    setCleanupConfirm(null);
    try {
      const res = await api.teardown(s.id);
      if (res.status === "needs_confirmation") {
        setCleanupConfirm({ id: s.id, warnings: res.warnings });
      } else {
        refreshSessions();
      }
    } catch (e) {
      setCleanupConfirm({ id: s.id, warnings: [`Teardown failed: ${String(e)}`] });
    }
  }

  async function confirmCleanup(id: string) {
    try {
      await api.teardown(id, true);
    } catch (e) {
      setCleanupConfirm({ id, warnings: [`Teardown failed: ${String(e)}`] });
      return;
    }
    setCleanupConfirm(null);
    setCommandsOpen((open) => (open === id ? null : open));
    refreshSessions();
  }

  return (
    <main className="panel">
      <header className="panel-header">
        <h1>m<span className="ai">AI</span>estro</h1>
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
            return (
              <div key={repo} className="repo-group">
                <div className="repo-group-header">
                  <span className="repo-group-name">{repo}</span>
                  <button className="btn-add" onClick={() => openStartWork(repo)}>
                    Start Work
                  </button>
                </div>

                {repoSessions.length === 0 ? (
                  <p className="repo-group-empty">No active work</p>
                ) : (
                  repoSessions.map((s) => {
                    const cmdOpen = commandsOpen === s.id;
                    const pr = prs[s.id];
                    const PrIcon = pr ? (PR_STATE_ICONS[pr.state] ?? PrOpenIcon) : null;
                    return (
                    <div key={s.id} className="workspace-item">
                      <div className="workspace-row" style={{ borderLeft: `3px solid ${s.color}` }}>
                        <span className="workspace-title">{s.session_title}</span>
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
                            <VSCodeIcon />
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
                        {/* Create PR / Merge PR are UI-only for now. */}
                        <button className="command-btn" onClick={() => {}}>Create PR</button>
                        <button className="command-btn" onClick={() => {}}>Merge PR</button>
                        <button className="command-btn" onClick={() => cleanUp(s)}>Clean Up</button>
                      </div>
                      {cleanupConfirm?.id === s.id && (
                        <div className="cleanup-confirm">
                          <p className="cleanup-lead">Remove this workspace?</p>
                          <ul className="cleanup-warnings">
                            {cleanupConfirm.warnings.map((w, i) => (
                              <li key={i}>{w}</li>
                            ))}
                          </ul>
                          <div className="issue-actions">
                            <button className="btn-danger" onClick={() => confirmCleanup(s.id)}>Remove anyway</button>
                            <button className="btn-ghost" onClick={() => setCleanupConfirm(null)}>Cancel</button>
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
        return (
          <div className="overlay" onClick={closePicker}>
            <div className="overlay-panel" onClick={(e) => e.stopPropagation()}>
              <div className="overlay-header">
                <span className="overlay-title">Start work · {picker.repo}</span>
                <button className="icon-btn" onClick={closePicker} aria-label="Close">✕</button>
              </div>
              <div className={`overlay-search-wrap ${ideaOpen ? "idea-open" : ""}`}>
                <input
                  className="text-input"
                  type="text"
                  placeholder="Write your own idea…"
                  value={picker.query}
                  onFocus={() => setExpanded({ kind: "idea" })}
                  onChange={(e) => setPicker((p) => (p ? { ...p, query: e.target.value } : p))}
                  spellCheck={false}
                  autoCapitalize="off"
                  autoCorrect="off"
                />
                {ideaOpen && (
                  <div className="issue-actions">
                    <button className="btn-save" disabled={!picker.query.trim()} onClick={createAndSpawn}>
                      Create Issue and Spawn
                    </button>
                    <button className="btn-ghost" onClick={() => setExpanded(null)}>Cancel</button>
                  </div>
                )}
              </div>
              <div className="overlay-body">
                {picker.loading ? (
                  <p className="repo-group-empty">Loading issues…</p>
                ) : picker.error ? (
                  <p className="cred-error">{picker.error}</p>
                ) : filtered.length > 0 ? (
                  <div className="issue-tree">
                    {filtered.map((n) => (
                      <IssueRow
                        key={n.number}
                        node={n}
                        depth={0}
                        expandedNumber={expanded?.kind === "issue" ? expanded.number : null}
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
                      <button className="btn-save" onClick={confirmRawSpawn}>Create issue from my text</button>
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
            </div>
          </div>
        );
      })()}
    </main>
  );
}

export default function App() {
  return getCurrentWindow().label === "settings" ? <Settings /> : <MainView />;
}
