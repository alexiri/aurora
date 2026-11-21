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
  id: "h1", name: "Deploy notice", url_prefix: "auw_9xLPEkbX", verification: { kind: "hmac_sha256", header: "X-Signature", format: "sha256=<hex>" },
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
    expect(copied[0]).toContain('curl -X POST "$WEBHOOK_URL" ');
    expect(copied[0]).toContain(`# WEBHOOK_URL is the address shown when you created it: ${window.location.origin}/v1/hooks/auw_9xLPEkbX…`);
    expect(copied[0]).toContain('"$SECRET"');
    // The command is exactly the one the How to call it panel shows (one generator).
    fireEvent.click(within(card).getByRole("button", { name: "How to call it" }));
    const shown = (await within(card).findByTestId("howto-snippet")).textContent!;
    expect(copied[0]).toBe(shown);
    expect(await within(card).findByRole("button", { name: "Copied" })).toBeInTheDocument();
  });

  it("has no From your terminal box on the list: the command is on the card", async () => {
    routes();
    wrap(page);
    await screen.findByText("Deploy notice");
    expect(screen.queryByTestId("from-your-terminal")).toBeNull();
    expect(screen.queryByText("From your terminal")).toBeNull();
  });

  it("has no Copy as curl and no fold with Show more controls off", async () => {
    routes();
    wrap(page, { moreControls: false });
    await screen.findByText("Deploy notice");
    expect(screen.queryByRole("button", { name: "Copy as curl" })).toBeNull();
    expect(screen.queryByTestId("from-your-terminal")).toBeNull();
  });
});
