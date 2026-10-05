import { describe, it, expect } from "vitest";
import { resizeFrame, type ResizeDirection } from "./resizeFrame";

const START = { x: 100, y: 50, width: 680, height: 460 };
const MIN = { width: 480, height: 360 };

describe("resizeFrame", () => {
  const cases: [ResizeDirection, { x: number; y: number; width: number; height: number }][] = [
    ["East", { x: 100, y: 50, width: 690, height: 460 }],
    ["West", { x: 110, y: 50, width: 670, height: 460 }],
    ["South", { x: 100, y: 50, width: 680, height: 480 }],
    ["North", { x: 100, y: 70, width: 680, height: 440 }],
    ["SouthEast", { x: 100, y: 50, width: 690, height: 480 }],
    ["NorthEast", { x: 100, y: 70, width: 690, height: 440 }],
    ["SouthWest", { x: 110, y: 50, width: 670, height: 480 }],
    ["NorthWest", { x: 110, y: 70, width: 670, height: 440 }],
  ];

  it.each(cases)("drags the %s edge by (+10, +20)", (dir, expected) => {
    expect(resizeFrame(dir, START, 10, 20, MIN)).toEqual(expected);
  });

  it("clamps east/south growth to the minimum size", () => {
    expect(resizeFrame("SouthEast", START, -1000, -1000, MIN)).toEqual({
      x: 100,
      y: 50,
      width: 480,
      height: 360,
    });
  });

  it("keeps the opposite edge pinned when north/west hit the minimum", () => {
    const f = resizeFrame("NorthWest", START, 1000, 1000, MIN);
    expect(f.width).toBe(480);
    expect(f.height).toBe(360);
    expect(f.x + f.width).toBe(START.x + START.width);
    expect(f.y + f.height).toBe(START.y + START.height);
  });

  it("does not touch the axis a direction doesn't drag", () => {
    expect(resizeFrame("East", START, 10, 999, MIN)).toEqual({ ...START, width: 690 });
    expect(resizeFrame("North", START, 999, -10, MIN)).toEqual({
      ...START,
      y: 40,
      height: 470,
    });
  });
});
