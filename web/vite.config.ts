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
    environmentMatchGlobs: [["src/components/**", "jsdom"]],
    setupFiles: ["./src/components/test-setup.ts"],
  },
});
