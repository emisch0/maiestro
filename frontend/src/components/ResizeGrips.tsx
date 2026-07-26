import React from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";

/**
 * Invisible drag grips along the popover's edges and corners. The `main` window
 * is undecorated and transparent (see tauri.conf.json), so the OS draws no resize
 * border — these thin overlays give the user something to grab, forwarding to
 * Tauri's `startResizeDragging`. Backend persists the resulting size on blur.
 */
// Tauri's `startResizeDragging` direction. The enum is declared but not exported
// from @tauri-apps/api/window, so we mirror its string union locally.
type ResizeDirection =
  | "North"
  | "South"
  | "East"
  | "West"
  | "NorthEast"
  | "NorthWest"
  | "SouthEast"
  | "SouthWest";

export function ResizeGrips() {
  const startResize = (direction: ResizeDirection) => (e: React.PointerEvent) => {
    // Only the primary button starts a resize; ignore right/middle clicks.
    if (e.button !== 0) return;
    e.preventDefault();
    void getCurrentWindow().startResizeDragging(direction);
  };
  const grips: { cls: string; dir: ResizeDirection }[] = [
    { cls: "resize-grip--n", dir: "North" },
    { cls: "resize-grip--s", dir: "South" },
    { cls: "resize-grip--e", dir: "East" },
    { cls: "resize-grip--w", dir: "West" },
    { cls: "resize-grip--ne", dir: "NorthEast" },
    { cls: "resize-grip--nw", dir: "NorthWest" },
    { cls: "resize-grip--se", dir: "SouthEast" },
    { cls: "resize-grip--sw", dir: "SouthWest" },
  ];
  return (
    <>
      {grips.map((g) => (
        <div
          key={g.cls}
          className={`resize-grip ${g.cls}`}
          aria-hidden="true"
          onPointerDown={startResize(g.dir)}
        />
      ))}
    </>
  );
}
