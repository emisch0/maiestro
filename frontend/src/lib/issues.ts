// Pure helpers for the Start Work issue picker: the branch/worktree slug (a mirror
// of the backend's slugify) and the issue-tree filter. No React.
import { IssueNode } from "../api";

// Slug for branch/worktree names — mirrors the backend's slugify (lowercase,
// non-alphanumerics collapsed to single dashes, trimmed, cut at a dash boundary)
// so the preview shows what the backend will actually use. The backend re-derives
// authoritatively on spawn (and may add a -2 suffix to dodge collisions).
export function slugify(s: string, maxLen = 25): string {
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
export function filterIssues(nodes: IssueNode[], query: string): IssueNode[] {
  const q = query.trim().toLowerCase();
  if (!q) return nodes;
  const keep = (n: IssueNode): IssueNode | null => {
    const children = n.children.map(keep).filter((c): c is IssueNode => c !== null);
    const self = n.title.toLowerCase().includes(q) || String(n.number).includes(q);
    return self || children.length ? { ...n, children } : null;
  };
  return nodes.map(keep).filter((n): n is IssueNode => n !== null);
}
