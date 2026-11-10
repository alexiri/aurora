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

export default defineConfig({
  testDir: "./e2e",
  timeout: 90_000,
  use: { baseURL: external ?? webUrl },
  webServer: external
    ? undefined
    : {
        command: `pnpm exec next dev -p ${port}`,
        url: webUrl,
        reuseExistingServer: process.env.AURORA_E2E_REUSE === "1",
      },
});
