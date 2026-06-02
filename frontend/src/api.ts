import { invoke } from "@tauri-apps/api/core";

/** Hide/snooze state for a repo or work item. Presence = hidden. */
export interface HideState {
  /** Unix-epoch millis to stay hidden until; null = hidden indefinitely. */
  snooze_until: number | null;
}

export interface RepoSettings {
  checkout_dir: string | null;
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

export type TeardownOutcome =
  | { status: "done" }
  | { status: "needs_confirmation"; warnings: string[] };

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
  /** One of "passed", "failed", "running", "pending", "none". */
  state: string;
  /** Any check actively in_progress — animate the indicator only when true. */
  running: boolean;
  /** GitHub reports the PR mergeable (`mergeable_state == "clean"`). */
  ready_to_merge: boolean;
}

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

  spawnWork: (repo: string, issueNumber: number, forceNew = false) =>
    invoke<SpawnResult>("spawn_work", { repo, issueNumber, forceNew }),

  createIssueAndSpawn: (repo: string, idea: string, useRawFallback = false, forceNew = false) =>
    invoke<CreateAndSpawnOutcome>("create_issue_and_spawn", { repo, idea, useRawFallback, forceNew }),

  sessionsList: () =>
    invoke<Session[]>("sessions_list"),

  teardown: (sessionId: string, confirmed = false) =>
    invoke<TeardownOutcome>("teardown", { sessionId, confirmed }),

  sessionPr: (sessionId: string) =>
    invoke<PrLink | null>("session_pr", { sessionId }),

  createPr: (sessionId: string) =>
    invoke<PrLink>("session_create_pr", { sessionId }),

  sessionPrChecks: (sessionId: string) =>
    invoke<PrChecks | null>("session_pr_checks", { sessionId }),

  mergePr: (sessionId: string) =>
    invoke<PrLink>("session_merge_pr", { sessionId }),
};
