import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { readFileSync } from "node:fs";
import { describe, expect, it, vi } from "vitest";

import { AppName } from "@/components/apps/app-name";
import { AppsTab } from "@/components/apps/apps-tab";
import { ConnectDialog } from "@/components/apps/connect-dialog";
import { appDisplayName, productKey, productName } from "@/lib/api/app-name";
import { wrap } from "./support/render";

// FIX-135 (D-242, C-744): one catalogue entry per product. Apps of a family are one tile, the connect window asks where the
// product runs, and every list names an app "<family> · <variant>".

vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme", ws: "main" }),
  useSearchParams: () => new URLSearchParams(),
  useRouter: () => ({ replace: vi.fn(), push: vi.fn() }),
}));

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

const fam = (variant_label: string, instance_bound: boolean) => ({ id: "github", name: "GitHub", variant_label, instance_bound });
const summary = (app_id: string, name: string, family: ReturnType<typeof fam> | null, n: number) => ({
  app_id, name, package_id: "github", package_version: "2.2.0", description: `${name} d`, category: "development", available: true,
  connection_count: n, has_dark_icon: false, family,
  app_restrictions: { custom_only: false, full_allowed: true },
  serves_library: false,
});
const field = { name: "access_token", label: "Token", type: "secret", store: "credentials", required: true };
const detail = (s: ReturnType<typeof summary>, instances: unknown) => ({
  ...s, api_docs: null, connect_note: null, label_hint: null,
  connect: [{ id: "token", kind: "fields", label: null, description: null, recommended: false, fields: [field], scopes: [], default_scopes: [], access_levels: [], probe_scopes: [], client_key: null, callback: null }],
  tools: [], hosts: [], agent_instructions: null, credentials: [], metadata: [], oauth_clients: [], instances,
});

const GH = summary("github", "GitHub", fam("github.com", false), 2);
const GHES = summary("github-ghes", "GitHub Enterprise Server", fam("Enterprise Server", true), 1);
const SOLO = summary("solo", "Solo", null, 0);

interface Call { method: string; path: string; body: unknown }
function stub(extra: Record<string, (c: Call) => Response> = {}) {
  const calls: Call[] = [];
  const routes: Record<string, (c: Call) => Response> = {
    "GET /v1/apps": () => json(200, { items: [GHES, SOLO, GH] }),
    "GET /v1/apps/github": () => json(200, detail(GH, null)),
    "GET /v1/apps/github-ghes": () => json(200, detail(GHES, { label: "GHES host", default_host: null })),
    "GET /v1/apps/github-ghes/guide": () => json(404, { error: { code: "not_found", message: "x" } }),
    "GET /v1/apps/github/guide": () => json(404, { error: { code: "not_found", message: "x" } }),
    "GET /v1/me": () => json(200, { user: { display_name: "A", email: "a@x.io", is_instance_admin: false } }),
    "GET /v1/resolve": () => json(200, { org: { id: "o1", name: "Acme", slug: "acme" }, workspace: { id: "w1", name: "Main", slug: "main" } }),
    "GET /v1/orgs/o1/app-instances": () =>
      json(200, {
        items: [{ id: "i1", org_id: "o1", package_id: "github", app_id: "github-ghes", host: "ghe.acme.example", base_path: null, base_url: "https://ghe.acme.example", label: "Acme GHE", status: "active", created_by: null, created_at: "", updated_at: "", connections: 1, builtin: false }],
        next_cursor: null,
      }),
    "GET /v1/workspaces/w1/oauth-clients": () => json(200, { items: [], next_cursor: null }),
    ...extra,
  };
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string) => {
      // `useMe` calls fetch with a plain path; the generated client passes a Request.
      const req = typeof input === "string" ? null : input;
      const url = new URL(req ? req.url : input as string, "http://localhost");
      const method = req ? req.method : "GET";
      const c: Call = { method, path: url.pathname, body: method === "GET" || !req ? null : await req.json().catch(() => null) };
      calls.push(c);
      const h = routes[`${c.method} ${c.path}`];
      return h ? h(c) : json(404, { error: { code: "not_found", message: "x" } });
    }),
  );
  return calls;
}

