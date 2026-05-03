import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    // Default suite covers source-side tests under src/. The smoke test
    // under test/ depends on dist/gate/index.js existing, so it runs via
    // a separate config — see vitest.config.smoke.ts and
    // `npm run test:smoke`.
    include: ["src/**/*.test.ts"],
  },
});


