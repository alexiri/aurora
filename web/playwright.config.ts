import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./e2e",
  timeout: 90_000,
  use: { baseURL: "http://localhost:4100" },
  webServer: {
    command: "pnpm dev",
    url: "http://localhost:4100",
    reuseExistingServer: true,
  },
});
