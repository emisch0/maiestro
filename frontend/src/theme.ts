import { getCurrentWindow } from "@tauri-apps/api/window";
import { api, Theme } from "./api";

/**
 * Apply a theme to this window's document. For an explicit Light/Dark choice we
 * set `data-theme` on `<html>`; for System we remove it so the
 * `prefers-color-scheme` media query in `styles.css` governs (and keeps tracking
 * OS appearance changes at runtime). Each webview window applies independently.
 */
export function applyTheme(theme: Theme): void {
  const root = document.documentElement;
  if (theme === "system") {
    root.removeAttribute("data-theme");
  } else {
    root.setAttribute("data-theme", theme);
  }
}

/**
 * Read the persisted theme, apply it to this window, and subscribe to
 * `theme-changed` (broadcast by the backend when the Appearance setting changes)
 * so every open window stays in sync live. Returns an unlisten function.
 */
export async function initTheme(): Promise<() => void> {
  try {
    applyTheme(await api.getTheme());
  } catch {
    // Fall back to the dark baseline if the setting can't be read.
  }
  return getCurrentWindow().listen<Theme>("theme-changed", (e) => {
    applyTheme(e.payload);
  });
}
