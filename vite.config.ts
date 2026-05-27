import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// The web app lives in frontend/; Tauri loads it from frontend/dist (build)
// or the fixed dev-server port (dev).
export default defineConfig({
  root: "frontend",
  plugins: [react()],
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
