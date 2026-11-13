import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { readFileSync } from "node:fs";
import { afterEach, describe, expect, it, vi } from "vitest";

import { InstancesView } from "@/components/apps/instances-view";
import { ApiError } from "@/lib/api/client";
import { errorMessage, hostConflict, hostConflictMessage, type HostConflict } from "@/lib/api/errors";

const json = (status: number, body: unknown) =>
  new Response(body === null ? null : JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const conflictErr = (conflict: unknown) =>
  new ApiError("instance_host_conflict", "claimed", { fields: [{ path: "/host", message: "x" }], conflict }, 422);

describe("host conflict messages (FIX-132)", () => {
  const cases: [string, HostConflict, RegExp][] = [
    ["static claim", { kind: "static_claim", app_id: "plain", app_name: "Plain", package_id: "plainpkg" }, /claimed by the app Plain \(package plainpkg\)/],
    ["LLM endpoint with id", { kind: "llm_host", endpoint_id: "litellm-main" }, /LLM endpoint litellm-main/],
    ["LLM endpoint without id", { kind: "llm_host", endpoint_id: null }, /an LLM endpoint of this instance/],
    ["own address with setting", { kind: "aurora_address", setting: "hookhost_url" }, /own address \(setting hookhost_url\)/],
    ["own address without setting", { kind: "aurora_address", setting: null }, /one of Aurora's own addresses/],
    ["instance-wide registration", { kind: "instance", scope: "instance", app_id: "desk", app_name: "Desk" }, /self-hosted Desk and is available to every organization/],
    ["org registration, named", { kind: "instance", scope: "org", app_name: "Desk", org: { id: "o1", name: "Acme" } }, /by the organization Acme/],
    ["org registration, unnamed", { kind: "instance", scope: "org", app_name: "Desk" }, /by another organization — ask an instance administrator/],
  ];
  for (const [name, c, re] of cases) {
    it(`says who holds the host: ${name}`, () => {
      expect(hostConflictMessage(c)).toMatch(re);
      expect(errorMessage(conflictErr(c))).toMatch(re);
      expect(hostConflict(conflictErr(c))).toEqual(c);
    });
  }

  it("an unnamed registration of the own organization says so and never names another one", () => {
    const c: HostConflict = { kind: "instance", scope: "org", app_name: "Desk" };
    expect(hostConflictMessage(c, true)).toMatch(/in this organization/);
    expect(hostConflictMessage(c, true)).not.toMatch(/another organization/);
  });

  it("falls back to the generic sentence without details", () => {
    expect(errorMessage(new ApiError("instance_host_conflict", "x", null, 422))).toMatch(/already claimed/);
  });
});

const APP = {
  app_id: "desk",
  package_id: "selfhost",
  name: "Desk",
  description: "d",
  category: "dev",
  connect: [],
  tools: [],
  hosts: [],
  credentials: [],
  metadata: [],
  oauth_clients: [],
  instances: { label: "Desk instance", default_host: "saas.desk.example" },
};

const row = (over: Record<string, unknown>) => ({
  id: "i1",
  org_id: "o1",
  org_name: "Acme",
  org_slug: "acme",
  package_id: "selfhost",
  app_id: "desk",
  host: "desk.acme.example",
  label: "Acme desk",
  status: "active",
  created_by: null,
  created_at: "",
  updated_at: "",
  connections: 2,
  ...over,
});

interface Call {
  method: string;
  path: string;
  search: string;
  body: unknown;
}

function stub(routes: Record<string, (c: Call) => Response>) {
  const calls: Call[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string) => {
      const req = typeof input === "string" ? new Request(new URL(input, "http://localhost")) : input;
      const url = new URL(req.url);
      const c: Call = {
        method: req.method,
        path: url.pathname,
        search: url.search,
        body: req.method === "GET" ? null : await req.json().catch(() => null),
      };
      calls.push(c);
      const h = routes[`${c.method} ${c.path}`];
      return h ? h(c) : json(404, { error: { code: "not_found", message: "x" } });
    }),
  );
  return calls;
}

const base = {
  "GET /v1/apps": () => json(200, { items: [APP] }),
  "GET /v1/apps/desk": () => json(200, APP),
  "GET /v1/me": () => json(200, { user: { id: "u", email: "r@x.io", display_name: "R", is_instance_admin: true } }),
  "GET /v1/admin/orgs": () => json(200, { items: [{ id: "o1", name: "Acme", slug: "acme", created_at: "" }], next_cursor: null }),
};

