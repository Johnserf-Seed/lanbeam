import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// Frontend unit/component tests. Kept separate from vite.config.ts so the
// Tauri dev/build pipeline is untouched. Tests run in happy-dom (fast DOM);
// the Tauri bridge (src/bridge/api.ts) detects non-Tauri environments and
// serves its browser-mode stubs, so store/lib tests need no IPC mocking —
// component tests that need specific backend replies mock ../bridge/api.
export default defineConfig({
  plugins: [react()],
  test: {
    environment: "happy-dom",
    setupFiles: ["./src/test/setup.ts"],
    include: ["src/**/*.test.{ts,tsx}"],
    coverage: {
      provider: "v8",
      include: ["src/**/*.{ts,tsx}"],
      exclude: [
        "src/**/*.test.{ts,tsx}",
        "src/test/**",
        "src/main.tsx", // bootstrap glue: mounts React onto #root, nothing to unit-test
        "src/vite-env.d.ts",
      ],
      reporter: ["text", "json-summary"],
    },
  },
});
