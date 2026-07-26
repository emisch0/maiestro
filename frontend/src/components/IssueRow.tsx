import { api, IssueNode } from "../api";
import { ActiveSessions, LIFECYCLE_LABELS } from "../lib/lifecycle";

interface IssueRowProps {
  node: IssueNode;
  depth: number;
  expandedNumber: number | null;
  // Issue number whose spawn preview is being prepared, so its row's button glows.
  preparingNumber: number | null;
  active: ActiveSessions;
  onExpand: (n: number) => void;
  onCollapse: () => void;
  onSpawn: (n: IssueNode) => void;
}

export function IssueRow({ node, depth, expandedNumber, preparingNumber, active, onExpand, onCollapse, onSpawn }: IssueRowProps) {
  const indent = 10 + depth * 16;
  const isExpanded = expandedNumber === node.number;
  const session = active[node.number];
  // A "working" pill tinted with the workspace color, marking issues that
  // already have a session. Such issues can't be spawned again, so they don't
  // expand into the Spawn Work action; the hover explains why.
  const workingHint = "There is already a work session for this issue";
  // The pill is tinted with the workspace color (its identity) and labelled with
  // the work's lifecycle phase, falling back to "working" until the phase loads.
  const workingPill = session && (
    <span className="issue-working-pill" style={{ background: session.color }} title={workingHint}>
      {session.phase ? LIFECYCLE_LABELS[session.phase] : "working"}
    </span>
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
            <button
              className={`btn-save ${preparingNumber === node.number ? "btn-busy" : ""}`}
              disabled={preparingNumber !== null}
              onClick={() => onSpawn(node)}
            >
              Spawn Work
            </button>
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
          preparingNumber={preparingNumber}
          active={active}
          onExpand={onExpand}
          onCollapse={onCollapse}
          onSpawn={onSpawn}
        />
      ))}
    </>
  );
}
