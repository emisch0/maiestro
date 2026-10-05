import { ComponentType, lazy, Suspense, useEffect } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { initTheme } from "./theme";
import { MainView } from "./MainView";

// The non-popover windows load their code on demand, so the popover (opened
// most often) doesn't parse them, and Settings' JSON Forms + ajv in particular.
// MainView stays a static import so the popover never waits on a chunk.
const Settings = lazy(() => import("./Settings").then((m) => ({ default: m.Settings })));
const LogsView = lazy(() => import("./LogsView").then((m) => ({ default: m.LogsView })));
const Onboarding = lazy(() => import("./Onboarding").then((m) => ({ default: m.Onboarding })));
const LAZY_VIEWS: Record<string, ComponentType | undefined> = {
  settings: Settings,
  logs: LogsView,
  onboarding: Onboarding,
};

export default function App() {
  // Apply the persisted theme to this window and keep it in sync with the
  // backend's `theme-changed` broadcast. Runs in every window (popover,
  // settings, logs) since each is a separate webview rendering this bundle.
  useEffect(() => {
    const unlisten = initTheme();
    return () => { unlisten.then((f) => f()); };
  }, []);

  // No Suspense fallback: the chunk loads from local disk, so a loader would
  // only flash.
  const View = LAZY_VIEWS[getCurrentWindow().label];
  if (!View) return <MainView />;
  return <Suspense fallback={null}><View /></Suspense>;
}
