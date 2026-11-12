import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { AppShell } from "@/components/app-shell";

// FIX-027: the sidebar has two stacked selectors — Organization, then (only with an active
// organization) Workspace. "All organizations" (instance admins) is the one path to the admin
// area, so the user menu's Admin item and the FIX-026 sidebar link are gone. The workspace nav
// no longer carries a Members item (it jumped to the org's members view; people live in
// Settings › Access). The org Members nav is unchanged (not rendered on a workspace page here).

const nav = vi.hoisted(() => ({
  pathname: "/",
  params: {} as Record<string, string | undefined>,
}));
vi.mock("next/navigation", () => ({
  useParams: () => nav.params,
  usePathname: () => nav.pathname,
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
}));

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

const now = "2026-11-10T10:00:00Z";
const org = { id: "o1", slug: "acme", name: "Acme Labs", my_role: "owner", app_posture: "open", external_id: "x", settings: {}, created_at: now };
const otherOrg = { ...org, id: "o2", slug: "other", name: "No other org" };
const ws = { id: "w1", org_id: "o1", slug: "main", name: "Main workspace", my_role: "owner", memory_language: "en", timezone: "UTC", created_at: now, updated_at: now };

function me(isAdmin: boolean) {
  return { user: { id: "u1", email: "alex@example.org", display_name: "Alex", is_instance_admin: isAdmin }, default_org_id: null, memberships: [] };
}

/** Routes the shell needs; later tests override the `me` admin flag per world. */
function mockFetch(isAdmin: boolean) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const req = input instanceof Request ? input : null;
      const url = new URL(req ? req.url : String(input), "http://localhost");
      const p = url.pathname;
      if (p === "/v1/me") return json(200, me(isAdmin));
      if (p === "/v1/orgs") return json(200, { items: [org, otherOrg] });
      if (p === "/v1/resolve") return json(200, { org, workspace: ws });
      if (p === "/v1/orgs/o1/workspaces") return json(200, { items: [ws] });
      if (p === "/v1/orgs/o1") return json(200, { ...org, frozen: null });
      if (p === "/v1/workspaces/w1") return json(200, { ...ws, frozen: null });
      if (p === "/v1/workspaces/w1/approvals") return json(200, { items: [] });
      return json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
}

