// (FIX-153, C-891, C-894) The gateway's dashboard links are real routes; the agent page accepts an id; the org Apps
// page lists apps with Connect…; the org Connections empty state points at it.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";

const router = vi.hoisted(() => ({ replace: vi.fn(), push: vi.fn() }));
const params = vi.hoisted(() => ({ value: { org: "acme", ws: "ops", handle: "a1" } as Record<string, string> }));
vi.mock("next/navigation", () => ({
  useParams: () => params.value,
  useRouter: () => router,
  usePathname: () => "/o/acme/w/ops/agents/a1",
  useSearchParams: () => new URLSearchParams(window.location.search),
}));
vi.mock("@/components/agents/chat-tab", () => ({ ChatTab: () => <div data-testid="chat-tab" /> }));
vi.mock("@/components/agents/access-tab", () => ({ AccessTab: () => <div data-testid="access-tab" /> }));
vi.mock("@/components/agents/instructions-tab", () => ({ InstructionsTab: () => <div /> }));
vi.mock("@/components/memory/memory-tab", () => ({ MemoryTab: () => <div /> }));
vi.mock("@/components/agents/connect-tab", () => ({ ConnectTab: () => <div /> }));

import { AgentPage } from "@/components/agents/agent-page";
import { AvailabilityView } from "@/components/apps/availability-view";
import { AccountsList } from "@/components/accounts/accounts-list";
import { AGENT_PAGE_IDS } from "@/components/agents/sections";
import { legacyTarget } from "@/lib/legacy-routes";

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
type Handler = (req: Request) => Response | Promise<Response>;
function mockFetch(routes: Record<string, Handler>) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const req = input as Request;
      const url = new URL(req.url, "http://localhost");
      const h = routes[`${req.method} ${url.pathname}`];
      return h ? h(req) : json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
}
const wrap = (ui: React.ReactElement) => {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
};
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  window.history.replaceState(null, "", "/");
  router.replace.mockClear();
  router.push.mockClear();
  params.value = { org: "acme", ws: "ops", handle: "a1" };
});

describe("the conformance fixture (C-891)", () => {
  const fixture = JSON.parse(readFileSync(join(process.cwd(), "..", "tests/conformance/dashboard-links.json"), "utf8")) as {
    templates: Record<string, string>;
  };
  const routeOf = (tpl: string) => {
    const [path] = tpl.replace("{external}", "").split("?");
    return path!;
  };
  const dir = (path: string) => {
    // `/o/{org}/w/{ws}/credentials` -> src/app/o/[org]/w/[ws]/credentials; `{agent}` is the `[handle]` segment.
    const segs = path
      .split("/")
      .filter(Boolean)
      .map((s) => (s === "{org}" ? "[org]" : s === "{ws}" ? "[ws]" : s === "{agent}" ? "[handle]" : s));
    return join(process.cwd(), "src/app", ...segs, "page.tsx");
  };

  it("maps every template to an existing route", () => {
    for (const [name, tpl] of Object.entries(fixture.templates)) {
      // A page of its own, or an old address the dashboard redirects to its new place (C-960).
      const redirected = legacyTarget(routeOf(tpl).replace("{org}", "acme").replace("{ws}", "ops")) !== null;
      expect(existsSync(dir(routeOf(tpl))) || redirected, `${name}: ${tpl}`).toBe(true);
    }
  });

  it("uses only tabs the pages accept", () => {
    for (const [name, tpl] of Object.entries(fixture.templates)) {
      const tab = new URL(tpl.replace("{external}", "http://x"), "http://x").searchParams.get("tab");
      if (!tab) continue;
      // An old `/credentials` address is redirected, whatever its tab (C-960); the others name a tab of the agent page.
      if (routeOf(tpl).endsWith("/credentials")) {
        const to = legacyTarget(routeOf(tpl).replace("{org}", "acme").replace("{ws}", "ops"), new URL(tpl.replace("{external}", "http://x"), "http://x").searchParams);
        expect(to, `${name}: tab=${tab}`).not.toBeNull();
        continue;
      }
      expect(AGENT_PAGE_IDS as readonly string[], `${name}: tab=${tab}`).toContain(tab);
    }
  });
});

