import { describe, it, expect, vi, afterEach } from "vitest";
import { act, fireEvent, render, screen } from "@testing-library/react";
import { ProviderQuota, QuotaWindow } from "../api";
import { forecastPercent, formatSpan, HOVER_DELAY_MS, quotaLevel, QuotaStrip, quotaTitle } from "./QuotaStrip";

const NOW = Date.UTC(2026, 9, 2, 12, 0, 0);
const nowSecs = NOW / 1000;

const claude: ProviderQuota = {
  agent: "claude",
  account: null,
  plan: null,
  windows: [
    { label: "5h", used_percent: 42.4, resets_at: nowSecs + 2 * 3600 + 600, window_secs: 5 * 3600 },
    // 4 of 7 days gone, 83% used → on pace for ~145%.
    { label: "7d", used_percent: 83, resets_at: nowSecs + 3 * 86400, window_secs: 7 * 86400 },
  ],
  observed_at: nowSecs - 180,
};
const copilot: ProviderQuota = {
  agent: "copilot",
  account: "octo",
  plan: "individual",
  // No reset known → no forecast; the plain thresholds apply.
  windows: [{ label: "premium", used_percent: 82, resets_at: null, window_secs: null }],
  observed_at: nowSecs,
};

describe("QuotaStrip (#205)", () => {
  it("renders nothing with no readings", () => {
    const { container } = render(<QuotaStrip quotas={[]} now={NOW} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("shows one chip per account with a meter per window, tinted by threshold", () => {
    const { container } = render(<QuotaStrip quotas={[claude, copilot]} now={NOW} />);
    const chips = container.querySelectorAll(".quota-chip");
    expect(chips).toHaveLength(2);
    expect(chips[0]).toHaveTextContent("5h42%7d83%");
    expect(chips[1]).toHaveTextContent("premium82%");
    const classes = [...container.querySelectorAll(".quota-window")].map((w) => w.className);
    expect(classes).toEqual([
      "quota-window quota-window--ok",
      "quota-window quota-window--high",
      "quota-window quota-window--warn",
    ]);
  });

  it("shows a dash for a window that reset since the reading", () => {
    const stale = { ...claude, windows: [{ label: "5h", used_percent: 99, resets_at: nowSecs - 60, window_secs: 18000 }] };
    const { container } = render(<QuotaStrip quotas={[stale]} now={NOW} />);
    expect(container.querySelector(".quota-window--reset")).toHaveTextContent("5h—");
    expect(screen.getByLabelText(/5h: reset since last reading/)).toBeInTheDocument();
  });

  it("puts plan, resets and the reading's age in the tooltip", () => {
    expect(quotaTitle(claude, nowSecs)).toBe(
      "Claude\n5h: 42% used, resets in 2 h 10 m\n7d: 83% used, resets in 3 d 0 h — at this pace, runs out 19 h 40 m from now\nas of 3 m ago",
    );
    expect(quotaTitle(copilot, nowSecs)).toBe("Copilot · octo · individual plan\npremium: 82% used\nas of 0 m ago");
  });

  it("colors by the forecast at reset once past 50%", () => {
    const H = 3600;
    // A 5h window with `left` seconds to go and `used` percent used.
    const w = (used: number, left: number): QuotaWindow =>
      ({ label: "5h", used_percent: used, resets_at: nowSecs + left, window_secs: 5 * H });
    const level = (used: number, left: number) => quotaLevel(w(used, left), nowSecs);

    expect(level(49, 4.9 * H)).toBe("ok"); // under 50%: green, however fast
    expect(level(60, 1 * H)).toBe("ok"); // 60% with 1h left → on pace for 75%
    expect(forecastPercent(w(60, 1 * H), nowSecs)).toBeCloseTo(75);
    expect(level(60, 2.5 * H)).toBe("warn"); // halfway → 120%
    expect(level(60, 4 * H)).toBe("high"); // 1h in → 300%
    expect(level(99, 60)).toBe("ok"); // a minute from the reset, 99% will do
    expect(level(100, 60)).toBe("high"); // exhausted is red, whatever the clock
    // No length or reset → the plain 80/95 thresholds.
    const plain = (used: number) => quotaLevel({ label: "chat", used_percent: used, resets_at: null, window_secs: null }, nowSecs);
    expect([79, 80, 94, 95].map(plain)).toEqual(["ok", "warn", "warn", "high"]);
  });

  it("formats spans", () => {
    expect(formatSpan(-5)).toBe("0 m");
    expect(formatSpan(59 * 60)).toBe("59 m");
    expect(formatSpan(25 * 3600)).toBe("1 d 1 h");
  });

  describe("tooltip (#214)", () => {
    afterEach(() => vi.useRealTimers());
    const tooltip = () => screen.queryByRole("tooltip");

    it("has no native title, so only our tooltip ever shows", () => {
      const { container } = render(<QuotaStrip quotas={[claude, copilot]} now={NOW} />);
      expect(container.querySelector("[title]")).toBeNull();
    });

    it("shows on hover only after the delay, and hides on leave", () => {
      vi.useFakeTimers();
      const { container } = render(<QuotaStrip quotas={[claude]} now={NOW} />);
      const chip = container.querySelector(".quota-chip")!;
      fireEvent.mouseEnter(chip);
      act(() => { vi.advanceTimersByTime(HOVER_DELAY_MS - 1); });
      expect(tooltip()).toBeNull();
      act(() => { vi.advanceTimersByTime(1); });
      expect(tooltip()).toHaveTextContent(/5h: 42% used/);
      fireEvent.mouseLeave(chip);
      expect(tooltip()).toBeNull();
    });

    it("shows at once on click and closes on a second click, an outside click, or Escape", () => {
      const { container } = render(<div><span data-testid="outside" /><QuotaStrip quotas={[claude, copilot]} now={NOW} /></div>);
      const [chipA, chipB] = [...container.querySelectorAll(".quota-chip")];
      fireEvent.click(chipA);
      expect(tooltip()?.textContent).toBe(quotaTitle(claude, nowSecs));
      fireEvent.click(chipA);
      expect(tooltip()).toBeNull();

      fireEvent.click(chipA);
      fireEvent.click(chipB); // another chip swaps it
      expect(tooltip()?.textContent).toBe(quotaTitle(copilot, nowSecs));
      fireEvent.mouseDown(screen.getByTestId("outside"));
      expect(tooltip()).toBeNull();

      fireEvent.click(chipA);
      fireEvent.keyDown(document, { key: "Escape" });
      expect(tooltip()).toBeNull();
    });

    it("opens just above the cursor, clamped inside the window", () => {
      const { container } = render(<QuotaStrip quotas={[claude]} now={NOW} />);
      const chip = container.querySelector(".quota-chip")!;
      fireEvent.click(chip, { clientX: 300, clientY: 10, detail: 1 });
      const tip = tooltip()!;
      // jsdom lays out nothing (zero-size strip and tooltip), so the cursor
      // maps straight through: centered on x, bottom edge 8px above y.
      expect(tip.style.left).toBe("300px");
      expect(tip.style.bottom).toBe("-2px");
      fireEvent.click(chip);
      fireEvent.click(chip, { clientX: 2, clientY: 10, detail: 1 });
      expect(tooltip()!.style.left).toBe("8px");
    });

    it("keeps a pinned tooltip open while the pointer leaves", () => {
      const { container } = render(<QuotaStrip quotas={[claude]} now={NOW} />);
      const chip = container.querySelector(".quota-chip")!;
      fireEvent.mouseEnter(chip);
      fireEvent.click(chip);
      fireEvent.mouseLeave(chip);
      expect(tooltip()).not.toBeNull();
    });
  });
});
