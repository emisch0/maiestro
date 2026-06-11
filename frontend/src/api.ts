import { invoke } from "@tauri-apps/api/core";

/** Hide/snooze state for a repo or work item. Presence = hidden. */
export interface HideState {
  /** Unix-epoch millis to stay hidden until; null = hidden indefinitely. */
  snooze_until: number | null;
}

export interface RepoSettings {
  checkout_dir: string | null;
  worktree_prefix: string | null;
  env_files: string[];
  identity_id: string | null;
  hidden: HideState | null;
}

export interface GHRepo {
  full_name: string;
  private: boolean;
  description: string | null;
}

export interface IssueNode {
  number: number;
  title: string;
  html_url: string;
  children: IssueNode[];
}

export interface SpawnResult {
  work_dir: string;
  branch: string;
  issue_url: string;
  reused: boolean;
  warnings: string[];
}

export type CreateAndSpawnOutcome =
  | ({ status: "spawned" } & SpawnResult)
  | { status: "needs_confirmation"; message: string };

export type CreateIssueOutcome =
  | { status: "created"; number: number; issue_url: string; warnings: string[] }
  | { status: "needs_confirmation"; message: string };

/** Editable fields shown in the spawn preview before a worktree is created. */
export interface SpawnPlan {
  repo: string;
  /** null on the create-and-spawn path: the issue isn't opened until confirm. */
  issue_number: number | null;
  issue_title: string;
  issue_body: string;
  short_title: string;
  color: string;
  emoji: string;
  /** Local checkout dir name, for rendering the worktree path. */
  repo_name: string;
}

export type DraftPreviewOutcome =
  | ({ status: "drafted" } & SpawnPlan)
  | { status: "needs_confirmation"; message: string };

/** The reviewed preview sent back on confirm. */
export interface SpawnEdits {
  issue_number: number | null;
  issue_title: string;
  issue_body: string;
  short_title: string;
  color: string;
  emoji: string;
  /** Existing issue only: PATCH the title/body back to GitHub. */
  update_issue: boolean;
}

export type TeardownOutcome =
  | { status: "done" }
  | { status: "needs_confirmation"; warnings: string[] }
  | { status: "blocked_by_editor"; message: string; accessibility: boolean };

export interface Session {
  id: string;
  repo: string;
  issue_number: number;
  issue_url: string;
  branch: string;
  work_dir: string;
  checkout_dir: string;
  session_title: string;
  color: string;
  emoji: string;
  hidden: HideState | null;
}

export interface PrLink {
  number: number;
  html_url: string;
  title: string;
  /** One of "draft", "open", "merged", "closed". */
  state: string;
}

export interface PrChecks {
  /** Pill indicator derived from mergeable_state: "passed" (mergeable),
   *  "failed" (conflicts), "pending" (behind/blocked/computing), "none" (draft). */
  state: string;
  /** Mergeability still being computed by GitHub — animate the indicator. */
  running: boolean;
  /** GitHub reports the PR mergeable (`mergeable_state == "clean"`). */
  ready_to_merge: boolean;
  /** Raw GitHub mergeable_state: clean / dirty / behind / blocked / unstable /
   *  draft / unknown. Used to stop the auto-merge loop on terminal blockers. */
  mergeable_state: string;
}

/** Live per-session status, written by the `maiestro hook` helper and watched
 *  by the backend. Pushed to the UI via the `session-status` event and read in
 *  bulk via `sessions_status_list`. */
export interface StatusRecord {
  /** Workspace id (= Session.id). */
  workspace: string;
  /** One of "running", "busy", "needs_you", "idle", "ended". */
  state: string;
  session_id?: string;
  cwd?: string;
  /** Short human detail, e.g. "permission: Bash" or a tool name. */
  detail?: string;
  /** The most recent failed tool call, kept until dismissed or a new turn. */
  last_error?: ToolError;
  ts: string;
}

/** A failed tool call from a `PostToolUseFailure` hook. Logged on every failure
 *  but only shown in the popover once `surfaced` is true (issue #48). */
export interface ToolError {
  tool?: string;
  message: string;
  ts: string;
  /** Consecutive failures of this same tool. */
  count: number;
  /** Whether to show this error prominently. False = pending/transient (hidden);
   *  true = Claude stopped without recovering, or the tool failed repeatedly. */
  surfaced: boolean;
}

/** Local git facts about a session's worktree, used to derive the work-lifecycle
 *  phase (planning → implementing → merged). Purely local — no GitHub call. */
export interface WorkState {
  /** Commits in origin/<default_branch>..HEAD (local ref; may lag origin). */
  ahead: number;
  /** Worktree has uncommitted changes. */
  dirty: boolean;
}

/** Chosen UI appearance. "system" follows the macOS dark/light setting. */
export type Theme = "light" | "dark" | "system";

export type CredentialScope = { kind: "identity"; identity_id: string };

export interface CredentialTypeDto {
  type_id: string;
  display_name: string;
  env_var: string;
  description: string;
}

