// Vitest global setup, run once before each test file (see vite.config.ts
// `test.setupFiles`).
//
// - Registers @testing-library/jest-dom matchers (toBeInTheDocument, …).
// - Clears mocked Tauri IPC after every test so a `mockIPC` from one test can't
//   leak into the next. Component/api tests fake the backend per-test with
//   `mockIPC` from `@tauri-apps/api/mocks`.
import "@testing-library/jest-dom/vitest";
import { afterEach } from "vitest";
import { cleanup } from "@testing-library/react";
import { clearMocks } from "@tauri-apps/api/mocks";

afterEach(() => {
  cleanup();
  clearMocks();
});
