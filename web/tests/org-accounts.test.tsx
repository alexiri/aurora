import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { LimitControl } from "@/components/accounts/limit-control";
import { ConnectInWorkspaceDialog } from "@/components/apps/org-apps-list";
import { buildJourneyConnect, orgDoneSentence, orgStepsOf } from "@/components/apps/connect-journey/logic";
import { loweringSentences, splitWorkspaces } from "@/lib/accounts";
import type { Account } from "@/lib/api/accounts";
import type { ConnectMethod } from "@/lib/api/apps";

// (P5-UX-11, C-936, C-937, C-947, C-990) The organization's journey, its limit, the lowering confirmation and the
// chooser's order.

vi.mock("next/navigation", () => ({ useRouter: () => ({ push: vi.fn() }), useParams: () => ({ org: "acme" }) }));

const json = (status: number, body: unknown) => new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
type Handler = (req: Request, body: unknown) => Response | Promise<Response>;
let calls: { key: string; body: unknown }[];
let routes: Record<string, Handler>;

beforeEach(() => {
  calls = [];
  routes = {};
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const req = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"));
      const url = new URL(req.url, "http://localhost");
      const text = req.method === "GET" || req.method === "DELETE" ? "" : await req.clone().text();
      const body = text ? JSON.parse(text) : undefined;
      const key = `${req.method} ${url.pathname}`;
      calls.push({ key: `${key}${url.search}`, body });
      const h = routes[key];
      return h ? h(req, body) : json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});
const wrap = (ui: React.ReactElement) => render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>{ui}</QueryClientProvider>);

describe("the words and steps of the organization's journey", () => {
  it("has no Agent step and sets the limit in the Access step", () => {
    expect(orgStepsOf({ appGiven: true, hasExisting: false, useExisting: false, hasChoice: true })).toEqual(["access", "signin", "done"]);
    expect(orgStepsOf({ appGiven: false, hasExisting: false, useExisting: false, hasChoice: true })).toEqual(["app", "access", "signin", "done"]);
    // An app that cannot change anything has no limit to set.
    expect(orgStepsOf({ appGiven: true, hasExisting: false, useExisting: false, hasChoice: false })).toEqual(["signin", "done"]);
  });
  it("offers an existing account, and using it skips to Done", () => {
    expect(orgStepsOf({ appGiven: true, hasExisting: true, useExisting: false, hasChoice: true })).toEqual(["account", "access", "signin", "done"]);
    expect(orgStepsOf({ appGiven: true, hasExisting: true, useExisting: true, hasChoice: true })).toEqual(["account", "done"]);
  });
  it("says Done in the sentence of the spec", () => {
    expect(orgDoneSentence("AWS", "Acme")).toBe("AWS is connected for Acme. No agent can use it yet.");
  });
  it("connects with scope organization, the limit and no grant", () => {
    const method = { id: "api_key", kind: "fields", fields: [], scopes: [], scope_presets: { read: [], write: [] }, access_levels: [] } as unknown as ConnectMethod;
    const body = buildJourneyConnect({ workspaceId: "", method, fields: {}, agentIds: [], level: "ask", hasWriteTools: true, org: { orgId: "o1" } });
    expect(body).toMatchObject({ scope: "organization", org_id: "o1", method_id: "api_key", limit: "ask" });
    expect(body).not.toHaveProperty("grant");
    expect(body).not.toHaveProperty("workspace_id");
    expect(body).not.toHaveProperty("access");
  });
});

describe("the lowering confirmation", () => {
  it("groups who changes by what they go from and to", () => {
    expect(
      loweringSentences([
        { from: "act", to: "read" },
        { from: "act", to: "read" },
        { from: "act", to: "read" },
        { from: "ask", to: "none" },
        { from: "custom", to: "ask" },
      ]),
    ).toEqual(["3 agents go from Change without asking to Read only", "1 agent goes from Ask before changes to No access", "1 agent goes from Custom to Ask before changes"]);
    expect(loweringSentences([])).toEqual([]);
  });
});

