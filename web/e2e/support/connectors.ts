import type { Page } from "@playwright/test";

import { openRoute, type Scheme } from "./open";

// P4-MCON-3: the in-page screens of Admin → Connectors, reached by clicking through the real UI
// with the mocked API (data: the real built-in connector, Mattermost).
export const CONNECTOR_SCREENS = ["list", "add-choose", "add-review", "add-settings", "detail", "self-hosted"] as const;
export type ConnectorScreen = (typeof CONNECTOR_SCREENS)[number];

export async function openConnectorScreen(page: Page, screen: ConnectorScreen, scheme: Scheme) {
  await openRoute(page, "/admin/connectors", scheme);
  if (screen === "list") return;
  if (screen === "detail") {
    await page.getByRole("button", { name: "Manage Mattermost" }).click();
    await page.getByLabel("Callback URL").waitFor();
    return;
  }
  await page.getByRole("button", { name: "Add connector" }).click();
  if (screen === "self-hosted") {
    await page.getByRole("button", { name: "Self-hosted connector (advanced)" }).click();
    await page.getByLabel("Connector id").waitFor();
    return;
  }
  if (screen === "add-choose") {
    await page.getByRole("list", { name: "Connector types" }).waitFor();
    return;
  }
  await page.getByRole("button", { name: "Add Mattermost" }).click();
  await page.getByTestId("manifest-summary").waitFor();
  if (screen === "add-review") return;
  await page.getByRole("button", { name: "Continue" }).click();
  await page.getByLabel("Callback URL").waitFor();
}
