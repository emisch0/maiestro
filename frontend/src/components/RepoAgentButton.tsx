import { Agent } from "../api";
import { AGENT_MARKS, AGENT_PRODUCTS } from "../lib/agents";

// The repo header's agent icon (issue #217): the mark of the repo's effective
// agentic coding CLI, next to its VS Code button. Clicking opens the
// `RepoAgentDialog` picker. `pinned` is whether the repo sets `agent` itself
// rather than following the global default.
export function RepoAgentButton({ agent, pinned, onClick }: {
  agent: Agent;
  pinned: boolean;
  onClick: () => void;
}) {
  const Mark = AGENT_MARKS[agent] ?? AGENT_MARKS.claude;
  const label = `Agentic coding CLI: ${AGENT_PRODUCTS[agent] ?? AGENT_PRODUCTS.claude}${pinned ? "" : " (global default)"}`;
  return (
    <button className="pill-btn repo-agent" onClick={onClick} title={label} aria-label={label}>
      <Mark />
    </button>
  );
}
