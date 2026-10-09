import React, { useEffect, useState } from "react";
import { getCurrentWindow, LogicalPosition, LogicalSize } from "@tauri-apps/api/window";
import { api, type Platform } from "../api";
import { resizeFrame, type Frame, type ResizeDirection } from "../lib/resizeFrame";

/**
 * Invisible drag grips along the popover's edges and corners. The `main` window
 * is undecorated and transparent (see tauri.conf.json), so the OS draws no resize
 * border — these thin overlays give the user something to grab. On Windows they
 * forward to Tauri's `startResizeDragging`; on macOS tao doesn't implement that
 * (it returns `NotSupported`), so the grip drives the resize itself from pointer
 * moves (`dragResize`). Backend persists the resulting size on blur.
 */

// The popover's minimum size — mirrors `minWidth`/`minHeight` of the `main`
// window in tauri.conf.json (and `MIN_POPOVER_*` in main.rs).
const MIN_SIZE = { width: 480, height: 360 };

/**
 * Resize the window by following the pointer until it is released. The frame is
 * computed from the pointer's screen-coordinate offset since pointerdown (screen
 * coordinates don't shift as the window moves under the cursor), and window
 * updates are coalesced: at most one set-size/set-position round trip is in
 * flight, always applying the latest pointer position.
 */
function dragResize(direction: ResizeDirection, e: React.PointerEvent<HTMLElement>) {
  const el = e.currentTarget;
  const pointerId = e.pointerId;
  const startX = e.screenX;
  const startY = e.screenY;
  el.setPointerCapture(pointerId);

  const win = getCurrentWindow();
  let latest = { dx: 0, dy: 0 };
  let start: Frame | null = null;
  let dirty = false;
  let busy = false;

  const pump = async () => {
    if (busy || !start) return;
    busy = true;
    try {
      while (dirty) {
        dirty = false;
        const f = resizeFrame(direction, start, latest.dx, latest.dy, MIN_SIZE);
        if (f.x !== start.x || f.y !== start.y) {
          await win.setPosition(new LogicalPosition(f.x, f.y));
        }
        await win.setSize(new LogicalSize(f.width, f.height));
      }
    } catch (err) {
      console.error("popover resize failed", err);
    } finally {
      busy = false;
    }
  };

  const onMove = (ev: PointerEvent) => {
    if (ev.pointerId !== pointerId) return;
    latest = { dx: ev.screenX - startX, dy: ev.screenY - startY };
    dirty = true;
    void pump();
  };
  const onEnd = (ev: PointerEvent) => {
    if (ev.pointerId !== pointerId) return;
    el.removeEventListener("pointermove", onMove);
    el.removeEventListener("pointerup", onEnd);
    el.removeEventListener("pointercancel", onEnd);
    if (el.hasPointerCapture(pointerId)) el.releasePointerCapture(pointerId);
  };
  el.addEventListener("pointermove", onMove);
  el.addEventListener("pointerup", onEnd);
  el.addEventListener("pointercancel", onEnd);

  // The starting frame needs a few async reads; moves that arrive meanwhile are
  // recorded in `latest` and applied as soon as it is known.
  void (async () => {
    const scale = await win.scaleFactor();
    const pos = (await win.outerPosition()).toLogical(scale);
    const size = (await win.outerSize()).toLogical(scale);
    start = { x: pos.x, y: pos.y, width: size.width, height: size.height };
    void pump();
  })().catch((err) => console.error("popover resize failed", err));
}

export function ResizeGrips() {
  const [platform, setPlatform] = useState<Platform | null>(null);
  useEffect(() => {
    api.platform().then(setPlatform, () => {});
  }, []);

  const startResize = (direction: ResizeDirection) => (e: React.PointerEvent<HTMLElement>) => {
    // Only the primary button starts a resize; ignore right/middle clicks.
    if (e.button !== 0) return;
    e.preventDefault();
    if (platform === "macos") {
      dragResize(direction, e);
    } else {
      void getCurrentWindow().startResizeDragging(direction);
    }
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