describe("Admin › Self-hosted apps (FIX-132)", () => {
  it("lists every registration with its organization, filters by organization and promotes after confirming", async () => {
    let promoted = false;
    const calls = stub({
      ...base,
      "GET /v1/admin/app-instances": () =>
        json(200, {
          items: promoted
            ? [row({ org_id: null, org_name: null, org_slug: null })]
            : [row({}), row({ id: "i2", org_id: null, org_name: null, org_slug: null, host: "desk.wide.example", label: "Wide" })],
          next_cursor: null,
        }),
      "POST /v1/admin/app-instances/i1/promote": () => {
        promoted = true;
        return json(200, row({ org_id: null, org_name: null, org_slug: null }));
      },
    });
    wrap(<InstancesView scope={{ kind: "instance", all: true }} canManage />);
    const table = await screen.findByRole("table", { name: "Registered self-hosted apps" });
    expect(calls.some((c) => c.path === "/v1/admin/app-instances" && c.search.includes("scope=all"))).toBe(true);
    expect(within(table).getByText("Organization", { selector: "th" })).toBeInTheDocument();
    expect(within(table).getByText("Acme")).toBeInTheDocument();
    // an organization's registration can only be promoted (the admin manages the instance-wide ones)
    expect(screen.queryByRole("button", { name: "Delete desk.acme.example" })).toBeNull();
    expect(screen.getByRole("button", { name: "Delete desk.wide.example" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Make desk.wide.example instance-wide" })).toBeNull();

    const orgSelect = screen.getByLabelText("Organization", { selector: "select" });
    await within(orgSelect).findByRole("option", { name: "Acme" });
    fireEvent.change(orgSelect, { target: { value: "o1" } });
    await waitFor(() => expect(calls.some((c) => c.search.includes("org=o1"))).toBe(true));

    fireEvent.click(screen.getByRole("button", { name: "Make desk.acme.example instance-wide" }));
    const dlg = await screen.findByRole("dialog");
    expect(dlg.textContent).toMatch(/Every organization will see this self-hosted app/);
    expect(calls.some((c) => c.method === "POST")).toBe(false);
    fireEvent.click(within(dlg).getByRole("button", { name: "Make instance-wide" }));
    await waitFor(() => expect(calls.some((c) => c.method === "POST" && c.path === "/v1/admin/app-instances/i1/promote")).toBe(true));
  });

  it("a refused host names the holder, links the admin and offers Make instance-wide for the same app", async () => {
    let n = 0;
    const calls = stub({
      ...base,
      "GET /v1/admin/app-instances": () => json(200, { items: [], next_cursor: null }),
      "POST /v1/admin/app-instances": () => {
        n += 1;
        return json(422, {
          error: {
            code: "instance_host_conflict",
            message: "x",
            details: {
              fields: [{ path: "/host", message: "x" }],
              conflict: { kind: "instance", scope: "org", app_id: "desk", app_name: "Desk", instance_id: "i1", promotable: true, org: { id: "o1", name: "Acme", slug: "acme" } },
            },
          },
        });
      },
      "POST /v1/admin/app-instances/i1/promote": () => json(200, row({ org_id: null })),
    });
    wrap(<InstancesView scope={{ kind: "instance", all: true }} canManage />);
    fireEvent.click(await screen.findByRole("button", { name: "Register a self-hosted app" }));
    fireEvent.change(await screen.findByLabelText("App"), { target: { value: "desk" } });
    fireEvent.change(screen.getByLabelText("Host"), { target: { value: "desk.acme.example" } });
    fireEvent.click(screen.getByRole("button", { name: "Register" }));
    const alert = (await screen.findAllByRole("alert")).find((a) => /host|claimed|registered/.test(a.textContent ?? "") && a.querySelector("p, button, a"))!;
    expect(alert.textContent).toMatch(/registered as a self-hosted Desk by the organization Acme/);
    expect(n).toBe(1);
    fireEvent.click(within(alert).getByRole("button", { name: "Make instance-wide" }));
    await waitFor(() => expect(calls.some((c) => c.path === "/v1/admin/app-instances/i1/promote")).toBe(true));
  });

  it("a static claim links an instance admin to the host claims", async () => {
    stub({
      ...base,
      "GET /v1/admin/app-instances": () => json(200, { items: [], next_cursor: null }),
      "POST /v1/admin/app-instances": () =>
        json(422, {
          error: {
            code: "instance_host_conflict",
            message: "x",
            details: { fields: [{ path: "/host", message: "x" }], conflict: { kind: "static_claim", app_id: "plain", app_name: "Plain", package_id: "plainpkg" } },
          },
        }),
    });
    wrap(<InstancesView scope={{ kind: "instance", all: true }} canManage />);
    fireEvent.click(await screen.findByRole("button", { name: "Register a self-hosted app" }));
    fireEvent.change(await screen.findByLabelText("App"), { target: { value: "desk" } });
    fireEvent.change(screen.getByLabelText("Host"), { target: { value: "plain.example.test" } });
    fireEvent.click(screen.getByRole("button", { name: "Register" }));
    const alert = (await screen.findAllByRole("alert")).find((a) => /host|claimed|registered/.test(a.textContent ?? "") && a.querySelector("p, button, a"))!;
    expect(alert.textContent).toMatch(/claimed by the app Plain \(package plainpkg\)/);
    const link = await within(alert).findByRole("link", { name: "Host claims" });
    expect(link.getAttribute("href")).toBe("/admin/host-claims");
  });
});

describe("the rename is complete", () => {
  it("no user-visible 'Instances' label for this feature remains in the nav, pages or views", () => {
    const read = (p: string) => readFileSync(`${process.cwd()}/${p}`, "utf8");
    const shell = read("src/components/app-shell.tsx");
    expect(shell).not.toMatch(/label: "Instances"/);
    expect(shell.match(/label: "Self-hosted apps"/g)).toHaveLength(2);
    for (const p of ["src/app/admin/instances/page.tsx", "src/app/o/[org]/instances/page.tsx", "src/components/apps/instances-view.tsx"]) {
      const src = read(p);
      // string literals and JSX text a person can read
      expect(src, p).not.toMatch(/title="Instances"|label: "Instances"|>Instances<|"Register instance"|No instances registered/);
    }
  });
});
