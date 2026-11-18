import AxeBuilder from "@axe-core/playwright";
import type { Page } from "@playwright/test";

/** The axe violations of the page that are serious or critical, one readable line each. */
export const serious = async (page: Page) => {
  const { violations } = await new AxeBuilder({ page }).analyze();
  return violations
    .filter((v) => v.impact === "serious" || v.impact === "critical")
    .map((v) => `${v.id} (${v.impact}): ${v.nodes.map((n) => n.target.join(" ")).slice(0, 5).join(" | ")}`);
};
