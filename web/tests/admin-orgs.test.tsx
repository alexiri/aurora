import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { OrgDetailView } from "@/components/admin/org-detail-view";
import { AdminOrgsView } from "@/components/admin/orgs-view";

const push = vi.fn();
let search = "";
vi.mock("next/navigation", () => ({
  useRouter: () => ({ push, replace: vi.fn(), back: vi.fn(), refresh: vi.fn() }),
  usePathname: () => "/admin/orgs",
  useSearchParams: () => new URLSearchParams(search),
}));

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
type Handler = (req: Request) => Response | Promise<Response>;
function mockFetch(routes: Record<string, Handler>) {
  const fn = vi.fn(async (input: Request | string | URL) => {
    const req = input as Request;
    const url = new URL(req.url, "http://localhost");
    const h = routes[`${req.method} ${url.pathname}`];
    return h ? h(req) : json(404, { error: { code: "not_found", message: "nf" } });
  });
  vi.stubGlobal("fetch", fn);
  return fn;
}
function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  push.mockReset();
  search = "";
});

describe("reauth_error notice (FIX-152)", () => {
  const routes = () => ({
    "GET /v1/admin/orgs": () => json(200, { items: [org()], next_cursor: null }),
    [`GET /v1/admin/orgs/${ORG}`]: () => json(200, org()),
    [`GET /v1/admin/orgs/${ORG}/workspaces`]: () => json(200, { items: [], next_cursor: null }),
  });
  const views = [
    ["list", () => <AdminOrgsView />],
    ["detail", () => <OrgDetailView orgId={ORG} />],
  ] as const;
  for (const [name, view] of views) {
    it(`${name}: reauth_stale shows the stale text`, async () => {
      mockFetch(routes());
      search = "reauth_error=reauth_stale";
      wrap(view());
      expect((await screen.findByRole("alert")).textContent).toMatch(/did not confirm a fresh sign-in.*reauth_stale/);
    });
    it(`${name}: an unknown code shows the fallback`, async () => {
      mockFetch(routes());
      search = "reauth_error=<b>x</b>";
      wrap(view());
      expect((await screen.findByRole("alert")).textContent).toBe("Re-authentication failed. Please try again.");
    });
    it(`${name}: no alert without the param`, async () => {
      mockFetch(routes());
      wrap(view());
      await screen.findByRole("heading", { level: 1 });
      expect(screen.queryByRole("alert")).toBeNull();
    });
  }
});

const ORG = "11111111-1111-4111-8111-111111111111";
const ORG2 = "22222222-2222-4222-8222-222222222222";
const WS = "33333333-3333-4333-8333-333333333333";
const frozen = { at: "2026-11-02T00:00:00Z", by_label: "root", reason: "x", authority: "instance_admin", level: "org" };
const org = (over: Record<string, unknown> = {}) => ({
  id: ORG,
  slug: "acme",
  name: "Acme",
  created_at: "2026-11-01T00:00:00Z",
  frozen: null,
  ownerless: false,
  member_count: 30,
  workspace_count: 4,
  agent_count: 12,
  ...over,
});
const instance = { oidc_providers: [{ id: "corp", display_name: "Corp SSO" }] };
const reauth = () => json(412, { error: { code: "reauth_required", message: "Re-authenticate" } });

/** Radix opens a dropdown on keydown Enter (jsdom has no pointer capture). */
const openMenu = async (name: string) => {
  fireEvent.keyDown(await screen.findByRole("button", { name }), { key: "Enter" });
};
const type = (label: string, value: string) => fireEvent.change(within(screen.getByRole("dialog")).getByLabelText(label), { target: { value } });

