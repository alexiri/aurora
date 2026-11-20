import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("next/navigation", () => ({
  usePathname: () => "/o/acme/chat-platforms",
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  useSearchParams: () => new URLSearchParams(),
  useParams: () => ({ org: "acme" }),
}));

import { AskAdmin } from "@/components/ask-admin";
import { AgentChannelsTab } from "@/components/channels/agent-channels-tab";
import { OrgChatPlatformsPage } from "@/components/org/org-topics";
import { SetupRequestItem } from "@/components/overview/setup-request-row";
import type { WhoCanValue } from "@/lib/who-can";

const ana = { user_id: "u-ana", name: "Ana Admin" };
const who = { role: "org_admin", action: "connect_chat", people: [ana] } as unknown as WhoCanValue;
const contacts = { owners: [ana], admins: [ana] };
const asker = (name: string, id: string) => ({ user_id: id, display_name: name, workspace_id: "ws1", asked_at: "2026-11-19T10:00:00Z" });
const mineRow = (subject: string, over: Record<string, unknown> = {}) => ({
  id: "r1",
  org_id: "org1",
  subject,
  app_id: "",
  app_name: subject === "ai_key" ? "An AI key" : "A chat platform",
  status: "open",
  askers: [asker("Mia Member", "u-mia")],
  needs: [],
  unavailable: false,
  created_at: "2026-11-19T10:00:00Z",
  closed_at: null,
  ...over,
});

type Call = { method: string; path: string; body: unknown };

/** One fetch router: the viewer's own open requests (changing as they ask and withdraw) and what the pages read. */
function mockApi(opts: { role?: string; mine?: unknown[]; conflict?: boolean } = {}) {
  const calls: Call[] = [];
  let mine = opts.mine ?? [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const req = typeof input === "string" ? new Request(new URL(input, "http://localhost")) : (input as Request);
      const url = new URL(req.url, "http://localhost");
      const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
      const text = req.method === "GET" || req.method === "DELETE" ? "" : await req.clone().text();
      calls.push({ method: req.method, path: url.pathname, body: text ? JSON.parse(text) : null });
      if (url.pathname === "/v1/workspaces/ws1/requests") {
        if (req.method === "GET") return json({ items: mine });
        if (opts.conflict) return json({ error: { code: "already_available", message: "This is already set up." } }, 409);
        const b = JSON.parse(text) as { subject: string };
        mine = [mineRow(b.subject)];
        return json(mine[0]);
      }
      if (url.pathname === "/v1/workspaces/ws1/requests/r1" && req.method === "DELETE") {
        mine = [];
        return new Response(null, { status: 204 });
      }
      if (url.pathname === "/v1/resolve") return json({ org: { id: "org1", slug: "acme", name: "Acme" } });
      if (url.pathname === "/v1/orgs/org1") return json({ id: "org1", slug: "acme", name: "Acme", my_role: opts.role ?? "member", contacts });
      if (url.pathname === "/v1/orgs/org1/workspaces") return json({ items: [{ id: "ws1", name: "Main", slug: "main" }], next_cursor: null });
      if (url.pathname.endsWith("/presence-options") || url.pathname.endsWith("/presences")) return json({ items: [] });
      return json({ error: { code: "not_found", message: "nf" } }, 404);
    }),
  );
  return calls;
}

const wrap = (ui: React.ReactElement) => render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>{ui}</QueryClientProvider>);

afterEach(() => vi.unstubAllGlobals());

describe("Ask <Name> for a chat platform or an AI key (P5-APIGAP-2, C-1047)", () => {
  it("asks with the subject only, then says Asked <Name> and Undo withdraws it", async () => {
    const calls = mockApi();
    wrap(<AskAdmin workspaceId="ws1" subject="ai_key" who={who} />);
    fireEvent.click(await screen.findByRole("button", { name: "Ask an admin for an AI key" }));
    expect(await screen.findByText("Asked the admins. You'll see it here when it's done.")).toBeTruthy();
    expect(calls.find((c) => c.method === "POST")?.body).toEqual({ subject: "ai_key" });
    fireEvent.click(screen.getByRole("button", { name: "Undo" }));
    await waitFor(() => expect(screen.getByRole("button", { name: /ask an admin/i })).toBeTruthy());
    expect(calls.some((c) => c.method === "DELETE" && c.path === "/v1/workspaces/ws1/requests/r1")).toBe(true);
  });

  it("starts in the asked state when the person has asked already", async () => {
    mockApi({ mine: [mineRow("chat_platform")] });
    wrap(<AskAdmin workspaceId="ws1" subject="chat_platform" who={who} />);
    expect(await screen.findByText("Asked the admins. You'll see it here when it's done.")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Undo" })).toBeTruthy();
  });

  it("says why when the thing exists already", async () => {
    mockApi({ conflict: true });
    wrap(<AskAdmin workspaceId="ws1" subject="chat_platform" who={who} />);
    fireEvent.click(await screen.findByRole("button", { name: "Ask an admin for a chat platform" }));
    expect(await screen.findByText(/already set up/i)).toBeTruthy();
  });

  it("an agent's Channels tab with no chat platform offers Ask an admin (never a person's name, D-407)", async () => {
    const calls = mockApi();
    wrap(<AgentChannelsTab agentId="a1" agentName="Bot" orgId="org1" canOwn={false} wsId="ws1" />);
    fireEvent.click(await screen.findByRole("button", { name: "Ask an admin for a chat platform" }));
    await waitFor(() => expect(calls.find((c) => c.method === "POST")?.body).toEqual({ subject: "chat_platform" }));
  });

  it("the organization's chat platforms page gives a member who to ask instead of a refusal", async () => {
    const calls = mockApi();
    wrap(<OrgChatPlatformsPage />);
    expect(await screen.findByTestId("member-chat-platforms")).toBeTruthy();
    fireEvent.click(await screen.findByRole("button", { name: "Ask an admin for a chat platform" }));
    await waitFor(() => expect(calls.find((c) => c.method === "POST")?.body).toEqual({ subject: "chat_platform" }));
  });
});

describe("Needs you rows for the new kinds", () => {
  const row = (over: Record<string, unknown>) =>
    render(
      <QueryClientProvider client={new QueryClient()}>
        <ul>
          <SetupRequestItem request={{ ...mineRow("ai_key"), askers: [asker("Bob Builder", "u1"), asker("Carol", "u2")], ...over } as never} orgBase="/o/acme" orgId="org1" />
        </ul>
      </QueryClientProvider>,
    );

  it("a chat platform: who wants it and Connect a chat platform", () => {
    row({ subject: "chat_platform" });
    expect(screen.getByText("Bob Builder and Carol want a chat platform")).toBeTruthy();
    expect(screen.getByRole("link", { name: "Connect a chat platform" }).getAttribute("href")).toBe("/o/acme/chat-platforms");
    expect(screen.getByRole("button", { name: "Not now" })).toBeTruthy();
  });

  it("a named chat platform reads as its name", () => {
    row({ subject: "chat_platform", connector_type: "mattermost", app_name: "Mattermost" });
    expect(screen.getByText("Bob Builder and Carol want Mattermost")).toBeTruthy();
  });

  it("an AI key: Add an AI key", () => {
    row({ subject: "ai_key" });
    expect(screen.getByText("Bob Builder and Carol want an AI key")).toBeTruthy();
    expect(screen.getByRole("link", { name: "Add an AI key" }).getAttribute("href")).toBe("/o/acme/apps?tab=accounts&kind=ai-key");
  });
});
