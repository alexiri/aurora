import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

vi.mock("next/navigation", () => ({
  usePathname: () => "/o/acme/w/main",
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  useSearchParams: () => new URLSearchParams(),
  useParams: () => ({}),
}));

import { NeedsYou } from "@/components/overview/needs-you";
import { askersPhrase, needSentence } from "@/components/overview/setup-request-row";

const asker = (name: string, id: string) => ({ user_id: id, display_name: name, workspace_id: "ws1", asked_at: "2026-11-19T10:00:00Z" });
const request = (over: Record<string, unknown> = {}) => ({
  id: "r1",
  org_id: "org1",
  app_id: "google-calendar",
  app_name: "Google Calendar",
  status: "open",
  askers: [asker("Bob Builder", "u1")],
  needs: ["oauth_client"],
  unavailable: false,
  created_at: "2026-11-19T10:00:00Z",
  closed_at: null,
  ...over,
});

/** Answers the org, its setup requests and the decline; everything else Needs you reads is empty or refused. */
function mockApi(role: string, items: unknown[]) {
  const declined: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const req = typeof input === "string" ? new Request(new URL(input, "http://localhost")) : (input as Request);
      const url = new URL(req.url, "http://localhost");
      const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
      if (url.pathname === "/v1/orgs/org1") return json({ id: "org1", slug: "acme", name: "Acme", my_role: role });
      if (url.pathname === "/v1/orgs/org1/app-setup-requests") return json({ items });
      if (url.pathname === "/v1/orgs/org1/app-setup-requests/r1/decline" && req.method === "POST") {
        declined.push("r1");
        return json(request({ status: "declined" }));
      }
      if (url.pathname.endsWith("/accounts")) return json({ items: [], next_cursor: null });
      return json({ error: { code: "not_found", message: "nf" } }, 404);
    }),
  );
  return declined;
}

const wrap = (ui: React.ReactElement) => render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>{ui}</QueryClientProvider>);

describe("the words of a setup request", () => {
  it("names one, two or several askers", () => {
    expect(askersPhrase(["Bob"])).toEqual({ who: "Bob", verb: "wants" });
    expect(askersPhrase(["Bob", "Carol"])).toEqual({ who: "Bob and Carol", verb: "want" });
    expect(askersPhrase(["Bob", "Carol", "Dan"])).toEqual({ who: "Bob, Carol and 1 other", verb: "want" });
    expect(askersPhrase(["Bob", "Carol", "Dan", "Eve"]).who).toBe("Bob, Carol and 2 others");
  });

  it("says what is missing in one sentence", () => {
    expect(needSentence({ app_name: "Calendar", needs: ["oauth_client"], unavailable: false })).toBe("A sign-in app for Calendar needs to be registered once.");
    expect(needSentence({ app_name: "Calendar", needs: [], unavailable: true })).toBe("Calendar is switched off for your organization.");
    expect(needSentence({ app_name: "GHES", needs: ["instance"], unavailable: false })).toBe("A GHES server needs to be registered.");
  });
});

describe("Needs you: app setup requests (P5-POLISH-4)", () => {
  it("shows an owner or admin the request with Set up and Not now (one asker)", async () => {
    mockApi("admin", [request()]);
    wrap(<NeedsYou wsId="ws1" orgId="org1" base="/o/acme/w/main" />);
    expect(await screen.findByText("Bob Builder wants Google Calendar")).toBeTruthy();
    expect(screen.getByText("A sign-in app for Google Calendar needs to be registered once.")).toBeTruthy();
    // The sign-in app is what is missing, so Set up opens the OAuth clients tab at the app.
    expect(screen.getByRole("link", { name: "Set up" }).getAttribute("href")).toBe("/o/acme/apps?tab=oauth-clients&app=google-calendar");
    // The organization's requests sit under their own heading; the heading's number is the workspace's (none waits there).
    expect(screen.getByRole("heading", { name: "For your organization" })).toBeTruthy();
    expect(screen.queryByLabelText(/waiting$/)).toBeNull();
  });

  it("names two askers", async () => {
    mockApi("owner", [request({ askers: [asker("Bob Builder", "u1"), asker("Carol Chen", "u2")] })]);
    wrap(<NeedsYou wsId="ws1" orgId="org1" base="/o/acme/w/main" />);
    expect(await screen.findByText("Bob Builder and Carol Chen want Google Calendar")).toBeTruthy();
  });

  it("opens the Apps tab at the app when the organization switched it off", async () => {
    mockApi("owner", [request({ needs: [], unavailable: true })]);
    wrap(<NeedsYou wsId="ws1" orgId="org1" base="/o/acme/w/main" />);
    expect(await screen.findByText("Bob Builder wants Google Calendar")).toBeTruthy();
    expect(screen.getByRole("link", { name: "Set up" }).getAttribute("href")).toBe("/o/acme/apps?tab=apps&app=google-calendar");
  });

  it("shows nothing to anyone who is not an owner or admin, and never asks for the list", async () => {
    mockApi("member", [request()]);
    wrap(<NeedsYou wsId="ws1" orgId="org1" base="/o/acme/w/main" />);
    await new Promise((r) => setTimeout(r, 50));
    expect(screen.queryByText(/wants Google Calendar/)).toBeNull();
    const calls = (fetch as unknown as { mock: { calls: unknown[][] } }).mock.calls.map((c) => String((c[0] as Request).url ?? c[0]));
    expect(calls.some((u) => u.includes("app-setup-requests"))).toBe(false);
  });

  it("Not now confirms, names who will be told, and declines", async () => {
    const declined = mockApi("admin", [request({ askers: [asker("Bob Builder", "u1"), asker("Carol Chen", "u2")] })]);
    wrap(<NeedsYou wsId="ws1" orgId="org1" base="/o/acme/w/main" />);
    fireEvent.click(await screen.findByRole("button", { name: "Not now" }));
    expect(screen.getByText("2 people will be told you aren't setting up Google Calendar now.")).toBeTruthy();
    expect(declined).toEqual([]);
    fireEvent.click(screen.getAllByRole("button", { name: "Not now" }).at(-1)!);
    await waitFor(() => expect(declined).toEqual(["r1"]));
  });
});
