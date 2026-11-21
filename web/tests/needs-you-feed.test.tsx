import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { NeedsYou, sinceWords } from "@/components/overview/needs-you";
import { json } from "./support/api";

// P5-REFINE-16 item 11 (D-417, C-1080/C-1082): the Overview's Needs you card reads the workspace attention feed, and its
// heading number is the number on the rail's Overview entry (the shared count hook).

vi.mock("next/navigation", () => ({
  usePathname: () => "/o/acme/w/main",
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  useSearchParams: () => new URLSearchParams(),
  useParams: () => ({}),
}));

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const item = (over: Record<string, unknown>) => ({
  id: "x",
  kind: "approval",
  level: "workspace",
  topic: "approvals",
  subject_id: "s",
  title: "Dani is waiting for your approval",
  detail: "Close issue #214 on GitHub",
  since: new Date(Date.now() - 11 * 60_000).toISOString(),
  href: "/o/acme/w/main/approvals",
  decision: true,
  actors: [],
  actors_total: 0,
  reason: null,
  own: false,
  action: { label: "Review", href: "/o/acme/w/main/approvals" },
  can_decline: false,
  state: "open",
  closed_at: null,
  message: null,
  closed_by_role: null,
  closed_by: null,
  ...over,
});

function mockApi(items: unknown[], count: number) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const req = input as Request;
      const url = new URL(req.url, "http://localhost");
      if (url.pathname === "/v1/workspaces/ws1/attention") return json(200, { items, count });
      if (url.pathname === "/v1/attention/counts") return json(200, { workspace: { workspace_id: "ws1", count, topics: {} } });
      return json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
}
const wrap = () =>
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <NeedsYou wsId="ws1" base="/o/acme/w/main" />
    </QueryClientProvider>,
  );

describe("Needs you reads the workspace feed", () => {
  it("shows one row per item with its sentence, detail, how long it waited and its one action; the heading says the rail's number", async () => {
    mockApi(
      [
        item({ id: "approval:1" }),
        item({
          id: "broken_account:c1",
          kind: "broken_account",
          topic: "accounts",
          title: "GitHub account needs you",
          detail: "the sign-in expired",
          action: { label: "Sign in again", href: "/o/acme/w/main/apps?tab=accounts&reconnect=c1" },
          decision: false,
        }),
        item({ id: "r1", kind: "my_request", topic: "apps", own: true, title: "You asked for Jira", detail: "Waiting for an organization admin.", action: null, decision: false }),
      ],
      2,
    );
    wrap();
    expect(await screen.findByText("Dani is waiting for your approval")).toBeTruthy();
    expect(await screen.findByLabelText("2 waiting")).toBeTruthy();
    expect(screen.getByText(/Close issue #214 on GitHub/)).toBeTruthy();
    expect(screen.getAllByText(/Since 11 min ago/).length).toBeGreaterThan(0);
    expect(screen.getByRole("link", { name: "Sign in again" }).getAttribute("href")).toBe("/o/acme/w/main/apps?tab=accounts&reconnect=c1");
    // The viewer's own open request is listed, has no action, and is not in the number.
    expect(screen.getByText("You asked for Jira")).toBeTruthy();
    expect(screen.getByRole("link", { name: "Review all approvals" }).getAttribute("href")).toBe("/o/acme/w/main/approvals");
  });

  it("an agent that can't answer is a row with its one action, counted in the heading (C-1080 agent_cannot_answer)", async () => {
    mockApi(
      [
        item({
          id: "agent_cannot_answer:a1",
          kind: "agent_cannot_answer",
          topic: "agents",
          title: "Daneel can't answer: no AI key",
          detail: "Give it an AI key so it can reply.",
          action: { label: "Fix", href: "/o/acme/w/main/agents/a1?tab=access" },
          decision: false,
        }),
      ],
      1,
    );
    wrap();
    expect(await screen.findByText("Daneel can't answer: no AI key")).toBeTruthy();
    expect(screen.getByText(/Give it an AI key so it can reply/)).toBeTruthy();
    expect(screen.getByRole("link", { name: "Fix" }).getAttribute("href")).toBe("/o/acme/w/main/agents/a1?tab=access");
    expect(await screen.findByLabelText("1 waiting")).toBeTruthy();
  });

  it("is absent when nothing waits", async () => {
    mockApi([], 0);
    const { container } = wrap();
    await new Promise((r) => setTimeout(r, 50));
    expect(container.querySelector("section")).toBeNull();
  });
});

describe("a time inside a sentence (batch 5 item 8)", () => {
  it("says 'Since just now.', never 'Since Just now.'", () => {
    expect(sinceWords(new Date().toISOString())).toBe("just now");
    expect(sinceWords(new Date(Date.now() - 11 * 60_000).toISOString())).toBe("11 min ago");
  });
});
