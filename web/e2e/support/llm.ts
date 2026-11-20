import type { Locator, Page } from "@playwright/test";

/**
 * (P5-REFINE-13, access-9) The AI key form asks "Which service is this key for?" with one card per service, then Next.
 * Picks the service by the name on its card and goes on to the key. `root` holds the cards and the Next button: the page,
 * the dialog, or the guide's inline form.
 */
export async function chooseService(root: Page | Locator, name: string | RegExp) {
  const card = root.getByRole("radio", { name: typeof name === "string" ? new RegExp(`^${name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}`) : name });
  // The radio is visually hidden (the card is its label): check it without a pointer.
  await card.check({ force: true });
  await root.getByRole("button", { name: "Next", exact: true }).click();
}
