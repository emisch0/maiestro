import { useState, useEffect, useCallback, useMemo } from "react";
import { api, CredentialScope, CredentialTypeDto, GHRepo, RepoSettings } from "./api";

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

export default function App() {
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
    <main className="panel">
      <header className="panel-header">
        <h1>m<span className="ai">AI</span>estro</h1>
        <span className="panel-subtitle">Credentials</span>
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