export const api = {
  listCredentialTypes: () =>
    invoke<CredentialTypeDto[]>("plugins_list_credential_types"),

  getCredential: (type_id: string, scope: CredentialScope) =>
    invoke<string>("credentials_get", { typeId: type_id, scope }),

  setCredential: (type_id: string, scope: CredentialScope, secret: string) =>
    invoke<void>("credentials_set", { typeId: type_id, scope, secret }),

  deleteCredential: (type_id: string, scope: CredentialScope) =>
    invoke<void>("credentials_delete", { typeId: type_id, scope }),

  listRepos: () =>
    invoke<string[]>("repos_list"),

  getRepoSettings: (repo: string) =>
    invoke<RepoSettings>("repo_settings_get", { repo }),

  setRepoSettings: (repo: string, settings: RepoSettings) =>
    invoke<void>("repo_settings_set", { repo, settings }),

  setRepoVisibility: (repo: string, hidden: HideState | null) =>
    invoke<void>("repo_set_visibility", { repo, hidden }),

  setSessionVisibility: (sessionId: string, hidden: HideState | null) =>
    invoke<void>("session_set_visibility", { sessionId, hidden }),

  scanEnvFiles: (checkoutDir: string) =>
    invoke<string[]>("repo_scan_env_files", { checkoutDir }),

  identitiesList: () =>
    invoke<string[]>("identities_list"),

  getDefaultIdentity: () =>
    invoke<string | null>("identities_get_default"),

  setDefaultIdentity: (identityId: string) =>
    invoke<void>("identities_set_default", { identityId }),

  identitiesAdd: (identityId: string) =>
    invoke<void>("identities_add", { identityId }),

  githubListRepos: (identityId: string) =>
    invoke<GHRepo[]>("github_list_repos", { identityId }),

  githubListIssues: (identityId: string, repo: string) =>
    invoke<IssueNode[]>("github_list_issues", { identityId, repo }),

  openUrl: (url: string) =>
    invoke<void>("open_url", { url }),

  openPath: (path: string) =>
    invoke<void>("open_path", { path }),

  openInEditor: (workDir: string) =>
    invoke<void>("open_in_editor", { workDir }),

  openRepoInEditor: (repo: string) =>
    invoke<void>("open_repo_in_editor", { repo }),

  spawnWork: (repo: string, issueNumber: number, forceNew = false) =>
    invoke<SpawnResult>("spawn_work", { repo, issueNumber, forceNew }),

  prepareSpawn: (repo: string, issueNumber: number) =>
    invoke<SpawnPlan>("prepare_spawn", { repo, issueNumber }),

  suggestShortTitle: (repo: string, issueNumber: number) =>
    invoke<string>("suggest_short_title", { repo, issueNumber }),

  draftSpawnPreview: (repo: string, idea: string, useRawFallback = false) =>
    invoke<DraftPreviewOutcome>("draft_spawn_preview", { repo, idea, useRawFallback }),

  confirmSpawn: (repo: string, edits: SpawnEdits, forceNew = false) =>
    invoke<SpawnResult>("confirm_spawn", { repo, edits, forceNew }),

  createIssue: (repo: string, idea: string, useRawFallback = false) =>
    invoke<CreateIssueOutcome>("create_issue", { repo, idea, useRawFallback }),

  createIssueDirect: (repo: string, title: string, body: string) =>
    invoke<CreateIssueOutcome>("create_issue_direct", { repo, title, body }),

  createIssueAndSpawn: (repo: string, idea: string, useRawFallback = false, forceNew = false) =>
    invoke<CreateAndSpawnOutcome>("create_issue_and_spawn", { repo, idea, useRawFallback, forceNew }),

  sessionsList: () =>
    invoke<Session[]>("sessions_list"),

  sessionsStatusList: () =>
    invoke<StatusRecord[]>("sessions_status_list"),

  clearSessionError: (workspace: string) =>
    invoke<void>("clear_session_error", { workspace }),

  teardown: (sessionId: string, confirmed = false, force = false) =>
    invoke<TeardownOutcome>("teardown", { sessionId, confirmed, force }),

  openAccessibilitySettings: () =>
    invoke<void>("open_accessibility_settings"),

  sessionPr: (sessionId: string) =>
    invoke<PrLink | null>("session_pr", { sessionId }),

  createPr: (sessionId: string) =>
    invoke<PrLink>("session_create_pr", { sessionId }),

  sessionPrChecks: (sessionId: string) =>
    invoke<PrChecks | null>("session_pr_checks", { sessionId }),

  sessionWorkState: (sessionId: string) =>
    invoke<WorkState | null>("session_work_state", { sessionId }),

  mergePr: (sessionId: string) =>
    invoke<PrLink>("session_merge_pr", { sessionId }),

  logsRead: () =>
    invoke<string>("logs_read"),

  logsReveal: () =>
    invoke<void>("logs_reveal"),

  getTheme: () =>
    invoke<Theme>("app_settings_get_theme"),

  setTheme: (theme: Theme) =>
    invoke<void>("app_settings_set_theme", { theme }),
};
