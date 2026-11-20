import { fireEvent, screen } from "@testing-library/react";

/**
 * (P5-REFINE-13, access-9) The AI key form asks "Which service is this key for?" with one card per service, then Next:
 * picks the service by the name on its card and goes on to the key.
 */
export async function chooseService(name: string) {
  const escaped = name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  fireEvent.click(await screen.findByRole("radio", { name: new RegExp(`^${escaped}`) }));
  fireEvent.click(screen.getByRole("button", { name: "Next" }));
}
