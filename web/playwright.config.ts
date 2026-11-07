import { defineConfig } from "@playwright/test";

// AURORA_E2E_BASE_URL: run against an already-running stack (scripts/e2e/m1.sh --kind: the kind dev stack).
const external = process.env.AURORA_E2E_BASE_URL;

export default defineConfig({
  testDir: "./e2e",
  timeout: 90_000,
  use: { baseURL: external ?? "http://localhost:4100" },
  webServer: external
    ? undefined
    : {
        command: "pnpm dev",
        url: "http://localhost:4100",
        reuseExistingServer: true,
      },
});
