import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    // Smoke-test config: targets the bundled gate.js end-to-end. The
    // suite must run AFTER `npm run build` produces dist/gate/index.js.
    include: ["test/**/*.test.ts"],
  },
});
