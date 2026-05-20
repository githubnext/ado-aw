import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    // Smoke-test config: targets bundled ado-script programs end-to-end.
    // The suite must run AFTER `npm run build` produces gate.js/import.js.
    include: ["test/**/*.test.ts"],
  },
});
