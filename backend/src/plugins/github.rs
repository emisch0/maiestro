use std::collections::{HashMap, HashSet};
use crate::credentials::{CredentialScope, CredentialStore};
use crate::plugin::{CredentialTypeInfo, Plugin};

const TYPES: &[CredentialTypeInfo] = &[CredentialTypeInfo {
    type_id: "github_token",
    display_name: "GitHub Token",
    env_var: "GITHUB_TOKEN",
    description: "Personal access token for GitHub API calls and git operations over HTTPS.",
}];

pub struct GitHubPlugin;

impl Plugin for GitHubPlugin {
    fn credential_types(&self) -> &[CredentialTypeInfo] { TYPES }
}

// ── Reusable REST client ────────────────────────────────────────────────────────

/// Authenticated GitHub REST client, scoped to a single identity's token.
/// Holds the token and a shared `reqwest::Client`; methods are thin wrappers
/// over the v3 API so callers (issue listing, spawning, …) don't re-implement
/// auth, headers, and error decoding.
pub struct GitHub {
    client: reqwest::Client,
    token: String,
}

impl GitHub {
    pub fn for_identity(identity_id: &str) -> Result<Self, String> {
        let scope = CredentialScope::Identity { identity_id: identity_id.to_string() };
        let token = CredentialStore::get("github_token", &scope)
            .map_err(|_| "No GitHub token found for this identity. Set one in Settings.".to_string())?;
        let client = reqwest::Client::builder()
            .user_agent("maiestro/0.1")
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self { client, token })
    }

    /// A request builder pre-loaded with auth and the standard GitHub headers.
    pub fn req(&self, method: reqwest::Method, url: &str) -> reqwest::RequestBuilder {
        self.client
            .request(method, url)
            .bearer_auth(&self.token)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
    }

    /// GET a URL and parse the JSON body, mapping non-2xx to a GitHub error message.
    pub async fn get_json(&self, url: &str) -> Result<serde_json::Value, String> {
        let resp = self.req(reqwest::Method::GET, url).send().await.map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(error_message(resp).await);
        }
        resp.json().await.map_err(|e| e.to_string())
    }

    /// Login of the token's owner (`GET /user`).
    pub async fn authenticated_login(&self) -> Result<String, String> {
        let v = self.get_json("https://api.github.com/user").await?;
        v["login"].as_str().map(str::to_string).ok_or_else(|| "could not resolve token user".to_string())
    }

    /// Repository metadata, including `default_branch`. `repo` is "owner/name".
    pub async fn repo(&self, repo: &str) -> Result<serde_json::Value, String> {
        self.get_json(&format!("https://api.github.com/repos/{repo}")).await
    }

    /// A single issue. `repo` is "owner/name".
    pub async fn issue(&self, repo: &str, number: u64) -> Result<serde_json::Value, String> {
        self.get_json(&format!("https://api.github.com/repos/{repo}/issues/{number}")).await
    }

    pub async fn add_assignees(&self, repo: &str, number: u64, assignees: &[String]) -> Result<(), String> {
        let url = format!("https://api.github.com/repos/{repo}/issues/{number}/assignees");
        let resp = self.req(reqwest::Method::POST, &url)
            .json(&serde_json::json!({ "assignees": assignees }))
            .send().await.map_err(|e| e.to_string())?;
        if resp.status().is_success() { Ok(()) } else { Err(error_message(resp).await) }
    }

    /// All pull requests (any state) whose head is `branch` on `repo` ("owner/name").
    pub async fn pulls_for_branch(&self, repo: &str, branch: &str) -> Result<Vec<serde_json::Value>, String> {
        let owner = repo.split('/').next().unwrap_or("");
        let url = format!(
            "https://api.github.com/repos/{repo}/pulls?head={owner}:{branch}&state=all&per_page=100"
        );
        let v = self.get_json(&url).await?;
        Ok(v.as_array().cloned().unwrap_or_default())
    }

    /// Open a new issue and return its number. `repo` is "owner/name".
    pub async fn create_issue(&self, repo: &str, title: &str, body: &str) -> Result<u64, String> {
        let url = format!("https://api.github.com/repos/{repo}/issues");
        let resp = self.req(reqwest::Method::POST, &url)
            .json(&serde_json::json!({ "title": title, "body": body }))
            .send().await.map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(error_message(resp).await);
        }
        let v: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
        v["number"].as_u64().ok_or_else(|| "issue created but no number returned".to_string())
    }

    pub async fn create_comment(&self, repo: &str, number: u64, body: &str) -> Result<(), String> {
        let url = format!("https://api.github.com/repos/{repo}/issues/{number}/comments");
        let resp = self.req(reqwest::Method::POST, &url)
            .json(&serde_json::json!({ "body": body }))
            .send().await.map_err(|e| e.to_string())?;
        if resp.status().is_success() { Ok(()) } else { Err(error_message(resp).await) }
    }

    /// Open a pull request and return the created PR object. `repo` is
    /// "owner/name"; `head` and `base` are branch names on that repo.
    pub async fn create_pull(
        &self,
        repo: &str,
        title: &str,
        head: &str,
        base: &str,
        body: &str,
        draft: bool,
    ) -> Result<serde_json::Value, String> {
        let url = format!("https://api.github.com/repos/{repo}/pulls");
        let resp = self.req(reqwest::Method::POST, &url)
            .json(&serde_json::json!({
                "title": title,
                "head": head,
                "base": base,
                "body": body,
                "draft": draft,
            }))
            .send().await.map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(error_message(resp).await);
        }
        resp.json().await.map_err(|e| e.to_string())
    }

    /// A single pull request. `repo` is "owner/name". The response carries the
    /// fields the merge flow needs: `mergeable_state`, the head `sha`, `draft`,
    /// and the GraphQL `node_id` used to mark a draft ready for review.
    pub async fn pull(&self, repo: &str, number: u64) -> Result<serde_json::Value, String> {
        self.get_json(&format!("https://api.github.com/repos/{repo}/pulls/{number}")).await
    }

    /// Check runs for a commit. `repo` is "owner/name"; `sha` is the head commit.
    /// Returns the bare `check_runs` array (each entry has `status` and
    /// `conclusion`); an empty vec means the commit has no checks configured.
    pub async fn check_runs(&self, repo: &str, sha: &str) -> Result<Vec<serde_json::Value>, String> {
        let url = format!("https://api.github.com/repos/{repo}/commits/{sha}/check-runs?per_page=100");
        let v = self.get_json(&url).await?;
        Ok(v["check_runs"].as_array().cloned().unwrap_or_default())
    }

    /// Mark a draft pull request ready for review. REST has no endpoint for this,
    /// so it goes through the GraphQL `markPullRequestReadyForReview` mutation.
    /// `node_id` is the PR's GraphQL id (the `node_id` field on the REST PR).
    pub async fn mark_ready(&self, node_id: &str) -> Result<(), String> {
        let query = "mutation($id: ID!) { \
            markPullRequestReadyForReview(input: { pullRequestId: $id }) { \
                pullRequest { isDraft } } }";
        let resp = self.req(reqwest::Method::POST, "https://api.github.com/graphql")
            .json(&serde_json::json!({ "query": query, "variables": { "id": node_id } }))
            .send().await.map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(error_message(resp).await);
        }
        // GraphQL returns 200 even on logical errors; surface them from `errors`.
        let v: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
        if let Some(err) = v["errors"][0]["message"].as_str() {
            return Err(err.to_string());
        }
        Ok(())
    }

    /// Merge a pull request. `repo` is "owner/name"; `method` is one of
    /// "merge" / "squash" / "rebase".
    pub async fn merge_pull(&self, repo: &str, number: u64, method: &str) -> Result<(), String> {
        let url = format!("https://api.github.com/repos/{repo}/pulls/{number}/merge");
        let resp = self.req(reqwest::Method::PUT, &url)
            .json(&serde_json::json!({ "merge_method": method }))
            .send().await.map_err(|e| e.to_string())?;
        if resp.status().is_success() { Ok(()) } else { Err(error_message(resp).await) }
    }
}

