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

  githubListRepos: (identityId: string) =>
    invoke<GHRepo[]>("github_list_repos", { identityId }),
};