describe("agent page addressed by id (C-894)", () => {
  const agent = { id: "a1", handle: "scout", name: "Scout", kind: "hosted", is_default: false, avatar_url: null, brief: null, frozen: null, updated_at: "2026-01-01T00:00:00Z" };
  const routes = () => ({
    "GET /v1/resolve": () => json(200, { org: { id: "o1" }, workspace: { id: "w1" } }),
    "GET /v1/workspaces/w1/agents": () => json(200, { items: [agent], next_cursor: null }),
    "GET /v1/workspaces/w1": () => json(200, { id: "w1", org_id: "o1", my_role: "owner", timezone: "Europe/Zurich" }),
    "GET /v1/orgs/o1": () => json(200, { id: "o1", my_role: "owner", settings: {} }),
  });

  it("replaces the id with the handle and keeps the query", async () => {
    window.history.replaceState(null, "", "/o/acme/w/ops/agents/a1?tab=access");
    mockFetch(routes());
    wrap(<AgentPage />);
    await waitFor(() => expect(router.replace).toHaveBeenCalledWith("/o/acme/w/ops/agents/scout?tab=access"));
    expect(screen.queryByText(/not found/i)).toBeNull();
  });

  it("keeps the not-found state for an unknown value", async () => {
    params.value = { org: "acme", ws: "ops", handle: "nobody" };
    mockFetch(routes());
    wrap(<AgentPage />);
    await waitFor(() => expect(screen.getByRole("alert")).toBeInTheDocument());
    expect(router.replace).not.toHaveBeenCalled();
  });
});

describe("org Apps page (C-894)", () => {
  const app = (id: string, name: string) => ({
    app_id: id, name, description: `${name} desc`, available: true, connection_count: 0, has_dark_icon: false,
    package_id: "p", package_version: "1", serves_library: false, app_restrictions: { custom_only: false },
  });
  const ws = (id: string, slug: string, role: string) => ({ id, org_id: "o1", name: slug.toUpperCase(), slug, my_role: role });
  const routes = (): Record<string, Handler> => ({
    "GET /v1/orgs/o1/app-availability": () => json(200, { posture: "open", allow: [] }),
    "GET /v1/orgs/o1/workspaces": () =>
      json(200, { items: [ws("w1", "ops", "owner"), ws("w2", "lab", "member"), ws("w3", "dev", "owner")], next_cursor: null }),
    "GET /v1/apps": (req) => {
      const w = new URL(req.url).searchParams.get("workspace_id");
      // CERN Phonebook is unavailable in `dev`.
      return json(200, { items: [{ ...app("cern-phonebook", "CERN Phonebook"), available: w !== "w3" }], next_cursor: null });
    },
  });

  it("lists the apps under the open posture; Connect… offers only owned workspaces and navigates to the deep link", async () => {
    mockFetch(routes());
    wrap(<AvailabilityView orgId="o1" orgSlug="acme" canEdit />);
    const list = await screen.findByRole("list", { name: "Installed apps" });
    expect(within(list).getByText("CERN Phonebook")).toBeInTheDocument();
    expect(screen.getByLabelText("People can connect every app")).toBeChecked();
    fireEvent.click(within(list).getByRole("button", { name: "Connect CERN Phonebook" }));
    const dlg = await screen.findByRole("dialog");
    await within(dlg).findByRole("list", { name: "Your workspaces" });
    expect(within(dlg).queryByText("LAB")).toBeNull();
    // the unavailable one is disabled with the reason
    await waitFor(() => expect(within(dlg).getByRole("button", { name: "Connect in DEV" })).toBeDisabled());
    expect(within(dlg).getByText("Not available in this workspace.")).toBeInTheDocument();
    const ops = within(dlg).getByRole("button", { name: "Connect in OPS" });
    await waitFor(() => expect(ops).toBeEnabled());
    fireEvent.click(ops);
    expect(router.push).toHaveBeenCalledWith("/o/acme/w/ops/apps?tab=apps&connect=cern-phonebook");
  });

  it("with no owned workspace the dialog says who can connect", async () => {
    mockFetch({
      ...routes(),
      "GET /v1/orgs/o1/workspaces": () => json(200, { items: [ws("w2", "lab", "member")], next_cursor: null }),
    });
    wrap(<AvailabilityView orgId="o1" orgSlug="acme" canEdit />);
    fireEvent.click(await screen.findByRole("button", { name: "Connect CERN Phonebook" }));
    expect(await screen.findByText(/Only a workspace owner can connect apps/)).toBeInTheDocument();
  });
});

describe("Connections empty state", () => {
  const none = () => json(200, { items: [], next_cursor: null });
  const empty = () => ({
    "GET /v1/orgs/o1/accounts": none,
    "GET /v1/workspaces/w1/accounts": none,
  });

  it("at org scope the empty list says what an organization account is for, with no link", async () => {
    mockFetch(empty());
    wrap(<AccountsList scope={{ kind: "organization", id: "o1" }} orgId="o1" canOwn />);
    expect(await screen.findByText("No accounts for the whole organization yet")).toBeInTheDocument();
    expect(screen.queryByRole("link")).toBeNull();
  });

  it("at workspace scope the empty list opens the connect journey", async () => {
    mockFetch(empty());
    wrap(<AccountsList scope={{ kind: "workspace", id: "w1" }} orgId="o1" canOwn />);
    expect(await screen.findByRole("button", { name: "Connect an app or website" })).toBeInTheDocument();
  });
});
