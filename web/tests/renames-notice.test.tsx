import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { RenamesNotice } from "@/components/onboarding/renames-notice";
import { movedRows, RENAMES_NOTICE, showWhatMoved } from "@/lib/moved";

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

const calls: { key: string; body?: unknown }[] = [];
function mockFetch(routes: Record<string, (body?: unknown) => Response>) {
  calls.length = 0;
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const req = input instanceof Request ? input : null;
      const url = new URL(req ? req.url : String(input), "http://localhost");
      const key = `${req?.method ?? "GET"} ${url.pathname}`;
      const body = req && req.method !== "GET" ? await req.clone().json().catch(() => undefined) : undefined;
      calls.push({ key, body });
      const h = routes[key];
      return h ? h(body) : json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
}

function wrap() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <RenamesNotice />
    </QueryClientProvider>,
  );
}

const onboarding = (notices_seen: string[]) => ({ welcome_seen: true, guide_dismissed: [], notices_seen, tour_completed_at: null });
const world = (notices: string[], opts: { admin?: boolean; orgRole?: string; wsRole?: string } = {}) => {
  mockFetch({
    "GET /v1/me": () => json(200, { user: { display_name: "A", email: "a@x.org", is_instance_admin: opts.admin ?? false } }),
    "GET /v1/me/onboarding": () => json(200, onboarding(notices)),
    "PATCH /v1/me/onboarding": (b) => json(200, onboarding([...notices, (b as { notice_seen: string }).notice_seen])),
    "GET /v1/orgs": () => json(200, { items: [{ id: "o1", slug: "acme", name: "Acme", my_role: opts.orgRole ?? "member" }], next_cursor: null }),
    "GET /v1/orgs/o1/workspaces": () =>
      json(200, { items: [{ id: "w1", org_id: "o1", slug: "main", name: "Main", my_role: opts.wsRole ?? "member" }], next_cursor: null }),
  });
};

describe("movedRows (C-987)", () => {
  it("lists only what the viewer can open, each with its new link", () => {
    const member = movedRows({ instanceAdmin: false, anyOrg: "acme", anyWs: { org: "acme", ws: "main" } });
    expect(member.map((r) => `${r.from} → ${r.to}`)).toEqual([
      "Policy → Internet access",
      "Governance › Model access → AI models",
      "Governance › Egress → Internet access",
    ]);
    expect(member[0]!.href).toBe("/o/acme/w/main/internet-access");
    expect(member[1]!.href).toBe("/o/acme/ai-models");
  });

  it("gives an organization admin and an instance admin the rest", () => {
    const rows = movedRows({
      instanceAdmin: true,
      adminOrg: "acme",
      anyOrg: "acme",
      ownerWs: { org: "acme", ws: "main" },
      anyWs: { org: "acme", ws: "main" },
    });
    expect(rows.map((r) => `${r.from} → ${r.to}`)).toEqual([
      "Credentials → Apps & accounts",
      "Policy → Internet access",
      "Governance › Model access → AI models",
      "Governance › Egress → Internet access",
      "Governance › Apps → Apps & accounts › Apps",
      "Governance › BYO agents → Agents › BYO",
      "Members → People",
      "Settings › Access → People",
      "Channels → Chat platforms",
      "Storage and Audit → Data",
      "All organizations → Administration",
      "Plugins → Administration › Apps & accounts",
      "Connectors → Chat platforms",
      "Identity → People › Sign-in",
      "Placement → People › New users",
    ]);
    expect(rows.find((r) => r.from === "Identity")!.href).toBe("/admin/people?tab=sign-in");
  });
});

describe("the notice", () => {
  it("appears for an account that has not seen it, with the rows and links, and dismissing stores it on the server", async () => {
    world([], { orgRole: "admin", wsRole: "owner" });
    wrap();
    const notice = await screen.findByTestId("renames-notice");
    expect(within(notice).getByRole("heading", { name: "Pages have new names" })).toBeInTheDocument();
    expect(notice).toHaveTextContent("Every level now has the same pages, with the same names. Nothing was removed.");
    expect(await within(notice).findByRole("link", { name: "Credentials → Apps & accounts" })).toHaveAttribute("href", "/o/acme/w/main/apps?tab=accounts");
    fireEvent.click(within(notice).getByRole("button", { name: "Got it" }));
    await waitFor(() => expect(screen.queryByTestId("renames-notice")).toBeNull());
    expect(calls.find((c) => c.key === "PATCH /v1/me/onboarding")?.body).toEqual({ notice_seen: RENAMES_NOTICE });
  });

  it("never appears for an account that has it already (created after the release)", async () => {
    world([RENAMES_NOTICE], { orgRole: "admin", wsRole: "owner" });
    wrap();
    await waitFor(() => expect(calls.some((c) => c.key === "GET /v1/me/onboarding")).toBe(true));
    await new Promise((r) => setTimeout(r, 30));
    expect(screen.queryByTestId("renames-notice")).toBeNull();
  });

  it("What moved replays it for an account that dismissed it, without storing again", async () => {
    world([RENAMES_NOTICE], { orgRole: "admin", wsRole: "owner" });
    wrap();
    await waitFor(() => expect(calls.some((c) => c.key === "GET /v1/orgs/o1/workspaces")).toBe(true));
    act(() => showWhatMoved());
    const notice = await screen.findByTestId("renames-notice");
    fireEvent.click(within(notice).getByRole("button", { name: "Got it" }));
    await waitFor(() => expect(screen.queryByTestId("renames-notice")).toBeNull());
    expect(calls.some((c) => c.key === "PATCH /v1/me/onboarding")).toBe(false);
  });
});
