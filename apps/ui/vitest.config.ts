import { defineConfig } from "vitest/config";

export default defineConfig({
  resolve: { alias: { "@": import.meta.dirname } },
  // globals.css is served to tests as its own text (test/skins.test.ts imports it with ?raw); other CSS stays empty.
  test: { include: ["test/**/*.test.ts", "test/**/*.test.tsx"], environment: "node", css: { include: [/globals\.css/] } },
});
