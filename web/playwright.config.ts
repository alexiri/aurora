import { defineConfig } from "@playwright/test";

// AURORA_E2E_BASE_URL: run against an already-running stack (scripts/e2e/m1.sh --kind: the kind dev stack).
const external = process.env.AURORA_E2E_BASE_URL;

// AURORA_E2E_WEB_PORT (default 4100): the port of the dev server Playwright starts from THIS checkout.
// Several checkouts or lanes may share a machine: give each its own port.
//
// A server that is already listening on that port is never silently reused (it may belong to another
// checkout, and the suite would test the wrong code): Playwright then fails fast with "... is already
// used". Reuse is opt-in with AURORA_E2E_REUSE=1, for a harness that started the dev server of this
// checkout itself (scripts/e2e/m1.sh).
const port = Number(process.env.AURORA_E2E_WEB_PORT ?? "4100");
if (!Number.isInteger(port) || port < 1 || port > 65535) {
  throw new Error(`AURORA_E2E_WEB_PORT must be a TCP port, got "${process.env.AURORA_E2E_WEB_PORT}"`);
}
const webUrl = `http://localhost:${port}`;

// VTL-001: per-file parallelism. Every mock-based suite (support/mock-api.ts and friends: each test
// installs its own `page.route()` mocks, no shared server state) is worker-safe. The real-stack
// journeys stay one test per file, and onboarding-journeys.spec.ts — which mutates the shared
// instance's placement — declares `test.describe.configure({ mode: "serial" })` in-file, so
// parallel workers never reorder its steps. Run the real-stack specs through their scripts
// (scripts/e2e/first-run.sh, m1.sh), one file at a time.
// Default workers: Playwright's own default is "half the cores", which oversubscribes a shared
// 16-core dev host running sibling lanes. The server under test is the single-process `next dev`,
// which the workers share, so 4 is the measured sweet spot. Override: AURORA_E2E_WORKERS.
const defaultWorkers = 4;

// FIX-067: the real-stack specs share ONE kind stack (instance, placement, users) and must never run
// next to each other. They live in their own project with a single worker and no file parallelism;
// every other (mocked) spec stays in the parallel "mocked" project. The real-stack project is
// selected by `--project real-stack` (the scripts/e2e/*.sh wrappers pass spec paths, which match it).
// The project's own `workers: 1` (a per-project cap under the global limit) keeps its files from
// running on different workers at the same time.
const REAL_STACK = [/first-run\.spec\.ts$/, /m1\.spec\.ts$/, /no-default-model\.spec\.ts$/, /onboarding-journeys\.spec\.ts$/];

export default defineConfig({
  testDir: "./e2e",
  timeout: 90_000,
  // Per-project parallelism below.
  fullyParallel: true,
  // Locally no retries (a retry hides a flake); CI keeps its own explicit value via AURORA_E2E_RETRIES.
  retries: process.env.AURORA_E2E_RETRIES ? Number(process.env.AURORA_E2E_RETRIES) : 0,
  workers: process.env.AURORA_E2E_WORKERS ? Number(process.env.AURORA_E2E_WORKERS) : defaultWorkers,
  projects: [
    { name: "mocked", testIgnore: REAL_STACK, fullyParallel: true },
    // One worker, files and tests strictly in order: they share one stack.
    { name: "real-stack", testMatch: REAL_STACK, fullyParallel: false, workers: 1 },
  ],
  use: { baseURL: external ?? webUrl },
  webServer: external
    ? undefined
    : {
        command: `pnpm exec next dev -p ${port}`,
        url: webUrl,
        reuseExistingServer: process.env.AURORA_E2E_REUSE === "1",
      },
});