// ── Tauri commands ─────────────────────────────────────────────────────────────

#[derive(serde::Serialize)]
pub struct RepoItem {
    pub full_name: String,
    pub private: bool,
    pub description: Option<String>,
}

#[tauri::command]
pub async fn github_list_repos(identity_id: String) -> Result<Vec<RepoItem>, String> {
    let gh = GitHub::for_identity(&identity_id)?;

    let mut repos = Vec::new();
    let mut page: u32 = 1;

    loop {
        let resp = gh
            .req(reqwest::Method::GET, "https://api.github.com/user/repos")
            .query(&[
                ("per_page", "100"),
                ("page", &page.to_string()),
                ("sort", "pushed"),
                ("affiliation", "owner,collaborator,organization_member"),
            ])
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if !resp.status().is_success() {
            return Err(error_message(resp).await);
        }

        let page_data: Vec<serde_json::Value> = resp.json().await.map_err(|e| e.to_string())?;
        let count = page_data.len();

        for repo in page_data {
            repos.push(RepoItem {
                full_name: repo["full_name"].as_str().unwrap_or("").to_string(),
                private: repo["private"].as_bool().unwrap_or(false),
                description: repo["description"].as_str().map(|s| s.to_string()),
            });
        }

        if count < 100 || page >= 10 {
            break;
        }
        page += 1;
    }

    Ok(repos)
}

/// An open issue plus its open sub-issues, nested recursively.
#[derive(serde::Serialize)]
pub struct IssueNode {
    pub number: u64,
    pub title: String,
    pub html_url: String,
    pub children: Vec<IssueNode>,
}

struct IssueMeta {
    title: String,
    html_url: String,
    created_at: String, // ISO-8601; lexicographic order == chronological order
}

