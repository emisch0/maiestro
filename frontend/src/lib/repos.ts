// A repo id segment as GitHub allows it in an owner or repo name.
const SEGMENT = /^[A-Za-z0-9._-]+$/;

/** The GitHub homepage for a canonical `"owner/name"` repo id, or `null` when
 *  the id isn't exactly two GitHub-legal segments (no link is shown then). */
export function repoGitHubUrl(repo: string): string | null {
  const parts = repo.split("/");
  if (parts.length !== 2 || !parts.every((p) => SEGMENT.test(p))) return null;
  return `https://github.com/${parts[0]}/${parts[1]}`;
}