describe("app names", () => {
  it("names a family member 'family · variant' and an app without one as it is", () => {
    expect(appDisplayName(GH)).toBe("GitHub · github.com");
    expect(appDisplayName(GHES)).toBe("GitHub · Enterprise Server");
    expect(appDisplayName(SOLO)).toBe("Solo");
    expect(productName(GHES)).toBe("GitHub");
    expect(productKey(GH)).toBe(productKey(GHES));
    expect(productKey(SOLO)).not.toBe(productKey(GH));
    render(<AppName app={GHES} host="ghe.acme.example" />);
    expect(document.body.textContent).toBe("GitHub · Enterprise Server (ghe.acme.example)");
  });

  // Every list, picker and dialog that names an app goes through the shared helper (replace ad-hoc `app.name`).
  it("every app-naming surface imports the shared helper", () => {
    for (const f of [
      "components/accounts/free-apps.tsx",
      "components/agents/grant-editor.tsx",
      "components/credentials/give-access.tsx",
      "components/apps/org-apps-list.tsx",
      "components/apps/instances-view.tsx",
      "components/apps/oauth-clients-panel.tsx",
      "components/apps/apps-tab.tsx",
      "components/apps/connect-dialog.tsx",
    ]) {
      const src = readFileSync(`${__dirname}/../src/${f}`, "utf8");
      expect(src, f).toMatch(/@\/lib\/api\/app-name/);
    }
  });
});

describe("catalogue and connect window", () => {
  it("shows one tile per product: GitHub (both deployments' connections) and Solo", async () => {
    stub();
    wrap(<AppsTab workspaceId="w1" orgId="o1" canOwn />);
    const list = await screen.findByRole("list", { name: "Apps" });
    const tiles = within(list).getAllByRole("listitem");
    expect(tiles).toHaveLength(2);
    expect(within(list).getByText("GitHub")).toBeInTheDocument();
    expect(within(list).queryByText("GitHub Enterprise Server")).toBeNull();
    expect(within(list).getByText("3 connections")).toBeInTheDocument();
    expect(within(list).getByRole("button", { name: "Connect GitHub" })).toBeInTheDocument();
  });

  it("asks where the product runs, then connects the chosen server's app with its instance", async () => {
    const calls = stub({
      "POST /v1/apps/github-ghes/connect": () => json(200, { kind: "connected", connection: { id: "k1" }, missing_scopes: [] }),
    });
    wrap(<ConnectDialog appId="github" workspaceId="w1" orgId="o1" onClose={() => {}} />);
    const where = await screen.findByRole("radiogroup", { name: "Where does your GitHub run?" });
    await waitFor(() => expect(within(where).getAllByRole("radio")).toHaveLength(2));
    const labels = within(where).getAllByRole("radio").map((r) => r.closest("label")?.textContent ?? "");
    expect(labels[0]).toContain("github.com");
    expect(labels[1]).toContain("Enterprise Server · Acme GHE");
    expect(labels[1]).toContain("https://ghe.acme.example");
    fireEvent.click(within(where).getAllByRole("radio")[1]!);
    fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    fireEvent.change(await screen.findByLabelText("Token"), { target: { value: "t" } });
    // The server is already chosen: the instance picker is not asked again.
    expect(screen.queryByLabelText("GHES host")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Connect" }));
    await waitFor(() => expect(calls.some((c) => c.path === "/v1/apps/github-ghes/connect")).toBe(true));
    expect(calls.find((c) => c.path === "/v1/apps/github-ghes/connect")?.body).toMatchObject({ instance_id: "i1", method_id: "token" });
  });

  it("the hosted deployment connects the hosted app without an instance", async () => {
    const calls = stub({
      "POST /v1/apps/github/connect": () => json(200, { kind: "connected", connection: { id: "k1" }, missing_scopes: [] }),
    });
    wrap(<ConnectDialog appId="github-ghes" workspaceId="w1" orgId="o1" onClose={() => {}} />);
    const where = await screen.findByRole("radiogroup", { name: "Where does your GitHub run?" });
    fireEvent.click(within(where).getAllByRole("radio")[0]!);
    fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    fireEvent.change(await screen.findByLabelText("Token"), { target: { value: "t" } });
    fireEvent.click(screen.getByRole("button", { name: "Connect" }));
    await waitFor(() => expect(calls.some((c) => c.path === "/v1/apps/github/connect")).toBe(true));
    expect(calls.find((c) => c.path === "/v1/apps/github/connect")?.body).not.toHaveProperty("instance_id");
  });

  it("without a registered server it says so (an instance admin is pointed at Self-hosted apps)", async () => {
    stub({
      "GET /v1/orgs/o1/app-instances": () => json(200, { items: [], next_cursor: null }),
      "GET /v1/me": () => json(200, { user: { display_name: "A", email: "a@x.io", is_instance_admin: true } }),
    });
    wrap(<ConnectDialog appId="github" workspaceId="w1" orgId="o1" onClose={() => {}} />);
    expect(await screen.findByText(/No self-hosted server is registered yet/)).toBeInTheDocument();
    expect(await screen.findByRole("link", { name: /Register a server/ })).toHaveAttribute("href", "/admin/apps?tab=self-hosted");
  });
});
