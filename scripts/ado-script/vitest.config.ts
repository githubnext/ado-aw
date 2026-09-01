import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    // Default suite covers source-side tests under src/. The smoke test
    // under test/ depends on gate.js/import.js existing, so it runs via a
    // separate config — see vitest.config.smoke.ts and `npm run test:smoke`.
    include: ["src/**/*.test.ts"],

    // Vitest's 5s/10s defaults are too tight for this workspace, and the
    // failure mode is a confusing cascade rather than an honest timeout.
    //
    // Several suites call `await import("../index.js")` *inside* the test
    // body, so Vite's on-demand transform of that module's whole dependency
    // graph is charged to the test's budget. That transform cost is
    // infrastructure work, not test work, and it varies by more than an order
    // of magnitude with cache state and machine load — measured between ~8s
    // and ~158s across the suite on the same machine.
    //
    // When such a test overran 5s, Vitest failed it but did *not* cancel the
    // still-running `main()`. The abandoned call kept consuming shared mock
    // state — including a `mockResolvedValueOnce` queued by the *next* test —
    // so the following test failed with a bogus assertion error that pointed
    // nowhere near the real cause. Budget for the transform instead; a genuine
    // hang still fails, just 30s later.
    testTimeout: 30_000,
    hookTimeout: 30_000,
  },
});

