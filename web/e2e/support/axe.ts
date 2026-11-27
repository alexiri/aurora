import AxeBuilder from "@axe-core/playwright";
import type { Page } from "@playwright/test";

import { titled } from "./open";

/**
 * The axe violations of the page that are serious or critical, one readable line each. It waits for
 * the document to be titled first: right after a client-side navigation the new route's `<title>`
 * may not be in yet. A page that never gets one still fails, at that wait.
 */
export const serious = async (page: Page) => {
  await titled(page);
  const { violations } = await new AxeBuilder({ page }).analyze();
  return violations
    .filter((v) => v.impact === "serious" || v.impact === "critical")
    .map((v) => `${v.id} (${v.impact}): ${v.nodes.map((n) => n.target.join(" ")).slice(0, 5).join(" | ")}`);
};
