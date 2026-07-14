import { useEffect, useRef } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";

// Subscribe to a Tauri window event for the lifetime of the component, always
// calling the latest handler without re-subscribing when it changes (the handler
// is read through a ref). Collapses the repeated
// `const un = win.listen(...); return () => un.then(f => f())` blocks.
export function useTauriListen<T>(event: string, handler: (payload: T) => void) {
  const ref = useRef(handler);
  ref.current = handler;
  useEffect(() => {
    const unlisten = getCurrentWindow().listen<T>(event, (e) => ref.current(e.payload));
    return () => { unlisten.then((f) => f()); };
  }, [event]);
}

// Hide (rather than close) the current window when the user hits its close
// button, keeping the webview alive — the behavior the Settings and Logs windows
// share so reopening is instant and state survives.
export function useHideOnClose() {
  useEffect(() => {
    const win = getCurrentWindow();
    const unlisten = win.onCloseRequested((event) => {
      event.preventDefault();
      win.hide();
    });
    return () => { unlisten.then((f) => f()); };
  }, []);
}
