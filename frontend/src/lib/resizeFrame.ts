/** A window frame in logical pixels: top-left position plus size. */
export interface Frame {
  x: number;
  y: number;
  width: number;
  height: number;
}

// Tauri's `startResizeDragging` direction. The enum is declared but not exported
// from @tauri-apps/api/window, so we mirror its string union locally.
export type ResizeDirection =
  | "North"
  | "South"
  | "East"
  | "West"
  | "NorthEast"
  | "NorthWest"
  | "SouthEast"
  | "SouthWest";

/**
 * The frame after dragging `direction`'s edge(s) by `(dx, dy)` from `start`.
 * East/South edges grow the size; North/West edges also move the origin so the
 * opposite edge stays pinned. Each axis is clamped to its minimum (which also
 * keeps the pinned edge fixed once the minimum is hit).
 */
export function resizeFrame(
  direction: ResizeDirection,
  start: Frame,
  dx: number,
  dy: number,
  min: { width: number; height: number },
): Frame {
  const frame = { ...start };
  if (direction.includes("East")) {
    frame.width = Math.max(min.width, start.width + dx);
  } else if (direction.includes("West")) {
    frame.width = Math.max(min.width, start.width - dx);
    frame.x = start.x + start.width - frame.width;
  }
  if (direction.includes("South")) {
    frame.height = Math.max(min.height, start.height + dy);
  } else if (direction.includes("North")) {
    frame.height = Math.max(min.height, start.height - dy);
    frame.y = start.y + start.height - frame.height;
  }
  return frame;
}
