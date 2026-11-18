import { fireEvent, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { wrap } from "./support/render";
import { json } from "./support/api";

vi.mock("next/navigation", () => ({
  useSearchParams: () => new URLSearchParams(""),
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  useParams: () => ({}),
  usePathname: () => "/",
}));

import { AgentChannelsTab } from "@/components/channels/agent-channels-tab";

function mockFetch(patch: (body: unknown) => Response) {
  const calls: { key: string; body: unknown }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request) => {
      const url = new URL(input.url, "http://localhost");
      const key = `${input.method} ${url.pathname}`;
      const text = input.method === "GET" ? "" : await input.clone().text();
      const body = text ? (JSON.parse(text) as unknown) : null;
      calls.push({ key, body });
      if (key === "PATCH /v1/agents/a1") return patch(body);
      if (input.method === "GET") return json(200, { items: [] });
      return json(404, { error: { code: "not_found", message: "not_found" } });
    }),
  );
  return calls;
}

describe("Show progress in chat (P4-NARR, C-852)", () => {
  it("is off by default and sends narration_cards on change", async () => {
    const calls = mockFetch(() => json(200, {}));
    wrap(<AgentChannelsTab agentId="a1" orgId="o1" canOwn wsId="w1" />);
    const box = (await screen.findByLabelText("Show progress in chat")) as HTMLInputElement;
    expect(box.checked).toBe(false);
    fireEvent.click(box);
    await waitFor(() => expect(calls.some((c) => c.key === "PATCH /v1/agents/a1")).toBe(true));
    expect(calls.find((c) => c.key === "PATCH /v1/agents/a1")?.body).toEqual({ narration_cards: true });
  });

  it("shows the saved value and the help text", async () => {
    mockFetch(() => json(200, {}));
    wrap(<AgentChannelsTab agentId="a1" orgId="o1" canOwn wsId="w1" narrationCards />);
    const box = (await screen.findByLabelText("Show progress in chat")) as HTMLInputElement;
    expect(box.checked).toBe(true);
    expect(screen.getByText(/In shared channels the card shows step names, not details/)).toBeTruthy();
  });

  it("shows an inline error when the save fails", async () => {
    mockFetch(() => json(403, { error: { code: "forbidden", message: "Only an owner can change this." } }));
    wrap(<AgentChannelsTab agentId="a1" orgId="o1" canOwn wsId="w1" />);
    fireEvent.click(await screen.findByLabelText("Show progress in chat"));
    const alert = await screen.findByRole("alert");
    expect(alert.textContent?.length).toBeGreaterThan(0);
  });

  it("is read-only for a caller who cannot own the agent", async () => {
    mockFetch(() => json(200, {}));
    wrap(<AgentChannelsTab agentId="a1" orgId="o1" canOwn={false} wsId="w1" />);
    const box = (await screen.findByLabelText("Show progress in chat")) as HTMLInputElement;
    expect(box.disabled).toBe(true);
  });
});
