import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import svgr from "vite-plugin-svgr";

// The web app lives in frontend/; Tauri loads it from frontend/dist (build)
// or the fixed dev-server port (dev).
export default defineConfig({
  root: "frontend",
  // svgr lets us import `*.svg?react` as React components. svgo is disabled so
  // icon markup (currentColor, baked PR-state fills, viewBox) passes through
  // verbatim — what's in the file is what renders.
  plugins: [react(), svgr({ svgrOptions: { svgo: false } })],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
  },
});
