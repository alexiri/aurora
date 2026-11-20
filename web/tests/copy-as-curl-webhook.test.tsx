import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { AutomationsTab } from "@/components/automations/automations-tab";
import { json, mockFetch } from "./support/api";
import { wrap } from "./support/render";

// P5-REFINE-19 item 5, webhook cards: Copy as curl is Expert and its command carries $SECRET, never a secret.

let copied: string[] = [];
beforeEach(() => {
  copied = [];
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText: vi.fn(async (t: string) => void copied.push(t)) } });
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const hook = {
  id: "h1", name: "Deploy notice", url_prefix: "https://aurora.example.org/hooks/abcd1234", verification: { kind: "hmac_sha256", header: "X-Signature", format: "sha256=<hex>" },
  instructions: "Say what shipped.", enabled: true, anchor: "owner", created_by: { id: "u", display_name: "U" }, last_delivery_at: null,
};

describe("Webhook cards", () => {
  function routes() {
    mockFetch({
      "GET /v1/agents/a1/webhooks": () => json(200, { items: [hook], next_cursor: null }),
      "GET /v1/agents/a1/schedules": () => json(200, { items: [], next_cursor: null }),
    });
  }
  const page = <AutomationsTab agentId="a1" agentName="Daneel" agentBase="/o/acme/w/main/agents/daneel" wsId="w1" canOwn />;

  it("copies the command the verification asks for, with $SECRET and no secret", async () => {
    routes();
    wrap(page);
    const card = (await screen.findByText("Deploy notice")).closest("li")!;
    fireEvent.click(within(card).getByRole("button", { name: "Copy as curl" }));
    await waitFor(() => expect(copied).toHaveLength(1));
    expect(copied[0]).toContain('-H "X-Signature: sha256=$SIG"');
    expect(copied[0]).toContain("hooks/abcd1234…");
    expect(copied[0]).toContain('"$SECRET"');
    // The command is the one the How to call it panel shows (one recipe source).
    fireEvent.click(within(card).getByRole("button", { name: "How to call it" }));
    const shown = (await within(card).findByTestId("howto-snippet")).textContent!;
    expect(copied[0]!.endsWith(shown)).toBe(true);
    expect(await within(card).findByRole("button", { name: "Copied" })).toBeInTheDocument();
  });

  it("has a From your terminal fold with the signed call and $SECRET (item 6)", async () => {
    routes();
    wrap(page);
    const fold = await screen.findByTestId("from-your-terminal");
    expect(within(fold).getByLabelText("the curl command")).toHaveTextContent("-hmac \"$SECRET\"");
    expect(within(fold).getByLabelText("the curl command")).toHaveTextContent('"$WEBHOOK_URL"');
  });

  it("has no Copy as curl and no fold with Show more controls off", async () => {
    routes();
    wrap(page, { moreControls: false });
    await screen.findByText("Deploy notice");
    expect(screen.queryByRole("button", { name: "Copy as curl" })).toBeNull();
    expect(screen.queryByTestId("from-your-terminal")).toBeNull();
  });
});
