import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
    outDir: "dist",
  },
  test: {
    environment: "node",
    // Bound simultaneous jsdom apps so full-suite runs do not starve interaction tests.
    maxWorkers: 2,
    environmentMatchGlobs: [["src/components/**", "jsdom"]],
    setupFiles: ["./src/components/test-setup.ts"],
  },
});
