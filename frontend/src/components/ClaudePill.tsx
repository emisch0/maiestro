import { StatusRecord } from "../api";
import ClaudeIcon from "../icons/claude.svg?react";

// How each status state renders in a work-item row. `running`/`idle` are quiet;
// `busy` and `needs_you` draw attention. States not in the map render nothing.
// The `creating` state (worktree being built after a spawn, issue #77) is
// deliberately absent: it renders as a `workspace-op-pill` ("Creating…") next to
// the title — like "Merging…"/"Tearing down…" — not as a Claude status pill, so
// ClaudePill renders nothing for it.
const STATUS_LABELS: Record<string, string> = {
  running: "Ready",
  busy: "Working",
  needs_you: "Needs you",
  idle: "Idle",
};

// The Claude session pill: the Claude mark tints by live state (green=working,
// amber=needs you, muted=ready/idle), with the status word beside it. Clicking
// jumps to where the session lives — the worktree's VS Code window (there is no
// deep link to the remote-controlled session itself). Renders nothing until a
// status exists, and once the session has ended.
export function ClaudePill({ status, onClick }: { status?: StatusRecord; onClick: () => void }) {
  if (!status || status.state === "ended") return null;
  const label = STATUS_LABELS[status.state];
  if (!label) return null;
  // `needs_you` carries the reason (e.g. the permission request) in `detail`.
  const title = status.detail ? `Claude · ${label} — ${status.detail}` : `Claude · ${label}`;
  // A *surfaced* failed tool tints the pill red; a pending/transient one Claude
  // may still recover from doesn't. The error itself lives in the dismissible row
  // block, not this tooltip.
  const cls = `claude-pill claude-pill--${status.state}${status.state === "busy" ? " busy-ring busy-ring--ai" : ""}${status.last_error?.surfaced ? " claude-pill--error" : ""}`;
  return (
    <button
      className={cls}
      onClick={onClick}
      title={title}
      aria-label={title}
    >
      <ClaudeIcon className="claude-pill-mark" />
      <span className="claude-pill-label">{label}</span>
    </button>
  );
}
