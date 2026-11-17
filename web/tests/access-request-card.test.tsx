// (P5-UX-17, C-984, D-348) An agent's request for access: the card in the conversation in each state, and the
// banner above the workspace's Accounts list.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { RequestsBanner } from "@/components/accounts/requests-banner";
import { AccessRequestCard } from "@/components/chat/access-request-card";
import type { TranscriptEvent } from "@/lib/api/conversations";

vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme", ws: "main" }),
  usePathname: () => "/o/acme/w/main/agents/daneel",
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  useSearchParams: () => new URLSearchParams(),
}));

const json = (status: number, body: unknown) => new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
const ev = (seq: number, payload: Record<string, unknown>): TranscriptEvent => ({ seq, type: "notice", ts: "2026-01-01T00:00:00Z", payload });

const agent = { id: "ag1", name: "Daneel" };
const requested = ev(1, {
  code: "app_connection_requested", request_id: "rq1", app_id: "linear", app_name: "Linear",
  reason: "to file the bug", connect_url: "https://x/o/acme/w/main/agents/ag1?tab=access&connect=linear",
});
const website = ev(1, { code: "secret_proposed", request_id: "rq2", host: "api.weather.example.com", name: "Weather", reason: "forecast" });

function stub(role: "owner" | "member", calls: string[] = [], items: unknown[] = []) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request) => {
      const url = new URL(input.url, "http://localhost");
      if (input.method === "POST") {
        calls.push(url.pathname);
        return json(200, { id: "rq1", status: "dismissed" });
      }
      if (url.pathname === "/v1/workspaces/w") return json(200, { my_role: role, contacts: { owners: [{ name: "Ada Lovelace" }] } });
      if (url.pathname === "/v1/workspaces/w/access-requests") return json(200, { items });
      return json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
}

const wrap = (ui: React.ReactElement) => render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>{ui}</QueryClientProvider>);

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("the access request card", () => {
  it("says what the agent needs, marks the reason as its words, and opens the journey for that agent and app", async () => {
    stub("owner");
    wrap(<AccessRequestCard ev={requested} events={[requested]} wsId="w" agent={agent} />);
    expect(screen.getByText("Daneel needs Linear for this")).toBeInTheDocument();
    expect(screen.getByText(/Daneel's words:/)).toBeInTheDocument();
    expect(screen.getByText(/“to file the bug”/)).toBeInTheDocument();
    const give = await screen.findByRole("link", { name: "Give Daneel Linear" });
    expect(give.getAttribute("href")).toBe("/o/acme/w/main/agents/ag1?tab=access&connect=linear");
  });

  it("Not now dismisses the request", async () => {
    const calls: string[] = [];
    stub("owner", calls);
    wrap(<AccessRequestCard ev={requested} events={[requested]} wsId="w" agent={agent} />);
    fireEvent.click(await screen.findByRole("button", { name: "Not now" }));
    await waitFor(() => expect(calls).toEqual(["/v1/access-requests/rq1/dismiss"]));
  });

  it("says who can give access to a person who cannot", async () => {
    stub("member");
    wrap(<AccessRequestCard ev={requested} events={[requested]} wsId="w" agent={agent} />);
    expect(await screen.findByText(/Ada Lovelace/)).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: /Give Daneel/ })).toBeNull();
    expect(screen.queryByRole("button", { name: "Not now" })).toBeNull();
  });

  it("reads 'needs a token for <host>' with Add the token for a proposed website", async () => {
    stub("owner");
    wrap(<AccessRequestCard ev={website} events={[website]} wsId="w" agent={agent} />);
    expect(screen.getByText("Daneel needs a token for api.weather.example.com")).toBeInTheDocument();
    expect(await screen.findByRole("link", { name: "Add the token" })).toBeInTheDocument();
  });

  it("folds away once the access was given or the request dismissed", () => {
    stub("owner");
    const given = ev(2, { code: "app_access_given", request_id: "rq1", text: "Daneel can now read Linear." });
    const a = wrap(<AccessRequestCard ev={requested} events={[requested, given]} wsId="w" agent={agent} />);
    expect(a.container.textContent).toBe("");
    cleanup();
    const dismissed = ev(2, { code: "app_access_request_dismissed", request_id: "rq1", text: "Dismissed." });
    const b = wrap(<AccessRequestCard ev={requested} events={[requested, dismissed]} wsId="w" agent={agent} />);
    expect(b.container.textContent).toBe("");
  });
});

describe("the requests banner", () => {
  const item = (over: Record<string, unknown> = {}) => ({
    id: "rq1", agent_id: "ag1", agent_name: "Daneel", agent_handle: "daneel", conversation_id: "c1", kind: "app",
    app_id: "linear", app_name: "Linear", secret_id: null, host: null, reason: "r", status: "open", created_at: "2026-01-01T00:00:00Z", ...over,
  });

  it("lists each open request with a Give button for people who can change access", async () => {
    stub("owner", [], [item(), item({ id: "rq3", kind: "website", app_id: null, app_name: null, host: "h.example" })]);
    wrap(<RequestsBanner wsId="w" canOwn />);
    expect(await screen.findByText(/asked for/)).toBeInTheDocument();
    const give = screen.getByRole("link", { name: "Give Daneel Linear" });
    expect(give.getAttribute("href")).toBe("/o/acme/w/main/agents/ag1?tab=access&connect=linear");
    expect(screen.getAllByRole("listitem")).toHaveLength(1);
  });

  it("shows no button to people who cannot, and nothing when nothing is open", async () => {
    stub("member", [], [item()]);
    wrap(<RequestsBanner wsId="w" canOwn={false} />);
    expect(await screen.findByText(/asked for/)).toBeInTheDocument();
    expect(screen.queryByRole("link")).toBeNull();
    cleanup();
    stub("owner", [], []);
    const { container } = wrap(<RequestsBanner wsId="w" canOwn />);
    await new Promise((r) => setTimeout(r, 50));
    expect(container.textContent).toBe("");
  });
});
