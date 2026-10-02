import { Agent } from "../api";
import { AgentChoices } from "./AgentSwitchDialog";
import { OverlayDialog } from "./OverlayDialog";

// Opened from a repo's "Agentic Coding CLI…" command (issue #214): pin the
// agent this repo's new work items start with, i.e. its `agent` setting. There
// is no "use global default" choice; resetting to it is done in Settings. A
// repo still on the global default can pick that same agent to pin it.
export function RepoAgentDialog({ repo, current, pinned, onConfirm, onClose }: {
  repo: string;
  /** The repo's effective agent: its own `agent`, else the global default. */
  current: Agent;
  /** Whether the repo sets `agent` itself (rather than following the default). */
  pinned: boolean;
  onConfirm: (agent: Agent) => void;
  onClose: () => void;
}) {
  return (
    <OverlayDialog title={`Agentic Coding CLI · ${repo}`} panelClass="hide-dialog" onClose={onClose}>
      <div className="hide-dialog-body">
        <p className="codex-hooks-lead">
          Choose the agentic coding CLI this repo&apos;s new work items start with. Existing work items keep
          theirs.
        </p>
        <AgentChoices
          current={current}
          currentHint={pinned ? "This repo's agentic coding CLI now" : "The global default now · pick to pin it"}
          currentSelectable={!pinned}
          onConfirm={onConfirm}
        />
      </div>
    </OverlayDialog>
  );
}
