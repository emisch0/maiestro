// Pure helpers for repo/work-item hide & snooze state: whether something reads as
// hidden right now, the remaining-time label, and the snooze presets. No React.
import { HideState, Session } from "../api";

export type HideTarget =
  | { kind: "repo"; repo: string }
  | { kind: "session"; session: Session };

// An item is effectively hidden when it has a HideState that is either indefinite
// (no snooze) or a snooze whose deadline hasn't passed. An expired snooze reads as
// visible — that's the automatic un-snooze, resolved at render time (no live tick).
export function effectiveHidden(h: HideState | null | undefined, now: number): boolean {
  if (!h) return false;
  return h.snooze_until == null || h.snooze_until > now;
}

// Remaining-time label for a snooze: days + hours normally, dropping to minutes
// only under an hour. Empty string once expired.
export function formatSnoozeRemaining(snoozeUntil: number, now: number): string {
  const ms = snoozeUntil - now;
  if (ms <= 0) return "";
  const mins = Math.floor(ms / 60000);
  if (mins < 60) return `${Math.max(1, mins)}m`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours}h`;
  const days = Math.floor(hours / 24);
  const remHours = hours % 24;
  return remHours ? `${days}d ${remHours}h` : `${days}d`;
}

// Default the snooze picker to ~24h out, formatted for a datetime-local input
// (local time, no timezone suffix).
export function defaultSnoozeLocal(): string {
  const d = new Date(Date.now() + 24 * 60 * 60 * 1000);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

// Quick snooze presets, resolved against `now`: 1 hour out, tomorrow at 8am
// local, and next Monday at 8am local. Anything else uses the custom picker.
export function snoozePresets(now: number): { label: string; hint: string; at: number }[] {
  const timeFmt = (d: Date) => d.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
  const dayTimeFmt = (d: Date) => d.toLocaleString([], { weekday: "short", hour: "numeric", minute: "2-digit" });

  const oneHour = new Date(now + 60 * 60 * 1000);

  const tomorrow = new Date(now);
  tomorrow.setDate(tomorrow.getDate() + 1);
  tomorrow.setHours(8, 0, 0, 0);

  const nextWeek = new Date(now);
  // Days until the *next* Monday — always strictly in the future (1–7).
  const daysUntilMon = ((8 - nextWeek.getDay()) % 7) || 7;
  nextWeek.setDate(nextWeek.getDate() + daysUntilMon);
  nextWeek.setHours(8, 0, 0, 0);

  return [
    { label: "Snooze for 1 hour", hint: timeFmt(oneHour), at: oneHour.getTime() },
    { label: "Snooze until tomorrow", hint: dayTimeFmt(tomorrow), at: tomorrow.getTime() },
    { label: "Snooze until next week", hint: dayTimeFmt(nextWeek), at: nextWeek.getTime() },
  ];
}