describe("Admin › Organizations (P5-ADMDEL)", () => {
  it("lists organizations with counts and state badges", async () => {
    mockFetch({
      "GET /v1/admin/orgs": () =>
        json(200, { items: [org(), org({ id: ORG2, slug: "globex", name: "Globex", ownerless: true, frozen })], next_cursor: null }),
    });
    wrap(<AdminOrgsView />);
    const table = await screen.findByRole("region", { name: "Organizations" });
    const rows = within(table).getAllByRole("row");
    expect(rows).toHaveLength(3);
    expect(within(rows[1]!).getByText("30")).toBeInTheDocument();
    expect(within(rows[1]!).getByText("12")).toBeInTheDocument();
    expect(within(rows[1]!).queryByText("Frozen")).toBeNull();
    expect(within(rows[2]!).getByText("Frozen")).toBeInTheDocument();
    expect(within(rows[2]!).getByText("No owner")).toBeInTheDocument();
    // The row is the target of the ownerless banner's link.
    expect(rows[1]!.id).toBe("org-acme");
  });

  it("offers Freeze…, Workspaces and Delete organization… in the row menu; Unfreeze when frozen", async () => {
    mockFetch({
      "GET /v1/admin/orgs": () => json(200, { items: [org(), org({ id: ORG2, slug: "globex", name: "Globex", frozen })], next_cursor: null }),
    });
    wrap(<AdminOrgsView />);
    await openMenu("Actions for Acme");
    expect(await screen.findByRole("menuitem", { name: "Freeze…" })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: "Workspaces" })).toHaveAttribute("href", `/admin/orgs/${ORG}`);
    expect(screen.getByRole("menuitem", { name: "Delete organization…" })).toBeInTheDocument();
    fireEvent.keyDown(screen.getByRole("menu"), { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());
    await openMenu("Actions for Globex");
    expect(await screen.findByRole("menuitem", { name: "Unfreeze" })).toBeInTheDocument();
    expect(screen.queryByRole("menuitem", { name: "Freeze…" })).toBeNull();
  });

  async function openDelete() {
    await openMenu("Actions for Acme");
    fireEvent.click(await screen.findByRole("menuitem", { name: "Delete organization…" }));
    return screen.findByRole("dialog");
  }

  it("names the consequences with the counts and gates the submit on the exact slug", async () => {
    mockFetch({ "GET /v1/admin/orgs": () => json(200, { items: [org()], next_cursor: null }) });
    wrap(<AdminOrgsView />);
    const dialog = await openDelete();
    expect(dialog).toHaveTextContent("Deletes the organization Acme with its 4 workspaces, 12 agents and 30 members\u2019 access");
    expect(dialog).toHaveTextContent("the audit log and usage records are kept");
    const submit = within(dialog).getByRole("button", { name: "Delete organization" });
    expect(submit).toBeDisabled();
    type("Type acme to confirm", "acm");
    expect(submit).toBeDisabled();
    type("Type acme to confirm", "acme");
    expect(submit).toBeEnabled();
    type("Type acme to confirm", "acmex");
    expect(submit).toBeDisabled();
  });

  it("412 reauth_required shows the re-authentication step, then one retry deletes and says so", async () => {
    let calls = 0;
    const bodies: unknown[] = [];
    mockFetch({
      "GET /v1/instance": () => json(200, instance),
      "GET /v1/admin/orgs": () => json(200, { items: [org()], next_cursor: null }),
      [`DELETE /v1/admin/orgs/${ORG}`]: async (req) => {
        bodies.push(await req.json());
        calls += 1;
        return calls === 1 ? reauth() : new Response(null, { status: 204 });
      },
    });
    wrap(<AdminOrgsView />);
    const dialog = await openDelete();
    type("Type acme to confirm", "acme");
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete organization" }));
    const link = await within(dialog).findByRole("link", { name: "Sign in again with Corp SSO" });
    expect(link.getAttribute("href")).toBe("/v1/auth/oidc/corp/start?return_to=%2Fadmin%2Forgs&reauth=1");
    expect(within(dialog).getByRole("alert")).toHaveTextContent("signing in again");
    // The person signs in again (outside this test) and confirms once more.
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete organization" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(calls).toBe(2);
    expect(bodies).toEqual([{ confirm: "acme" }, { confirm: "acme" }]);
    expect(await screen.findByTestId("admin-orgs-done")).toHaveTextContent("Deleted the organization Acme (acme).");
  });

  it("shows the API's confirmation_mismatch message and keeps the dialog open", async () => {
    mockFetch({
      "GET /v1/admin/orgs": () => json(200, { items: [org()], next_cursor: null }),
      [`DELETE /v1/admin/orgs/${ORG}`]: () => json(400, { error: { code: "confirmation_mismatch", message: "Confirmation does not match." } }),
    });
    wrap(<AdminOrgsView />);
    const dialog = await openDelete();
    type("Type acme to confirm", "acme");
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete organization" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("The confirmation text does not match.");
    expect(screen.getByRole("dialog")).toBeInTheDocument();
  });

  it("freezes with a reason", async () => {
    const bodies: unknown[] = [];
    mockFetch({
      "GET /v1/admin/orgs": () => json(200, { items: [org()], next_cursor: null }),
      [`POST /v1/admin/orgs/${ORG}/freeze`]: async (req) => {
        bodies.push(await req.json());
        return json(200, { frozen: null, effective_frozen: true, outcome: "frozen" });
      },
    });
    wrap(<AdminOrgsView />);
    await openMenu("Actions for Acme");
    fireEvent.click(await screen.findByRole("menuitem", { name: "Freeze…" }));
    const dialog = await screen.findByRole("dialog");
    const submit = within(dialog).getByRole("button", { name: "Freeze organization" });
    expect(submit).toBeDisabled();
    type("Reason", "incident 42");
    fireEvent.click(submit);
    await waitFor(() => expect(bodies).toEqual([{ reason: "incident 42" }]));
  });
});

