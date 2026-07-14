// Pure helpers for the work-lifecycle axis (Planning / Implementing / Merged),
// the PR pill's mergeability copy, and the accent-color math. No React, no state
// — safe to import from any window component.
import { PrChecks, PrLink, WorkState } from "../api";
import PrOpenIcon from "../icons/pull-request-open.svg?react";
import PrDraftIcon from "../icons/pull-request-draft.svg?react";
import PrMergedIcon from "../icons/pull-request-merged.svg?react";
import PrClosedIcon from "../icons/pull-request-closed.svg?react";

// One static icon per PR state — colour is baked into each asset, so no runtime
// theming. Falls back to the open icon for any unexpected state string.
export const PR_STATE_ICONS: Record<string, typeof PrOpenIcon> = {
  open: PrOpenIcon,
  draft: PrDraftIcon,
  merged: PrMergedIcon,
  closed: PrClosedIcon,
};
export { PrOpenIcon };

// Tooltip text for the PR pill's merge indicator, off GitHub's mergeable_state
// (we can't read the Checks API with a fine-grained token).
export function checkLabel(checks: PrChecks): string {
  switch (checks.mergeable_state) {
    case "clean":
    case "has_hooks": return "Ready to merge";
    case "unstable": return "Mergeable — a non-required check is failing";
    case "dirty": return "Merge conflicts with the base branch";
    case "behind": return "Branch is behind the base — update it";
    case "blocked": return "Blocked by a required review or status check";
    case "draft": return "Draft — mark ready for review to merge";
    default: return "Checking mergeability…"; // unknown / not yet computed
  }
}

// A terminal reason the auto-merge can't proceed and won't self-resolve, or null
// to keep waiting (e.g. mergeability still computing, or a required check still
// running which GitHub will clear to "clean"). These need the user to act, so
// the loop stops and surfaces them. Driven purely by mergeable_state now.
export function mergeBlocker(c: PrChecks): string | null {
  switch (c.mergeable_state) {
    case "dirty": return "Merge conflicts with the base branch — resolve them and push, then merge again.";
    case "behind": return "Branch is behind the base — update it (merge or rebase) and push, then merge again.";
    case "blocked": return "Blocked by a required review or status check — resolve it, then merge again.";
    default: return null;
  }
}

// The work-lifecycle phase of a session, an axis distinct from the live Claude
// busy/idle status. A merged PR wins outright; otherwise any local work (commits
// ahead of base or an uncommitted change) means Implementing, and a pristine
// branch is still Planning. Returns null while the work state is unresolved so a
// working branch isn't briefly mislabeled "Planning".
export type LifecyclePhase = "planning" | "implementing" | "merged";
export function lifecyclePhase(pr: PrLink | null | undefined, work: WorkState | null | undefined): LifecyclePhase | null {
  if (pr?.state === "merged") return "merged";
  if (work === undefined) return null;
  if (work && (work.ahead > 0 || work.dirty)) return "implementing";
  return "planning";
}

export const LIFECYCLE_LABELS: Record<LifecyclePhase, string> = {
  planning: "Planning",
  implementing: "Implementing",
  merged: "Merged",
};

// Top-to-bottom order of the lifecycle zones within a repo group. Work items live
// in the zone matching their phase and slide between zones as the phase changes.
export const ZONES: LifecyclePhase[] = ["planning", "implementing", "merged"];

// How far along each phase is, used to collapse several sessions on one issue down
// to the single most-advanced phase for that issue's pill.
const PHASE_RANK: Record<LifecyclePhase, number> = { planning: 0, implementing: 1, merged: 2 };
export function furtherPhase(a: LifecyclePhase | null, b: LifecyclePhase | null): LifecyclePhase | null {
  if (a == null) return b;
  if (b == null) return a;
  return PHASE_RANK[a] >= PHASE_RANK[b] ? a : b;
}

// Issue number → its active session(s): the workspace color (for the dot) and a
// title to surface on hover. Issues present here have a spawned worktree.
export type ActiveSessions = Record<number, { color: string; title: string; phase: LifecyclePhase | null }>;

// The workspace palette colors are dark (they're VS Code title-bar backgrounds),
// so as a thin border or an icon tint on the dark popover they read as muddy and
// hard to tell apart. Keep each color's hue but pin it to a bright, uniform
// lightness/saturation so the eight hues separate cleanly. Falls back to the raw
// value for greys or anything unparseable.
export function accentColor(hex: string): string {
  const m = /^#?([0-9a-f]{6})$/i.exec(hex.trim());
  if (!m) return hex;
  const n = parseInt(m[1], 16);
  const r = (n >> 16) / 255, g = ((n >> 8) & 255) / 255, b = (n & 255) / 255;
  const max = Math.max(r, g, b), min = Math.min(r, g, b), d = max - min;
  if (d === 0) return hex;
  let h: number;
  if (max === r) h = ((g - b) / d) % 6;
  else if (max === g) h = (b - r) / d + 2;
  else h = (r - g) / d + 4;
  h = (h * 60 + 360) % 360;
  return `hsl(${Math.round(h)}, 68%, 62%)`;
}
