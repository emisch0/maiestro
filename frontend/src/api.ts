import { invoke } from "@tauri-apps/api/core";

export interface RepoSettings {
  checkout_dir: string | null;
  env_files: string[];
  identity_id: string | null;
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
};
