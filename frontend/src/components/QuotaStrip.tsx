// The subscription-quota strip along the popover's bottom edge (#205): one chip per
// agent account behind the visible work items — Claude `5h 42% · 7d 18%`,
// Codex `30d 5%`, Copilot `premium 75%` — each window a small meter. Quota is
// per account, not per session, so an agent shows once however many sessions
// use it. The parent decides which readings to pass (only visible agents); with
// none, this renders nothing.
//
// A meter's color is a forecast, not a raw threshold: under 50% used it is
// green; past that, the usage so far is projected linearly to the window's
// reset (`forecastPercent`), so 60% used with an hour of a 5h window left is
// still green. See `quotaLevel` for the bands.

import { ProviderQuota, QuotaWindow } from "../api";
import { AGENT_MARKS, AGENT_NAMES } from "../lib/agents";

/** Below this much used, a window is green whatever the pace. */
const CALM_PERCENT = 50;
/** A forecast up to this is green (on pace to last until the reset)… */
const ON_PACE_PERCENT = 100;
/** …up to this amber (would run out shortly before the reset), beyond it red. */
const OVER_PACE_PERCENT = 125;

/** Usage projected to the window's reset at the pace so far (`used ÷ fraction
 *  of the window elapsed`), or `null` when the window's length or reset is
 *  unknown, or it has reset. */
export function forecastPercent(w: QuotaWindow, nowSecs: number): number | null {
  if (w.resets_at === null || w.window_secs === null || w.window_secs <= 0) return null;
  const left = w.resets_at - nowSecs;
  if (left <= 0) return null;
  const elapsed = Math.min(1, Math.max(0, 1 - left / w.window_secs));
  return elapsed > 0 ? w.used_percent / elapsed : Infinity;
}

/** Meter tint for a window:
 *  - under 50% used → green;
 *  - 100% or more used → red (already exhausted);
 *  - otherwise by the forecast at reset: ≤ 100% green, ≤ 125% amber, else red;
 *  - with no forecast (length or reset unknown), the plain thresholds:
 *    amber from 80%, red from 95%. */
export function quotaLevel(w: QuotaWindow, nowSecs: number): "ok" | "warn" | "high" {
  const used = w.used_percent;
  if (used < CALM_PERCENT) return "ok";
  if (used >= 100) return "high";
  const forecast = forecastPercent(w, nowSecs);
  if (forecast === null) return used >= 95 ? "high" : used >= 80 ? "warn" : "ok";
  if (forecast <= ON_PACE_PERCENT) return "ok";
  return forecast <= OVER_PACE_PERCENT ? "warn" : "high";
}

/** "2 h 10 m" / "3 d 4 h" / "5 m" for a span in seconds. */
export function formatSpan(secs: number): string {
  const m = Math.max(0, Math.round(secs / 60));
  if (m < 60) return `${m} m`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h} h ${m % 60} m`;
  return `${Math.floor(h / 24)} d ${h % 24} h`;
}

/** Whether a window has reset since it was read, so its percentage is stale. */
function hasReset(w: QuotaWindow, nowSecs: number): boolean {
  return w.resets_at !== null && w.resets_at <= nowSecs;
}

function windowTitle(w: QuotaWindow, nowSecs: number): string {
  if (hasReset(w, nowSecs)) return `${w.label}: reset since last reading`;
  let line = `${w.label}: ${Math.round(w.used_percent)}% used`;
  if (w.resets_at !== null) line += `, resets in ${formatSpan(w.resets_at - nowSecs)}`;
  // The pace only matters once it can change the color.
  const forecast = w.used_percent >= CALM_PERCENT && w.used_percent < 100 ? forecastPercent(w, nowSecs) : null;
  if (forecast !== null) {
    line += forecast > ON_PACE_PERCENT
      ? ` — at this pace, runs out ${formatSpan(((100 - w.used_percent) / w.used_percent) * (w.window_secs! - (w.resets_at! - nowSecs)))} from now`
      : " — on pace to last until the reset";
  }
  return line;
}

/** The chip's tooltip: name, account and plan, each window, and the reading's age. */
export function quotaTitle(q: ProviderQuota, nowSecs: number): string {
  const who = [AGENT_NAMES[q.agent], q.account, q.plan && `${q.plan} plan`].filter(Boolean).join(" · ");
  const lines = q.windows.map((w) => windowTitle(w, nowSecs));
  return [who, ...lines, `as of ${formatSpan(nowSecs - q.observed_at)} ago`].join("\n");
}

/** Stable identity of a chip: one per agent + account. */
export const quotaKey = (q: ProviderQuota) => `${q.agent}:${q.account ?? ""}`;

export function QuotaStrip({ quotas, now }: { quotas: ProviderQuota[]; now: number }) {
  if (quotas.length === 0) return null;
  const nowSecs = Math.floor(now / 1000);
  return (
    <div className="quota-strip" aria-label="Subscription usage">
      {quotas.map((q) => {
        const Mark = AGENT_MARKS[q.agent];
        const title = quotaTitle(q, nowSecs);
        return (
          <div key={quotaKey(q)} className="quota-chip" title={title} aria-label={title}>
            <Mark className="quota-chip-mark" aria-hidden="true" />
            {q.windows.map((w) => {
              const reset = hasReset(w, nowSecs);
              const pct = Math.round(w.used_percent);
              return (
                <span key={w.label} className={`quota-window quota-window--${reset ? "reset" : quotaLevel(w, nowSecs)}`}>
                  <span className="quota-window-label">{w.label}</span>
                  <span className="quota-meter" aria-hidden="true">
                    <span className="quota-meter-fill" style={{ width: reset ? 0 : `${Math.min(100, Math.max(0, pct))}%` }} />
                  </span>
                  <span className="quota-window-value">{reset ? "—" : `${pct}%`}</span>
                </span>
              );
            })}
          </div>
        );
      })}
    </div>
  );
}
