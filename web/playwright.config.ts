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

export default defineConfig({
  testDir: "./e2e",
  timeout: 90_000,
  // See the parallelism notes above the config.
  fullyParallel: true,
  workers: process.env.AURORA_E2E_WORKERS ? Number(process.env.AURORA_E2E_WORKERS) : defaultWorkers,
  // VTL-001 measurement: each worker's first test pays the per-worker Next.js dev-server compile
  // of every route it opens (~30 s, cached in the dev server after that). With 4 workers that
  // first wave dominates short files, so a single slow first test is expected; retries stay at
  // Playwright's flake-masking default for the mock suites. Strict single-run mode (what the
  // "no retries" policy means here): AURORA_E2E_RETRIES=0.
  retries: process.env.AURORA_E2E_RETRIES ? Number(process.env.AURORA_E2E_RETRIES) : undefined,
  use: { baseURL: external ?? webUrl },
  webServer: external
    ? undefined
    : {
        command: `pnpm exec next dev -p ${port}`,
        url: webUrl,
        reuseExistingServer: process.env.AURORA_E2E_REUSE === "1",
      },
});
