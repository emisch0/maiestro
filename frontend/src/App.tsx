import { useEffect } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { initTheme } from "./theme";
import { MainView } from "./MainView";
import { Settings } from "./Settings";
import { LogsView } from "./LogsView";

export default function App() {
  // Apply the persisted theme to this window and keep it in sync with the
  // backend's `theme-changed` broadcast. Runs in every window (popover,
  // settings, logs) since each is a separate webview rendering this bundle.
  useEffect(() => {
    const unlisten = initTheme();
    return () => { unlisten.then((f) => f()); };
  }, []);

  const label = getCurrentWindow().label;
  if (label === "settings") return <Settings />;
  if (label === "logs") return <LogsView />;
  return <MainView />;
}
