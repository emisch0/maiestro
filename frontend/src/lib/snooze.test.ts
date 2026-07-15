import { describe, it, expect } from "vitest";
import { effectiveHidden, formatSnoozeRemaining } from "./snooze";

const NOW = 1_700_000_000_000;

describe("effectiveHidden", () => {
  it("treats no hide state as visible", () => {
    expect(effectiveHidden(null, NOW)).toBe(false);
    expect(effectiveHidden(undefined, NOW)).toBe(false);
  });

  it("treats an indefinite hide (no snooze) as hidden", () => {
    expect(effectiveHidden({ snooze_until: null }, NOW)).toBe(true);
  });

  it("hides while the snooze deadline is still in the future", () => {
    expect(effectiveHidden({ snooze_until: NOW + 60_000 }, NOW)).toBe(true);
  });

  it("reads an expired snooze as visible (auto un-snooze at render time)", () => {
    expect(effectiveHidden({ snooze_until: NOW - 1 }, NOW)).toBe(false);
    // Exactly at the deadline is no longer in the future → visible.
    expect(effectiveHidden({ snooze_until: NOW }, NOW)).toBe(false);
  });
});

describe("formatSnoozeRemaining", () => {
  it("returns empty once expired", () => {
    expect(formatSnoozeRemaining(NOW, NOW)).toBe("");
    expect(formatSnoozeRemaining(NOW - 5000, NOW)).toBe("");
  });

  it("shows minutes under an hour, flooring at 1m", () => {
    expect(formatSnoozeRemaining(NOW + 30_000, NOW)).toBe("1m"); // <1min rounds up to 1
    expect(formatSnoozeRemaining(NOW + 5 * 60_000, NOW)).toBe("5m");
    expect(formatSnoozeRemaining(NOW + 59 * 60_000, NOW)).toBe("59m");
  });

  it("shows whole hours under a day", () => {
    expect(formatSnoozeRemaining(NOW + 60 * 60_000, NOW)).toBe("1h");
    expect(formatSnoozeRemaining(NOW + 23 * 60 * 60_000, NOW)).toBe("23h");
  });

  it("shows days plus remaining hours past a day", () => {
    const h = 60 * 60_000;
    expect(formatSnoozeRemaining(NOW + 24 * h, NOW)).toBe("1d");
    expect(formatSnoozeRemaining(NOW + 25 * h, NOW)).toBe("1d 1h");
    expect(formatSnoozeRemaining(NOW + (2 * 24 + 3) * h, NOW)).toBe("2d 3h");
  });
});