const account = (limit: "read" | "ask" | "act"): Account =>
  ({
    ref: { type: "connection", id: "c1" },
    kind: "app",
    name: "AWS",
    app: { app_id: "aws", name: "AWS", custom_only: false },
    scope: "organization",
    limit,
    levels: ["read", "ask", "act"].slice(0, ["read", "ask", "act"].indexOf(limit) + 1),
    health: { state: "ok" },
    agents: [],
    can: { give_access: true, change_access: true, manage: true },
  }) as unknown as Account;

describe("the limit control", () => {
  it("reads the limit and, when lowering, shows who goes down before saving", async () => {
    routes["GET /v1/connections/c1/limit-preview"] = () =>
      json(200, { lowered: [{ agent: { id: "a1", name: "Daneel", handle: "daneel" }, workspace: { id: "w1", name: "Bob" }, from: "act", to: "read" }] });
    routes["PATCH /v1/connections/c1"] = () => json(200, { id: "c1" });
    wrap(<LimitControl account={account("act")} orgId="o1" />);
    expect(screen.getByText("Change without asking")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Change the limit of AWS" }));
    const dlg = await screen.findByRole("dialog");
    fireEvent.click(within(dlg).getByLabelText(/Read only/));
    await within(dlg).findByText("1 agent goes from Change without asking to Read only");
    expect(calls.some((c) => c.key.startsWith("GET /v1/connections/c1/limit-preview?limit=read"))).toBe(true);
    expect(calls.some((c) => c.key.startsWith("PATCH"))).toBe(false);
    fireEvent.click(within(dlg).getByRole("button", { name: "Lower to Read only" }));
    await waitFor(() => expect(calls.find((c) => c.key === "PATCH /v1/connections/c1")?.body).toEqual({ limit: "read" }));
  });
  it("raising asks for no preview and changes no agent", async () => {
    routes["PATCH /v1/connections/c1"] = () => json(200, { id: "c1" });
    wrap(<LimitControl account={account("read")} orgId="o1" />);
    fireEvent.click(screen.getByRole("button", { name: "Change the limit of AWS" }));
    const dlg = await screen.findByRole("dialog");
    fireEvent.click(within(dlg).getByLabelText(/Ask before changes/));
    fireEvent.click(within(dlg).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(calls.find((c) => c.key === "PATCH /v1/connections/c1")?.body).toEqual({ limit: "ask" }));
    expect(calls.some((c) => c.key.includes("limit-preview"))).toBe(false);
  });
});

describe("where to connect", () => {
  const ws = (id: string, name: string, extra: Record<string, unknown> = {}) => ({ id, name, slug: id, my_role: "owner", ...extra });
  it("splits the person's own and shared workspaces from other people's personal ones", () => {
    const items: Array<{ id: string; personal_of?: { id: string } | null }> = [
      { id: "w1" },
      { id: "w2", personal_of: { id: "u2" } },
      { id: "w3", personal_of: { id: "u1" } },
    ];
    const { mine, others } = splitWorkspaces(items, "u1");
    expect(mine.map((w) => w.id)).toEqual(["w1", "w3"]);
    expect(others.map((w) => w.id)).toEqual(["w2"]);
  });
  it("leads with the whole organization and folds other people's personal workspaces away", async () => {
    routes["GET /v1/orgs/o1/workspaces"] = () =>
      json(200, { items: [ws("w1", "Shared"), ws("w2", "Alice", { personal_of: { id: "u2", name: "Alice" } })], next_cursor: null });
    routes["GET /v1/me"] = () => json(200, { user: { id: "u1", display_name: "Me", email: "me@x.io" } });
    routes["GET /v1/apps"] = () => json(200, { items: [{ app_id: "aws", name: "AWS", available: true }], next_cursor: null });
    const onOrg = vi.fn();
    wrap(<ConnectInWorkspaceDialog orgId="o1" orgSlug="acme" app={{ app_id: "aws", name: "AWS", family: null }} canOrg onOrg={onOrg} onClose={vi.fn()} />);
    const where = await screen.findByLabelText("Where to connect");
    const rows = within(where).getAllByRole("listitem");
    expect(rows[0]!.textContent).toContain("For the whole organization");
    expect(rows[1]!.textContent).toContain("Shared");
    const folded = screen.getByText(/Other people's personal workspaces/).closest("details")!;
    expect(folded.hasAttribute("open")).toBe(false);
    expect(within(folded).getByText("Alice")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Connect for the whole organization" }));
    expect(onOrg).toHaveBeenCalled();
  });
  it("does not offer the whole organization to people who are not organization admins", async () => {
    routes["GET /v1/orgs/o1/workspaces"] = () => json(200, { items: [ws("w1", "Shared")], next_cursor: null });
    routes["GET /v1/me"] = () => json(200, { user: { id: "u1", display_name: "Me", email: "me@x.io" } });
    routes["GET /v1/apps"] = () => json(200, { items: [], next_cursor: null });
    wrap(<ConnectInWorkspaceDialog orgId="o1" orgSlug="acme" app={{ app_id: "aws", name: "AWS", family: null }} onClose={vi.fn()} />);
    await screen.findByText("Shared");
    expect(screen.queryByText("For the whole organization")).toBeNull();
  });
});

describe("Give access on an organization's account", () => {
  it("lists agents as <workspace> · <agent>, own workspaces first, one choice per ticked agent within the limit", async () => {
    const ws = (id: string, name: string, extra: Record<string, unknown> = {}) => ({ id, name, slug: id, my_role: "owner", ...extra });
    routes["GET /v1/orgs/o1/workspaces"] = () =>
      json(200, { items: [ws("w2", "Alice", { personal_of: { id: "u2", name: "Alice" } }), ws("w1", "Shared")], next_cursor: null });
    routes["GET /v1/me"] = () => json(200, { user: { id: "u1", display_name: "Me", email: "me@x.io" } });
    routes["GET /v1/workspaces/w1/agents"] = () => json(200, { items: [{ id: "a1", name: "Daneel", handle: "daneel", kind: "hosted" }], next_cursor: null });
    routes["GET /v1/workspaces/w2/agents"] = () => json(200, { items: [{ id: "a2", name: "Giskard", handle: "giskard", kind: "hosted" }], next_cursor: null });
    routes["PUT /v1/agents/a1/grants/connections/c1"] = () => json(200, {});
    routes["PUT /v1/agents/a2/grants/connections/c1"] = () => json(200, {});
    const { OrgGiveAccessDialog } = await import("@/components/accounts/org-give-access");
    const subject = { target: { kind: "connection", id: "c1" }, scope: { kind: "organization", id: "o1" }, orgId: "o1" } as const;
    wrap(<OrgGiveAccessDialog account={account("ask")} orgId="o1" orgName="Acme" subject={subject} onOpenChange={vi.fn()} />);
    const first = await screen.findByLabelText("Give Daneel access");
    const second = await screen.findByLabelText("Give Giskard access");
    expect(screen.getByText("Shared · Daneel")).toBeTruthy();
    expect(screen.getByText("Alice · Giskard")).toBeTruthy();
    expect(first.compareDocumentPosition(second) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    fireEvent.click(first);
    fireEvent.click(second);
    // Within the limit: Change without asking is not a choice; the account is limited by the organization.
    expect(screen.queryAllByLabelText(/Change without asking/)).toHaveLength(0);
    expect(screen.getAllByText("Limited by Acme.").length).toBe(2);
    const groups = screen.getAllByRole("radiogroup");
    fireEvent.click(within(groups[1]!).getByLabelText(/Ask before changes/));
    fireEvent.click(screen.getByRole("button", { name: "Give access" }));
    await waitFor(() => expect(calls.filter((c) => c.key.startsWith("PUT /v1/agents/")).length).toBe(2));
    expect(calls.find((c) => c.key === "PUT /v1/agents/a1/grants/connections/c1")?.body).toEqual({ level: "read" });
    expect(calls.find((c) => c.key === "PUT /v1/agents/a2/grants/connections/c1")?.body).toEqual({ level: "ask" });
  });
});