/// List a repo's open issues as a tree, nesting GitHub's native sub-issues under
/// their parent. `repo` is "owner/name". Pull requests are excluded, and issues
/// are ordered most-recently-created first at every level.
#[tauri::command]
pub async fn github_list_issues(identity_id: String, repo: String) -> Result<Vec<IssueNode>, String> {
    let (owner, name) = repo
        .split_once('/')
        .ok_or_else(|| format!("invalid repo (expected owner/name): {repo}"))?;

    let gh = GitHub::for_identity(&identity_id)?;

    // 1. Collect every open issue. The issues endpoint also returns PRs, so skip
    //    anything carrying a `pull_request` field. Remember which ones have
    //    sub-issues so we only make follow-up calls where needed.
    let mut meta: HashMap<u64, IssueMeta> = HashMap::new();
    let mut parents: Vec<u64> = Vec::new();
    let mut page: u32 = 1;

    loop {
        let resp = gh
            .req(reqwest::Method::GET, &format!("https://api.github.com/repos/{owner}/{name}/issues"))
            .query(&[
                ("state", "open"),
                ("sort", "created"),
                ("direction", "desc"),
                ("per_page", "100"),
                ("page", &page.to_string()),
            ])
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if !resp.status().is_success() {
            return Err(error_message(resp).await);
        }

        let data: Vec<serde_json::Value> = resp.json().await.map_err(|e| e.to_string())?;
        let count = data.len();

        for issue in data {
            if issue.get("pull_request").is_some() {
                continue;
            }
            let Some(number) = issue["number"].as_u64() else { continue };
            meta.insert(number, issue_meta(&issue));
            if issue["sub_issues_summary"]["total"].as_u64().unwrap_or(0) > 0 {
                parents.push(number);
            }
        }

        if count < 100 || page >= 10 {
            break;
        }
        page += 1;
    }

    // 2. For each issue that has sub-issues, fetch them to learn the hierarchy.
    let mut children_of: HashMap<u64, Vec<u64>> = HashMap::new();
    let mut is_child: HashSet<u64> = HashSet::new();

    for parent in parents {
        let resp = gh
            .req(reqwest::Method::GET, &format!("https://api.github.com/repos/{owner}/{name}/issues/{parent}/sub_issues"))
            .query(&[("per_page", "100")])
            .send()
            .await
            .map_err(|e| e.to_string())?;

        // A repo without the sub-issues feature returns an error here; treat that
        // as "no children" rather than failing the whole list.
        if !resp.status().is_success() {
            continue;
        }

        let subs: Vec<serde_json::Value> = resp.json().await.map_err(|e| e.to_string())?;
        let mut kids = Vec::new();
        for sub in subs {
            let Some(n) = sub["number"].as_u64() else { continue };
            if sub["state"].as_str() != Some("open") {
                continue;
            }
            meta.entry(n).or_insert_with(|| issue_meta(&sub));
            kids.push(n);
            is_child.insert(n);
        }
        children_of.insert(parent, kids);
    }

    // 3. Order every sibling group most-recently-created first.
    let by_recency = |a: &u64, b: &u64| {
        let ca = meta.get(a).map(|m| m.created_at.as_str()).unwrap_or("");
        let cb = meta.get(b).map(|m| m.created_at.as_str()).unwrap_or("");
        cb.cmp(ca).then(b.cmp(a))
    };
    for kids in children_of.values_mut() {
        kids.sort_by(by_recency);
    }

    // Roots are open issues that aren't anyone's sub-issue; build down from each.
    let mut roots: Vec<u64> = meta.keys().copied().filter(|n| !is_child.contains(n)).collect();
    roots.sort_by(by_recency);

    let mut visited = HashSet::new();
    let tree = roots
        .iter()
        .filter_map(|n| build_issue_node(*n, &meta, &children_of, &mut visited))
        .collect();

    Ok(tree)
}

fn issue_meta(issue: &serde_json::Value) -> IssueMeta {
    IssueMeta {
        title: issue["title"].as_str().unwrap_or("").to_string(),
        html_url: issue["html_url"].as_str().unwrap_or("").to_string(),
        created_at: issue["created_at"].as_str().unwrap_or("").to_string(),
    }
}

fn build_issue_node(
    number: u64,
    meta: &HashMap<u64, IssueMeta>,
    children_of: &HashMap<u64, Vec<u64>>,
    visited: &mut HashSet<u64>,
) -> Option<IssueNode> {
    if !visited.insert(number) {
        return None; // guard against unexpected cycles
    }
    let m = meta.get(&number)?;
    let children = children_of
        .get(&number)
        .map(|kids| {
            kids.iter()
                .filter_map(|c| build_issue_node(*c, meta, children_of, visited))
                .collect()
        })
        .unwrap_or_default();
    Some(IssueNode {
        number,
        title: m.title.clone(),
        html_url: m.html_url.clone(),
        children,
    })
}

async fn error_message(resp: reqwest::Response) -> String {
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v["message"].as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| format!("HTTP {}", status.as_u16()))
}