describe("Admin › one organization (P5-ADMDEL)", () => {
  const ws = (over: Record<string, unknown> = {}) => ({
    id: WS,
    slug: "main",
    name: "Main",
    personal: false,
    agent_count: 3,
    created_at: "2026-11-01T00:00:00Z",
    ...over,
  });

  it("shows the counts and the workspaces table with a Personal badge", async () => {
    mockFetch({
      [`GET /v1/admin/orgs/${ORG}`]: () => json(200, org()),
      [`GET /v1/admin/orgs/${ORG}/workspaces`]: () =>
        json(200, { items: [ws(), ws({ id: "44444444-4444-4444-8444-444444444444", slug: "ada", name: "Ada", personal: true, agent_count: 1 })], next_cursor: null }),
    });
    wrap(<OrgDetailView orgId={ORG} />);
    const table = await screen.findByRole("region", { name: "Workspaces of Acme" });
    const rows = within(table).getAllByRole("row");
    expect(rows).toHaveLength(3);
    expect(within(rows[2]!).getByText("Personal")).toBeInTheDocument();
    expect(within(rows[1]!).queryByText("Personal")).toBeNull();
    expect(screen.getByText(/30 active members/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Delete workspace Main" })).toBeInTheDocument();
  });

  it("deletes a workspace with the typed slug, 412 → re-auth → retry, then says what was deleted", async () => {
    let calls = 0;
    mockFetch({
      "GET /v1/instance": () => json(200, instance),
      [`GET /v1/admin/orgs/${ORG}`]: () => json(200, org()),
      [`GET /v1/admin/orgs/${ORG}/workspaces`]: () => json(200, { items: [ws()], next_cursor: null }),
      [`DELETE /v1/admin/workspaces/${WS}`]: () => {
        calls += 1;
        return calls === 1 ? reauth() : new Response(null, { status: 204 });
      },
    });
    wrap(<OrgDetailView orgId={ORG} />);
    fireEvent.click(await screen.findByRole("button", { name: "Delete workspace Main" }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("Deletes the workspace Main with its 3 agents");
    const submit = within(dialog).getByRole("button", { name: "Delete workspace" });
    expect(submit).toBeDisabled();
    type("Type main to confirm", "main");
    fireEvent.click(submit);
    const link = await within(dialog).findByRole("link", { name: "Sign in again with Corp SSO" });
    expect(link.getAttribute("href")).toContain(`return_to=%2Fadmin%2Forgs%2F${ORG}`);
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete workspace" }));
    expect(await screen.findByTestId("admin-orgs-done")).toHaveTextContent("Deleted the workspace Main (main).");
    expect(calls).toBe(2);
  });

  it("deleting the organization returns to the list", async () => {
    mockFetch({
      [`GET /v1/admin/orgs/${ORG}`]: () => json(200, org()),
      [`GET /v1/admin/orgs/${ORG}/workspaces`]: () => json(200, { items: [], next_cursor: null }),
      [`DELETE /v1/admin/orgs/${ORG}`]: () => new Response(null, { status: 204 }),
    });
    wrap(<OrgDetailView orgId={ORG} />);
    fireEvent.click(await screen.findByRole("button", { name: "Delete organization…" }));
    const dialog = await screen.findByRole("dialog");
    type("Type acme to confirm", "acme");
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete organization" }));
    await waitFor(() => expect(push).toHaveBeenCalledWith("/admin/orgs"));
  });
});