function wrap(ui: ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

function world(pathname: string, params: Record<string, string | undefined>, isAdmin = true) {
  nav.pathname = pathname;
  nav.params = params;
  mockFetch(isAdmin);
  return wrap(<AppShell>Page</AppShell>);
}

/** Open one of the sidebar selects (keyboard works in jsdom; the menu renders in a portal). */
async function open(testId: string) {
  const trigger = within(screen.getByTestId(testId)).getByRole("button");
  fireEvent.keyDown(trigger, { key: "Enter" });
  return trigger;
}

const item = (name: string) => screen.findByRole("menuitem", { name });
const semibold = (el: HTMLElement) => expect(el.className).toContain("font-semibold");

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("organization selector", () => {
  it("lists the user's organizations with the active one highlighted, on a workspace page", async () => {
    world("/o/acme/w/main/agents", { org: "acme", ws: "main" });
    await open("org-selector");
    semibold(await item("Acme Labs"));
    expect((await item("No other org")).getAttribute("href")).toBe("/o/other");
  });

  it("offers All organizations (to /admin) to instance admins only", async () => {
    world("/o/acme/w/main/agents", { org: "acme", ws: "main" });
    await open("org-selector");
    expect((await item("All organizations")).getAttribute("href")).toBe("/admin");

    cleanup();
    vi.unstubAllGlobals();
    world("/o/acme/w/main/agents", { org: "acme", ws: "main" }, false);
    await open("org-selector");
    await item("Acme Labs");
    expect(screen.queryByRole("menuitem", { name: "All organizations" })).toBeNull();
  });

  it("selecting an organization navigates to the org's workspace list — from /admin too", async () => {
    world("/admin/users", {});
    await open("org-selector");
    expect((await item("Acme Labs")).getAttribute("href")).toBe("/o/acme");
  });

  it("reads All organizations while in /admin, highlights the menu entry there, and drops the workspace selector", async () => {
    world("/admin/users", {});
    // The admin nav list still shows below the selectors (assert before the menu opens:
    // while it is open the modal menu hides the rest of the page).
    expect(screen.getByRole("link", { name: "Users" }).getAttribute("aria-current")).toBe("page");
    expect(within(screen.getByTestId("org-selector")).getByText("All organizations")).toBeInTheDocument();
    expect(screen.queryByTestId("workspace-selector")).toBeNull();
    await open("org-selector");
    const all = await item("All organizations");
    semibold(all);
    expect(all.getAttribute("data-active")).toBe("true");
  });

  it("shows the placeholder outside /admin when no org is active (account pages)", async () => {
    world("/account", {});
    expect(within(screen.getByTestId("org-selector")).getByText("Select organization")).toBeInTheDocument();
    expect(screen.queryByTestId("workspace-selector")).toBeNull();
    // The account nav replaces the old static caption.
    expect(screen.getByRole("link", { name: "Profile" })).toBeInTheDocument();
  });

  it("the visible caption labels the control (WCAG 2.5.3)", async () => {
    world("/o/acme/w/main/agents", { org: "acme", ws: "main" });
    // The value resolves from /v1/orgs (slug fallback until then). The name includes the
    // avatar's initial, so it contains every visible token of the trigger.
    await waitFor(() => expect(screen.getByRole("button", { name: "Organization A Acme Labs" })).toBeInTheDocument());
    expect(screen.getByRole("button", { name: "Workspace Main workspace" })).toBeInTheDocument();
  });
});

describe("workspace selector", () => {
  it("sits below the organization control and reads All workspaces at the org root", async () => {
    world("/o/acme", { org: "acme" });
    const shell = screen.getByRole("complementary", { name: "Sidebar" });
    const orgSel = within(shell).getByTestId("org-selector");
    const wsSel = within(shell).getByTestId("workspace-selector");
    expect(orgSel.compareDocumentPosition(wsSel) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(within(wsSel).getByText("All workspaces")).toBeInTheDocument();
    await open("workspace-selector");
    const all = await item("All workspaces…");
    expect(all.getAttribute("href")).toBe("/o/acme");
    semibold(all);
    expect((await item("Main workspace")).getAttribute("href")).toBe("/o/acme/w/main");
  });

  it("reads the workspace name on a workspace page and highlights it in the menu", async () => {
    world("/o/acme/w/main/agents", { org: "acme", ws: "main" });
    // The workspace name resolves from the workspaces query (slug fallback until then).
    await waitFor(() => expect(within(screen.getByTestId("workspace-selector")).getByText("Main workspace")).toBeInTheDocument());
    await open("workspace-selector");
    semibold(await item("Main workspace"));
    const all = await item("All workspaces…");
    expect(all.getAttribute("href")).toBe("/o/acme");
    expect(all.className).not.toContain("font-semibold");
  });
});

describe("superseded and moving entry points", () => {
  it("has no Members item in the workspace nav (Settings › Access owns workspace people)", async () => {
    world("/o/acme/w/main/agents", { org: "acme", ws: "main" });
    await screen.findByRole("link", { name: "Settings" });
    expect(screen.queryByRole("link", { name: "Members" })).toBeNull();
  });

  it("the user menu no longer offers Admin, and no Instance admin link remains", async () => {
    world("/o/acme/w/main/agents", { org: "acme", ws: "main" });
    const trigger = await screen.findByRole("button", { name: "Alex Instance admin" });
    fireEvent.keyDown(trigger, { key: "Enter" });
    await item("Show me around");
    expect(screen.queryByRole("menuitem", { name: "Admin" })).toBeNull();
    expect(screen.queryByRole("menuitem", { name: "Instance admin" })).toBeNull();
    expect(screen.queryByRole("link", { name: "Instance admin" })).toBeNull();
  });
});
